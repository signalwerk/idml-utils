//! Lossless, arena-backed XML. Node IDs and formatting travel with nodes.
use crate::value::Value;
use crate::{Element, Error, ErrorKind, Location, ReadOptions};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

static NEXT_OWNER: AtomicUsize = AtomicUsize::new(1);
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

/// Stable within a tree and its clones; deleted IDs are never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId {
    owner: usize,
    index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name {
    pub namespace: String,
    pub local: String,
    pub prefix: Option<String>,
}
impl Name {
    pub fn qualified(&self) -> String {
        self.prefix.as_ref().map_or_else(|| self.local.clone(), |p| format!("{p}:{}", self.local))
    }
}

#[derive(Debug, Clone)]
pub struct Attribute {
    pub name: Name,
    pub value: String,
    original: Option<Range<usize>>,
    changed: bool,
}
impl Attribute {
    pub fn is_namespace(&self) -> bool {
        self.name.namespace == XMLNS_NS
    }
}

#[derive(Debug, Clone)]
pub enum NodeKind {
    Document,
    Element(Name),
    Text(String),
    Comment(String),
    Instruction {
        target: String,
        value: Option<String>,
    },
    /// Lexical material not represented by the XML parser (e.g. declaration).
    Raw,
}

#[derive(Debug, Clone)]
struct Record {
    kind: NodeKind,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    attributes: Vec<Attribute>,
    removed_attributes: Vec<Range<usize>>,
    source: Option<Range<usize>>,
    opening: Option<Range<usize>>,
    closing: Option<Range<usize>>,
    self_closing: bool,
    cdata: bool,
    dirty: bool,
    deleted: bool,
}

/// A lossless source document. Parsing, walking, writing, cloning and dropping
/// do not recurse with document depth. Reading never migrates or defaults data.
#[derive(Debug, Clone)]
pub struct XmlTree {
    source: Arc<str>,
    records: Vec<Record>,
    owner: usize,
    root: NodeId,
    revision: u64,
    /// Initial encoded length for a directly constructed producer part,
    /// including its generated XML declaration. Invalidated by any edit.
    producer_size: Option<u64>,
    /// A processing instruction written after the XML declaration of a produced part (the
    /// text between `<?` and `?>`, such as a new designmap's `aid` instruction).
    instruction: Option<Arc<str>>,
}

/// The bytes a produced part's instruction adds: `<?…?>` and a line feed.
fn instruction_len(instruction: Option<&str>) -> u64 {
    instruction.map_or(0, |i| i.len() as u64 + 5)
}

fn err(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::Xml, message)
}
fn write_err(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::Write, message)
}

/// Count lexical XML nodes and attributes before building an arena or DOM.
/// This pass borrows the input and does not collect nodes or attributes.
pub(crate) fn check_xml_counts(xml: &str, options: &ReadOptions) -> Result<(), Error> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut nodes = 0usize;
    let mut attributes = 0usize;
    loop {
        let event = reader.read_event().map_err(|e| err(e.to_string()))?;
        match event {
            Event::Eof => break,
            Event::End(_) => continue,
            Event::Start(ref e) | Event::Empty(ref e) => {
                for attribute in e.attributes().with_checks(false) {
                    attribute.map_err(|e| err(e.to_string()))?;
                    attributes = attributes.saturating_add(1);
                    if attributes > options.max_attributes {
                        return Err(Error::new(ErrorKind::Limit, "XML exceeds max_attributes"));
                    }
                }
            }
            _ => {}
        }
        nodes = nodes.saturating_add(1);
        if nodes > options.max_nodes {
            return Err(Error::new(ErrorKind::Limit, "XML exceeds max_nodes"));
        }
    }
    Ok(())
}

fn check_element_counts(root: &Element, defaults: &[(&str, &str)], instruction: Option<&str>, options: &ReadOptions) -> Result<u64, Error> {
    let mut pending = vec![(root, 1usize)];
    // The declaration (and instruction) added to a producer part.
    let mut nodes = 1usize + usize::from(instruction.is_some());
    let mut attributes = 0usize;
    // Producer serialization writes an XML declaration and canonical element
    // syntax. Count it before constructing records, without serializing or
    // reparsing the producer's tree.
    let mut encoded_size = crate::xml::DECLARATION.len() as u64 + instruction_len(instruction);
    while let Some((element, depth)) = pending.pop() {
        if depth > options.max_depth {
            return Err(Error::new(ErrorKind::Limit, "elements nested beyond max_depth"));
        }
        let text_nodes = element.text.as_ref().map_or(0, |text| producer_text_nodes(text, element.cdata));
        nodes = nodes.saturating_add(1).saturating_add(text_nodes);
        if nodes.saturating_add(pending.len()).saturating_add(element.children.len()) > options.max_nodes {
            return Err(Error::new(ErrorKind::Limit, "tree exceeds max_nodes"));
        }
        attributes = attributes.saturating_add(element.attributes.len());
        if depth == 1 {
            attributes = attributes.saturating_add(defaults.len());
        }
        if attributes > options.max_attributes {
            return Err(Error::new(ErrorKind::Limit, "tree exceeds max_attributes"));
        }
        let name_size = element.name.len() as u64;
        encoded_size = encoded_size.saturating_add(if element.text.is_some() || !element.children.is_empty() {
            name_size.saturating_mul(2).saturating_add(6)
        } else {
            name_size.saturating_add(4)
        });
        for (name, value) in &element.attributes {
            encoded_size = encoded_size.saturating_add(4 + name.len() as u64).saturating_add(escaped_attribute_len(value));
        }
        if depth == 1 {
            for (name, value) in defaults {
                encoded_size = encoded_size.saturating_add(4 + name.len() as u64).saturating_add(escaped_attribute_len(value));
            }
        }
        if let Some(text) = &element.text {
            encoded_size = encoded_size.saturating_add(if element.cdata && !text.contains('\r') && text.chars().all(xml_char) {
                (text.len() as u64).saturating_add(12).saturating_add((text.matches("]]>").count() as u64).saturating_mul(12))
            } else {
                escaped_text_len(text)
            });
        }
        if encoded_size > options.max_entry_size || encoded_size > options.max_size {
            return Err(Error::new(ErrorKind::Limit, "producer XML exceeds size budget"));
        }
        pending.extend(element.children.iter().rev().map(|child| (child, depth.saturating_add(1))));
    }
    Ok(encoded_size)
}

fn escaped_attribute_len(value: &str) -> u64 {
    value.chars().fold(0u64, |len, c| {
        len.saturating_add(match c {
            '&' | '\r' | '\n' | '\t' => 5,
            '<' => 4,
            '"' => 6,
            _ => c.len_utf8() as u64,
        })
    })
}

fn producer_text_nodes(value: &str, cdata: bool) -> usize {
    if cdata && !value.contains('\r') && value.chars().all(xml_char) {
        return 1usize.saturating_add(value.matches("]]>").count());
    }
    let mut nodes = 0usize;
    let mut in_text = false;
    for c in value.chars() {
        if matches!(c, '&' | '<' | '>' | '\r') || crate::xml::needs_ace(c) {
            nodes = nodes.saturating_add(1);
            in_text = false;
        } else if !in_text {
            nodes = nodes.saturating_add(1);
            in_text = true;
        }
    }
    // An empty text node exists in the producer arena even though it emits no text.
    nodes.max(1)
}

fn escaped_text_len(value: &str) -> u64 {
    value.chars().fold(0u64, |len, c| {
        len.saturating_add(match c {
            '&' | '\r' => 5,
            '<' | '>' => 4,
            c if crate::xml::needs_ace(c) => 8 + u64::from((32 - (c as u32).leading_zeros()).max(1).div_ceil(4)),
            _ => c.len_utf8() as u64,
        })
    })
}

impl XmlTree {
    pub fn parse(xml: &str, options: &ReadOptions) -> Result<Self, Error> {
        if xml.len() as u64 > options.max_size {
            return Err(Error::new(ErrorKind::Limit, "XML exceeds max_size"));
        }
        if xml.len() as u64 > options.max_entry_size {
            return Err(Error::new(ErrorKind::Limit, "XML exceeds max_entry_size"));
        }
        if crate::xml::nesting_exceeds(xml, options.max_depth) {
            return Err(Error::new(ErrorKind::Limit, "XML exceeds max_depth"));
        }
        check_xml_counts(xml, options)?;
        use quick_xml::{XmlVersion, events::Event};
        let mut reader = quick_xml::Reader::from_str(xml);
        reader.config_mut().check_comments = true;
        let owner = NEXT_OWNER.fetch_add(1, Ordering::Relaxed);
        let mut tree = Self {
            source: Arc::from(xml),
            records: Vec::new(),
            owner,
            root: NodeId { owner, index: 0 },
            revision: 0,
            producer_size: None,
            instruction: None,
        };
        let document = tree.push(NodeKind::Document, None);
        tree.records[document.index].source = Some(0..xml.len());
        let mut stack = vec![document];
        let mut root = None;

        loop {
            let start = reader.buffer_position() as usize;
            let event = reader.read_event().map_err(|e| err(e.to_string()))?;
            let end = reader.buffer_position() as usize;
            let parent = *stack.last().ok_or_else(|| err("unbalanced XML"))?;
            let id = match event {
                Event::Start(e) | Event::Empty(e) => {
                    if stack.len() > options.max_depth {
                        return Err(Error::new(ErrorKind::Limit, "XML exceeds max_depth"));
                    }
                    let lexical = e.name().0;
                    let mut bindings = tree.bindings(parent);
                    let mut ranges = lexical_attributes(xml, start..end)?.into_iter();
                    let mut attrs = Vec::new();
                    // Duplicate expanded names are checked after resolving
                    // namespaces below. Avoid quadratic duplicate checking and
                    // matching spans by position in the same lexical order.
                    for a in e.attributes().with_checks(false) {
                        let a = a.map_err(|e| err(e.to_string()))?;
                        if a.value.contains('<') {
                            return Err(err("literal < in attribute value"));
                        }
                        let value = a.normalized_value(XmlVersion::Implicit1_0).map_err(|e| err(e.to_string()))?.into_owned();
                        if value.chars().any(|c| !xml_char(c)) {
                            return Err(err("invalid attribute character"));
                        }
                        let name = a.key.0;
                        if !valid_xml_name(name) {
                            return Err(err("invalid attribute name"));
                        }
                        if name == "xmlns" {
                            bindings.insert(String::new(), value.clone());
                        } else if let Some(prefix) = name.strip_prefix("xmlns:") {
                            bindings.insert(prefix.into(), value.clone());
                        }
                        let (span_name, _, range) = ranges.next().ok_or_else(|| err("missing attribute span"))?;
                        if span_name != name {
                            return Err(err("attribute span order mismatch"));
                        }
                        attrs.push((name, value, range));
                    }
                    if ranges.next().is_some() {
                        return Err(err("extra attribute span"));
                    }
                    for (prefix, uri) in &bindings {
                        if prefix == "xmlns"
                            || uri == XMLNS_NS
                            || (uri == XML_NS && prefix != "xml")
                            || (prefix == "xml" && uri != XML_NS)
                            || (!prefix.is_empty() && uri.is_empty())
                        {
                            return Err(err("invalid namespace declaration"));
                        }
                    }
                    let name = resolve_name(lexical, false, &bindings)?;
                    let empty = xml.get(end - 2..end) == Some("/>");
                    let id = tree.push(NodeKind::Element(name), Some(parent));
                    if parent == document && root.replace(id).is_some() {
                        return Err(err("multiple document roots"));
                    }
                    tree.records[id.index].opening = Some(start..end);
                    tree.records[id.index].self_closing = empty;
                    let mut seen = BTreeSet::new();
                    for (lexical, value, range) in attrs {
                        let name = if lexical == "xmlns" {
                            Name { namespace: XMLNS_NS.into(), local: "xmlns".into(), prefix: None }
                        } else if let Some(p) = lexical.strip_prefix("xmlns:") {
                            Name { namespace: XMLNS_NS.into(), local: p.into(), prefix: Some("xmlns".into()) }
                        } else {
                            resolve_name(lexical, true, &bindings)?
                        };
                        if !seen.insert((name.namespace.clone(), name.local.clone())) {
                            return Err(err("duplicate expanded attribute name"));
                        }
                        tree.records[id.index].attributes.push(Attribute { name, value, original: Some(range), changed: false });
                    }
                    tree.records[id.index].source = Some(start..end);
                    if !empty {
                        stack.push(id);
                    }
                    id
                }
                Event::End(e) => {
                    if stack.len() <= 1 {
                        return Err(err("unexpected closing tag"));
                    }
                    let id = stack.pop().ok_or_else(|| err("unbalanced XML"))?;
                    if tree.name(id).map(|n| n.qualified()).as_deref() != Some(e.name().0) {
                        return Err(err("mismatched closing tag"));
                    }
                    let record = &mut tree.records[id.index];
                    let begin = record.opening.as_ref().map_or(start, |r| r.start);
                    record.source = Some(begin..end);
                    record.closing = Some(start..end);
                    continue;
                }
                Event::Text(t) => {
                    let text = t.xml10_content().into_owned();
                    if text.contains("]]>") {
                        return Err(err("literal ]]> outside CDATA"));
                    }
                    if text.chars().any(|c| !xml_char(c)) {
                        return Err(err("invalid text character"));
                    }
                    if parent == document && !text.trim_matches([' ', '\t', '\r', '\n']).is_empty() {
                        return Err(err("text outside document root"));
                    }
                    tree.push(NodeKind::Text(text), Some(parent))
                }
                Event::CData(t) => {
                    if parent == document {
                        return Err(err("CDATA outside root"));
                    }
                    let text = t.xml10_content().into_owned();
                    if text.chars().any(|c| !xml_char(c)) {
                        return Err(err("invalid CDATA character"));
                    }
                    let id = tree.push(NodeKind::Text(text), Some(parent));
                    tree.records[id.index].cdata = true;
                    id
                }
                Event::GeneralRef(r) => {
                    if parent == document {
                        return Err(err("reference outside root"));
                    }
                    let text = if let Some(c) = r.resolve_char_ref().map_err(|e| err(e.to_string()))? {
                        if !xml_char(c) {
                            return Err(err("invalid character reference"));
                        }
                        c.to_string()
                    } else {
                        quick_xml::escape::resolve_predefined_entity(r.as_ref()).ok_or_else(|| err("unknown entity"))?.into()
                    };
                    tree.push(NodeKind::Text(text), Some(parent))
                }
                Event::Comment(t) => tree.push(NodeKind::Comment(t.xml10_content().into_owned()), Some(parent)),
                Event::PI(p) => {
                    if p.target().eq_ignore_ascii_case("xml") {
                        return Err(err("reserved processing instruction target"));
                    }
                    tree.push(
                        NodeKind::Instruction {
                            target: p.target().into(),
                            value: (!p.content().trim().is_empty()).then(|| p.content().trim_start().into()),
                        },
                        Some(parent),
                    )
                }
                Event::Decl(d) => {
                    if parent != document || root.is_some() || !matches!(&xml[..start], "" | "\u{feff}") {
                        return Err(err("misplaced XML declaration"));
                    }
                    if d.version().map_err(|e| err(e.to_string()))?.as_ref() != "1.0" {
                        return Err(err("only XML 1.0 is supported"));
                    }
                    tree.push(NodeKind::Raw, Some(parent))
                }
                Event::DocType(_) => return Err(err("DTD declarations are not supported")),
                Event::Eof => break,
            };
            let r = &mut tree.records[id.index];
            if r.source.is_none() {
                r.source = Some(start..end);
            }
            tree.records[parent.index].children.push(id);
        }
        if stack.len() != 1 {
            return Err(err("unclosed element"));
        }
        tree.root = root.ok_or_else(|| err("missing root element"))?;
        for record in &mut tree.records {
            record.dirty = false;
        }
        Ok(tree)
    }

    /// Builds directly from producer elements; no XML string or XML parser.
    pub fn from_element(element: &Element) -> Result<Self, Error> {
        Self::from_element_with(element, &[], &ReadOptions::default()).map(|(tree, _)| tree)
    }
    pub(crate) fn from_element_with(element: &Element, defaults: &[(&str, &str)], options: &ReadOptions) -> Result<(Self, u64), Error> {
        Self::from_element_with_instruction(element, defaults, None, options)
    }

    /// A produced part with a processing instruction after its XML declaration.
    pub(crate) fn from_element_with_instruction(
        element: &Element,
        defaults: &[(&str, &str)],
        instruction: Option<&str>,
        options: &ReadOptions,
    ) -> Result<(Self, u64), Error> {
        let size = check_element_counts(element, defaults, instruction, options)?;
        let owner = NEXT_OWNER.fetch_add(1, Ordering::Relaxed);
        let mut tree = Self {
            source: Arc::from(""),
            records: Vec::new(),
            owner,
            root: NodeId { owner, index: 0 },
            revision: 0,
            producer_size: Some(size),
            instruction: instruction.map(Arc::from),
        };
        let root = tree.build_element(element, defaults)?;
        tree.root = root;
        Ok((tree, size))
    }

    pub fn root(&self) -> NodeId {
        self.root
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn original_xml(&self) -> &str {
        &self.source
    }
    /// The processing instruction written after the XML declaration of a produced part.
    pub fn producer_instruction(&self) -> Option<&str> {
        self.instruction.as_deref().filter(|_| self.source.is_empty())
    }
    fn record(&self, id: NodeId) -> Result<&Record, Error> {
        self.records
            .get(id.index)
            .filter(|r| id.owner == self.owner && !r.deleted)
            .ok_or_else(|| write_err("node does not belong to this tree, or was deleted"))
    }
    pub fn kind(&self, id: NodeId) -> Result<&NodeKind, Error> {
        Ok(&self.record(id)?.kind)
    }
    pub fn name(&self, id: NodeId) -> Option<&Name> {
        match &self.record(id).ok()?.kind {
            NodeKind::Element(n) => Some(n),
            _ => None,
        }
    }
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.record(id).ok()?.parent
    }
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        self.record(id).map(|r| r.children.as_slice()).unwrap_or(&[])
    }
    pub fn elements(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.children(id).iter().copied().filter(|id| self.name(*id).is_some())
    }
    pub fn descendants(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut stack = vec![id];
        std::iter::from_fn(move || {
            let id = stack.pop()?;
            stack.extend(self.children(id).iter().rev().copied());
            Some(id)
        })
    }
    pub fn attributes(&self, id: NodeId) -> impl Iterator<Item = &Attribute> {
        self.record(id).into_iter().flat_map(|r| &r.attributes).filter(|a| !a.is_namespace())
    }
    pub fn attribute(&self, id: NodeId, name: &str) -> Option<&str> {
        self.attributes(id).find(|a| a.name.qualified() == name).map(|a| a.value.as_str())
    }
    pub fn attribute_fragment(&self, id: NodeId, name: &str) -> Option<String> {
        let attribute = self.attributes(id).find(|a| a.name.qualified() == name)?;
        Some(
            if !attribute.changed
                && let Some(range) = &attribute.original
            {
                self.source[range.clone()].to_owned()
            } else {
                format!("{}=\"{}\"", attribute.name.qualified(), escape_attribute(&attribute.value))
            },
        )
    }
    pub fn attribute_ns(&self, id: NodeId, ns: &str, local: &str) -> Option<&str> {
        self.attributes(id).find(|a| a.name.namespace == ns && a.name.local == local).map(|a| a.value.as_str())
    }
    pub fn text(&self, id: NodeId) -> Result<String, Error> {
        let mut out = String::new();
        for &child in self.children(id) {
            match self.kind(child)? {
                NodeKind::Text(t) => out.push_str(t),
                NodeKind::Instruction { target, value } if target == "ACE" => {
                    let c = value
                        .as_deref()
                        .and_then(|v| u32::from_str_radix(v.trim(), 16).ok())
                        .and_then(char::from_u32)
                        .ok_or_else(|| err("invalid ACE instruction"))?;
                    out.push(c);
                }
                NodeKind::Element(_) => return Err(err("element where text is expected")),
                _ => {}
            }
        }
        Ok(out)
    }
    pub fn dom_version(&self) -> Option<&str> {
        self.attribute(self.root, "DOMVersion")
    }
    pub fn location(&self, id: NodeId) -> Location {
        let mut path = Vec::new();
        let mut current = Some(id);
        while let Some(n) = current {
            if let Some(name) = self.name(n) {
                let q = name.qualified();
                let position = self.parent(n).map_or(1, |p| {
                    self.elements(p)
                        .take_while(|c| *c != n)
                        .filter(|c| self.name(*c).is_some_and(|cn| cn.namespace == name.namespace && cn.local == name.local))
                        .count()
                        + 1
                });
                path.push(if position == 1 { q } else { format!("{q}[{position}]") });
            }
            current = self.parent(n);
        }
        path.reverse();
        let offset = self.record(id).ok().and_then(|r| r.source.as_ref()).map_or(0, |r| r.start);
        let prefix = self.source.get(..offset).unwrap_or("");
        Location {
            part: String::new(),
            line: prefix.bytes().filter(|b| *b == b'\n').count() as u32 + 1,
            column: prefix.rsplit('\n').next().unwrap_or("").chars().count() as u32 + 1,
            element: path.join("/"),
        }
    }
    fn bindings(&self, id: NodeId) -> BTreeMap<String, String> {
        let mut out = BTreeMap::from([("xml".into(), XML_NS.into()), ("".into(), "".into())]);
        let mut chain = Vec::new();
        let mut at = Some(id);
        while let Some(n) = at {
            chain.push(n);
            at = self.parent(n);
        }
        for n in chain.into_iter().rev() {
            if let Ok(r) = self.record(n) {
                for a in &r.attributes {
                    if a.is_namespace() {
                        out.insert(if a.name.prefix.is_some() { a.name.local.clone() } else { String::new() }, a.value.clone());
                    }
                }
            }
        }
        out
    }
    /// The in-scope URI of a namespace prefix; use an empty prefix for the
    /// default namespace. Missing/invalid source nodes return None.
    pub fn namespace_binding(&self, id: NodeId, prefix: &str) -> Option<String> {
        self.record(id).ok()?;
        self.bindings(id).get(prefix).cloned()
    }
    fn dirty(&mut self, id: NodeId) {
        let mut at = Some(id);
        while let Some(n) = at {
            let r = &mut self.records[n.index];
            r.dirty = true;
            at = r.parent;
        }
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn set_attribute(&mut self, id: NodeId, name: &str, value: &str) -> Result<(), Error> {
        if id == self.root && name == "DOMVersion" && self.attribute(id, name) != Some(value) {
            return Err(write_err("changing DOMVersion requires a validated conversion plan"));
        }
        self.set_attribute_internal(id, name, value)
    }
    fn set_attribute_internal(&mut self, id: NodeId, name: &str, value: &str) -> Result<(), Error> {
        self.record(id)?;
        if name == "xmlns" || name.starts_with("xmlns:") {
            return Err(write_err("use declare_namespace; namespace rebinding is not implicit"));
        }
        if !valid_xml_name(name) || value.chars().any(|c| !xml_char(c)) {
            return Err(write_err("invalid XML attribute name or value"));
        }
        let bindings = self.bindings(id);
        let q = resolve_name(name, true, &bindings)?;
        let r = &mut self.records[id.index];
        if !matches!(r.kind, NodeKind::Element(_)) {
            return Err(write_err("attribute requires an element"));
        }
        if let Some(a) = r.attributes.iter_mut().find(|a| a.name.namespace == q.namespace && a.name.local == q.local) {
            if a.value == value {
                return Ok(());
            }
            a.value = value.into();
            a.changed = true;
        } else {
            r.attributes.push(Attribute { name: q, value: value.into(), original: None, changed: true });
        }
        self.dirty(id);
        Ok(())
    }
    pub fn remove_attribute(&mut self, id: NodeId, name: &str) -> Result<(), Error> {
        if id == self.root && name == "DOMVersion" {
            return Err(write_err("removing DOMVersion is not a conversion"));
        }
        let r = self.record(id)?;
        let Some(i) = r.attributes.iter().position(|a| !a.is_namespace() && a.name.qualified() == name) else {
            return Ok(());
        };
        let a = self.records[id.index].attributes.remove(i);
        if let Some(range) = a.original {
            self.records[id.index].removed_attributes.push(range);
        }
        self.dirty(id);
        Ok(())
    }
    pub fn declare_namespace(&mut self, id: NodeId, prefix: &str, uri: &str) -> Result<(), Error> {
        self.record(id)?;
        if self.name(id).is_none() {
            return Err(write_err("namespace declarations require an element"));
        }
        if prefix == "xmlns"
            || prefix.contains(':')
            || uri == XMLNS_NS
            || (uri == XML_NS && prefix != "xml")
            || (prefix == "xml" && uri != XML_NS)
            || (!prefix.is_empty() && !valid_xml_name(prefix))
            // Namespaces in XML 1.0 cannot undeclare a prefix (`xmlns:p=""`).
            || (!prefix.is_empty() && uri.is_empty())
            || uri.chars().any(|c| !xml_char(c))
        {
            return Err(write_err("invalid namespace binding"));
        }
        let existing = self.bindings(id);
        if existing.get(prefix).is_some_and(|u| u == uri) {
            return Ok(());
        }
        if existing.get(prefix).is_some_and(|u| !u.is_empty()) {
            return Err(write_err("rebinding an existing namespace could change unknown content"));
        }
        if prefix.is_empty() && self.name(id).is_some_and(|n| n.prefix.is_none() && n.namespace != uri) {
            return Err(write_err("default namespace would change the existing element name"));
        }
        self.namespace_unchecked(id, prefix, uri);
        Ok(())
    }
    fn namespace_unchecked(&mut self, id: NodeId, prefix: &str, uri: &str) {
        let r = &mut self.records[id.index];
        let local = if prefix.is_empty() { "xmlns" } else { prefix };
        if let Some(a) = r.attributes.iter_mut().find(|a| a.is_namespace() && a.name.local == local) {
            a.value = uri.into();
            a.changed = true;
        } else {
            r.attributes.push(Attribute {
                name: Name { namespace: XMLNS_NS.into(), local: local.into(), prefix: (!prefix.is_empty()).then(|| "xmlns".into()) },
                value: uri.into(),
                original: None,
                changed: true,
            });
        }
        self.dirty(id);
    }
    /// Replaces text while retaining comments, instructions and namespace data.
    /// Mixed element content must be edited by node, not erased through a scalar setter.
    pub fn set_text(&mut self, id: NodeId, text: &str) -> Result<(), Error> {
        self.record(id)?;
        if self.name(id).is_none() {
            return Err(write_err("set_text requires an element"));
        }
        if self.elements(id).next().is_some() {
            return Err(write_err("set_text would erase element content"));
        }
        if text.chars().any(|c| !xml_char(c) && !crate::xml::needs_ace(c)) {
            return Err(write_err("text contains a character XML cannot represent"));
        }
        let cdata = self.children(id).iter().any(|n| self.records[n.index].cdata);
        let old = self.children(id).to_vec();
        let mut keep = Vec::new();
        let mut first = None;
        for child in old {
            if matches!(self.kind(child)?, NodeKind::Text(_)) || matches!(self.kind(child)?, NodeKind::Instruction { target, .. } if target == "ACE")
            {
                if first.is_none() {
                    first = Some(keep.len());
                }
                self.records[child.index].deleted = true;
            } else {
                keep.push(child);
            }
        }
        let new = self.push(NodeKind::Text(text.into()), Some(id));
        self.records[new.index].cdata = cdata;
        keep.insert(first.unwrap_or(0), new);
        self.records[id.index].children = keep;
        self.dirty(id);
        Ok(())
    }
    pub fn insert_element(&mut self, parent: NodeId, element_index: usize, element: &Element) -> Result<NodeId, Error> {
        self.record(parent)?;
        if self.name(parent).is_none() || element_index > self.elements(parent).count() {
            return Err(write_err("invalid parent or child index"));
        }
        let mut subtree = Self::from_element_in(element, &self.bindings(parent))?;
        let root = self.import(&mut subtree);
        self.attach(parent, element_index, root)?;
        Ok(root)
    }
    /// Move the node itself, retaining its ID, lexical form and namespace context.
    pub fn move_node(&mut self, id: NodeId, parent: NodeId, element_index: usize) -> Result<(), Error> {
        self.record(id)?;
        self.record(parent)?;
        if self.name(id).is_none() {
            return Err(write_err("only elements can carry namespace context through a move"));
        }
        if id == self.root {
            return Err(write_err("cannot move the root element"));
        }
        let mut at = Some(parent);
        while let Some(n) = at {
            if n == id {
                return Err(write_err("move would create a cycle"));
            }
            at = self.parent(n);
        }
        let remaining = self.elements(parent).count().saturating_sub(usize::from(self.parent(id) == Some(parent)));
        if self.name(parent).is_none() || element_index > remaining {
            return Err(write_err("invalid parent or child index"));
        }
        let previous = self.bindings(id);
        let target = self.bindings(parent);
        if let Some(old_parent) = self.parent(id) {
            self.records[old_parent.index].children.retain(|c| *c != id);
            self.dirty(old_parent);
        }
        self.attach(parent, element_index, id)?;
        for (prefix, uri) in previous {
            if target.get(&prefix) != Some(&uri) {
                self.namespace_unchecked(id, &prefix, &uri);
            }
        }
        Ok(())
    }
    pub fn remove_node(&mut self, id: NodeId) -> Result<(), Error> {
        self.record(id)?;
        if id == self.root {
            return Err(write_err("cannot delete the root element"));
        }
        let parent = self.parent(id).ok_or_else(|| write_err("node has no parent"))?;
        let nodes: Vec<_> = self.descendants(id).collect();
        for n in nodes {
            self.records[n.index].deleted = true;
        }
        self.records[parent.index].children.retain(|c| *c != id);
        self.dirty(parent);
        Ok(())
    }
    fn attach(&mut self, parent: NodeId, index: usize, id: NodeId) -> Result<(), Error> {
        if self.name(parent).is_none() {
            return Err(write_err("parent must be an element"));
        }
        let elements: Vec<_> = self.elements(parent).collect();
        if index > elements.len() {
            return Err(write_err("child index out of bounds"));
        }
        let position = elements.get(index).and_then(|e| self.children(parent).iter().position(|c| c == e)).unwrap_or(self.children(parent).len());
        self.records[parent.index].children.insert(position, id);
        self.records[id.index].parent = Some(parent);
        self.dirty(parent);
        Ok(())
    }
    fn push(&mut self, kind: NodeKind, parent: Option<NodeId>) -> NodeId {
        let id = NodeId { owner: self.owner, index: self.records.len() };
        self.records.push(Record {
            kind,
            parent,
            children: Vec::new(),
            attributes: Vec::new(),
            removed_attributes: Vec::new(),
            source: None,
            opening: None,
            closing: None,
            self_closing: true,
            cdata: false,
            dirty: true,
            deleted: false,
        });
        id
    }
    fn build_element(&mut self, root: &Element, defaults: &[(&str, &str)]) -> Result<NodeId, Error> {
        self.build_element_in(root, defaults, BTreeMap::from([("xml".into(), XML_NS.into()), ("".into(), "".into())]))
    }
    fn from_element_in(root: &Element, bindings: &BTreeMap<String, String>) -> Result<Self, Error> {
        let owner = NEXT_OWNER.fetch_add(1, Ordering::Relaxed);
        let mut tree = Self {
            source: Arc::from(""),
            records: Vec::new(),
            owner,
            root: NodeId { owner, index: 0 },
            revision: 0,
            producer_size: None,
            instruction: None,
        };
        tree.root = tree.build_element_in(root, &[], bindings.clone())?;
        Ok(tree)
    }
    fn build_element_in(&mut self, root: &Element, defaults: &[(&str, &str)], inherited: BTreeMap<String, String>) -> Result<NodeId, Error> {
        let mut pending = vec![(root, None, inherited, true)];
        let mut first = None;
        while let Some((element, parent, mut bindings, is_root)) = pending.pop() {
            if element.text.is_some() && !element.children.is_empty() {
                return Err(err("element contains both text and child elements"));
            }
            let attrs: Vec<_> =
                defaults.iter().copied().filter(|_| is_root).chain(element.attributes.iter().map(|(n, v)| (n.as_str(), v.as_str()))).collect();
            for (name, value) in &attrs {
                if *name == "xmlns" {
                    bindings.insert("".into(), (*value).into());
                } else if let Some(p) = name.strip_prefix("xmlns:") {
                    bindings.insert(p.into(), (*value).into());
                }
            }
            for (prefix, uri) in &bindings {
                if prefix == "xmlns"
                    || uri == XMLNS_NS
                    || (uri == XML_NS && prefix != "xml")
                    || (prefix == "xml" && uri != XML_NS)
                    || (!prefix.is_empty() && uri.is_empty())
                {
                    return Err(err("invalid namespace declaration"));
                }
            }
            let name = resolve_name(&element.name, false, &bindings)?;
            let id = self.push(NodeKind::Element(name), parent);
            first.get_or_insert(id);
            if let Some(parent) = parent {
                self.records[parent.index].children.push(id);
            }
            let mut seen = BTreeSet::new();
            for (lexical, value) in attrs {
                if value.chars().any(|c| !xml_char(c)) {
                    return Err(err("invalid attribute character"));
                }
                let name = if lexical == "xmlns" {
                    Name { namespace: XMLNS_NS.into(), prefix: None, local: "xmlns".into() }
                } else if let Some(p) = lexical.strip_prefix("xmlns:") {
                    Name { namespace: XMLNS_NS.into(), prefix: Some("xmlns".into()), local: p.into() }
                } else {
                    resolve_name(lexical, true, &bindings)?
                };
                if !seen.insert((name.namespace.clone(), name.local.clone())) {
                    return Err(err("duplicate attribute"));
                }
                self.records[id.index].attributes.push(Attribute { name, value: value.into(), original: None, changed: true });
            }
            if let Some(text) = &element.text {
                if text.chars().any(|c| !xml_char(c) && !crate::xml::needs_ace(c)) {
                    return Err(err("invalid text character"));
                }
                let child = self.push(NodeKind::Text(text.clone()), Some(id));
                self.records[child.index].cdata = element.cdata;
                self.records[id.index].children.push(child);
            }
            pending.extend(element.children.iter().rev().map(|c| (c, Some(id), bindings.clone(), false)));
        }
        first.ok_or_else(|| err("missing root"))
    }
    fn import(&mut self, other: &mut Self) -> NodeId {
        let offset = self.records.len();
        let root = NodeId { owner: self.owner, index: offset + other.root.index };
        for mut r in other.records.drain(..) {
            r.parent = r.parent.map(|n| NodeId { owner: self.owner, index: offset + n.index });
            r.children = r.children.into_iter().map(|n| NodeId { owner: self.owner, index: offset + n.index }).collect();
            self.records.push(r);
        }
        root
    }
    pub fn to_xml(&self) -> Result<String, Error> {
        let start = if matches!(self.records.first().map(|r| &r.kind), Some(NodeKind::Document)) {
            NodeId { owner: self.owner, index: 0 }
        } else {
            self.root
        };
        let xml = self.serialize(start)?;
        // The serializer preserves namespace context and escaping. Parsing is a
        // validation step here, never part of element construction or typed reading.
        Self::parse(
            &xml,
            &ReadOptions {
                max_depth: usize::MAX,
                max_size: u64::MAX,
                max_entry_size: u64::MAX,
                max_nodes: usize::MAX,
                max_attributes: usize::MAX,
                ..ReadOptions::default()
            },
        )
        .map_err(|e| write_err(e.to_string()))?;
        Ok(xml)
    }
    pub fn fragment(&self, id: NodeId) -> Result<String, Error> {
        self.record(id)?;
        self.serialize(id)
    }
    fn serialize(&self, id: NodeId) -> Result<String, Error> {
        enum Action {
            Node(NodeId),
            Close(String),
        }
        let mut out = String::new();
        let mut stack = vec![Action::Node(id)];
        while let Some(action) = stack.pop() {
            match action {
                Action::Close(s) => out.push_str(&s),
                Action::Node(id) => {
                    let r = self.record(id)?;
                    if !r.dirty
                        && let Some(range) = &r.source
                    {
                        out.push_str(&self.source[range.clone()]);
                        continue;
                    }
                    match &r.kind {
                        NodeKind::Document => stack.extend(r.children.iter().rev().copied().map(Action::Node)),
                        NodeKind::Element(name) => {
                            let q = name.qualified();
                            let empty = r.children.is_empty();
                            let mut open = if let Some(range) = &r.opening {
                                let mut changes: Vec<_> = r.removed_attributes.iter().cloned().map(|r| (r, String::new())).collect();
                                let mut additions = String::new();
                                for a in &r.attributes {
                                    if !a.changed {
                                        continue;
                                    }
                                    let rendered = format!("{}=\"{}\"", a.name.qualified(), escape_attribute(&a.value));
                                    if let Some(r) = &a.original {
                                        changes.push((r.clone(), rendered));
                                    } else {
                                        additions.push(' ');
                                        additions.push_str(&rendered);
                                    }
                                }
                                let insert = range.end - if r.self_closing { 2 } else { 1 };
                                if !additions.is_empty() {
                                    changes.push((insert..insert, additions));
                                }
                                changes.sort_by_key(|(r, _)| r.start);
                                let mut rendered = String::new();
                                let mut at = range.start;
                                for (r, replacement) in changes {
                                    if r.start < at {
                                        return Err(write_err("overlapping attribute changes"));
                                    }
                                    rendered.push_str(&self.source[at..r.start]);
                                    rendered.push_str(&replacement);
                                    at = r.end;
                                }
                                rendered.push_str(&self.source[at..range.end]);
                                rendered
                            } else {
                                let mut s = format!("<{q}");
                                for a in &r.attributes {
                                    s.push_str(&format!(" {}=\"{}\"", a.name.qualified(), escape_attribute(&a.value)));
                                }
                                s.push_str(" />");
                                s
                            };
                            let was_empty = open.ends_with("/>");
                            if !empty && was_empty {
                                open.truncate(open.len() - 2);
                                open.push('>');
                            }
                            out.push_str(&open);
                            if empty && was_empty {
                                continue;
                            }
                            let close = r.closing.as_ref().map_or_else(|| format!("</{q}>"), |r| self.source[r.clone()].to_owned());
                            stack.push(Action::Close(close));
                            stack.extend(r.children.iter().rev().copied().map(Action::Node));
                        }
                        NodeKind::Text(t) if r.cdata && !t.contains('\r') && t.chars().all(xml_char) => {
                            out.push_str("<![CDATA[");
                            out.push_str(&t.replace("]]>", "]]]]><![CDATA[>"));
                            out.push_str("]]>");
                        }
                        NodeKind::Text(t) => escape_text(t, &mut out),
                        NodeKind::Comment(t) => {
                            if t.contains("--") || t.ends_with('-') {
                                return Err(write_err("invalid comment"));
                            }
                            out.push_str(&format!("<!--{t}-->"));
                        }
                        NodeKind::Instruction { target, value } => {
                            out.push_str(&format!("<?{target}{}?>", value.as_ref().map_or(String::new(), |v| format!(" {v}"))))
                        }
                        NodeKind::Raw => {
                            if let Some(range) = &r.source {
                                out.push_str(&self.source[range.clone()]);
                            }
                        }
                    }
                }
            }
        }
        Ok(out)
    }
    pub fn check_limits(&self, options: &ReadOptions) -> Result<(), Error> {
        self.check_limits_and_size(options).map(|_| ())
    }
    pub(crate) fn check_limits_and_size(&self, options: &ReadOptions) -> Result<u64, Error> {
        // The declaration (and instruction) on produced parts.
        let mut count = usize::from(self.source.is_empty()) + usize::from(self.producer_instruction().is_some());
        let mut attributes = 0usize;
        for record in self.records.iter().filter(|record| !record.deleted) {
            let added = match &record.kind {
                NodeKind::Document => 0,
                NodeKind::Text(text) if self.source.is_empty() => producer_text_nodes(text, record.cdata),
                _ => 1,
            };
            count = count.saturating_add(added);
            attributes = attributes.saturating_add(record.attributes.len());
            if count > options.max_nodes {
                return Err(Error::new(ErrorKind::Limit, "tree exceeds max_nodes"));
            }
            if attributes > options.max_attributes {
                return Err(Error::new(ErrorKind::Limit, "tree exceeds max_attributes"));
            }
        }
        let mut stack = vec![(self.root, 1usize)];
        while let Some((id, depth)) = stack.pop() {
            if depth > options.max_depth {
                return Err(Error::new(ErrorKind::Limit, "elements nested beyond max_depth"));
            }
            let r = self.record(id)?;
            stack.extend(r.children.iter().map(|n| (*n, depth + usize::from(self.name(*n).is_some()))));
        }
        let size = if self.revision == 0 && !self.source.is_empty() {
            self.source.len() as u64
        } else if self.revision == 0 && self.producer_size.is_some() {
            self.producer_size.unwrap_or(0)
        } else {
            // Edited trees can have longer or shorter encoded values than
            // their source spans. Check the actual serialized form, including
            // nodes created by escaping and ACE instructions.
            let start = if matches!(self.records.first().map(|r| &r.kind), Some(NodeKind::Document)) {
                NodeId { owner: self.owner, index: 0 }
            } else {
                self.root
            };
            let serialized = self.serialize(start)?;
            let mut counts_options = options.clone();
            if self.source.is_empty() {
                counts_options.max_nodes = counts_options.max_nodes.saturating_sub(1 + usize::from(self.producer_instruction().is_some()));
            }
            check_xml_counts(&serialized, &counts_options)?;
            (serialized.len() as u64).saturating_add(if self.source.is_empty() {
                crate::xml::DECLARATION.len() as u64 + instruction_len(self.producer_instruction())
            } else {
                0
            })
        };
        if size > options.max_entry_size || size > options.max_size {
            return Err(Error::new(ErrorKind::Limit, "tree exceeds size budget"));
        }
        Ok(size)
    }
    #[cfg(feature = "owned")]
    pub(crate) fn explicit_end(&self, id: NodeId) -> bool {
        self.record(id).is_ok_and(|r| !r.self_closing)
    }
    #[cfg(feature = "owned")]
    pub(crate) fn cdata(&self, id: NodeId) -> Option<bool> {
        self.children(id).iter().find(|id| matches!(self.kind(**id), Ok(NodeKind::Text(_)))).map(|id| self.records[id.index].cdata)
    }
    pub(crate) fn resolve_attribute(&self, node: NodeId, name: &str) -> Result<Name, Error> {
        self.record(node)?;
        resolve_name(name, true, &self.bindings(node))
    }
    pub(crate) fn reversion(&mut self, version: &str) -> Result<(), Error> {
        self.set_attribute_internal(self.root, "DOMVersion", version)
    }
}

type LexicalAttribute<'a> = (&'a str, Range<usize>, Range<usize>);
fn lexical_attributes(xml: &str, open: Range<usize>) -> Result<Vec<LexicalAttribute<'_>>, Error> {
    let bytes = xml.as_bytes();
    let mut at = open.start + 1;
    while at < open.end && !bytes[at].is_ascii_whitespace() && !matches!(bytes[at], b'/' | b'>') {
        at += 1;
    }
    let mut attrs = Vec::new();
    loop {
        while at < open.end && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if at >= open.end || matches!(bytes[at], b'/' | b'>') {
            break;
        }
        let start = at;
        while at < open.end && !bytes[at].is_ascii_whitespace() && bytes[at] != b'=' {
            at += 1;
        }
        let name = &xml[start..at];
        while at < open.end && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if bytes.get(at) != Some(&b'=') {
            return Err(err("attribute without ="));
        }
        at += 1;
        while at < open.end && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let quote = *bytes.get(at).ok_or_else(|| err("missing attribute quote"))?;
        at += 1;
        let value = at;
        while at < open.end && bytes[at] != quote {
            at += 1;
        }
        if at >= open.end {
            return Err(err("missing attribute end"));
        }
        let value_range = value..at;
        at += 1;
        attrs.push((name, value_range, start..at));
    }
    Ok(attrs)
}
fn resolve_name(name: &str, attribute: bool, bindings: &BTreeMap<String, String>) -> Result<Name, Error> {
    if !valid_xml_name(name) {
        return Err(err(format!("invalid XML name {name:?}")));
    }
    let (prefix, local) = name.split_once(':').map_or((None, name), |(p, l)| (Some(p), l));
    let ns = match prefix {
        Some(p) => bindings.get(p).ok_or_else(|| err(format!("undeclared prefix {p}")))?.as_str(),
        None if attribute => "",
        None => bindings.get("").map_or("", String::as_str),
    };
    Ok(Name { namespace: ns.into(), local: local.into(), prefix: prefix.map(Into::into) })
}
fn valid_xml_name(name: &str) -> bool {
    fn start(c: char) -> bool {
        matches!(c, 'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
    }
    let mut parts = name.split(':');
    let valid = |s: &str| {
        let mut c = s.chars();
        c.next().is_some_and(start)
            && c.all(|c| start(c) || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}'))
    };
    parts.next().is_some_and(valid) && parts.next().is_none_or(valid) && parts.next().is_none()
}
fn xml_char(c: char) -> bool {
    matches!(c, '\t' | '\r' | '\n' | ' '..='\u{FFFD}' | '\u{10000}'..)
}
fn escape_attribute(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            '\r' => out.push_str("&#xd;"),
            '\n' => out.push_str("&#xa;"),
            '\t' => out.push_str("&#x9;"),
            c => out.push(c),
        }
    }
    out
}
fn escape_text(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#xd;"),
            c if crate::xml::needs_ace(c) => out.push_str(&format!("<?ACE {:x}?>", c as u32)),
            c => out.push(c),
        }
    }
}

/// The lexical value is retained even when its typed interpretation is unknown.
#[derive(Debug, Clone, PartialEq)]
pub enum Field<'a, T> {
    Absent,
    Known(T),
    Unrecognized { raw: std::borrow::Cow<'a, str>, reason: String },
}
impl XmlTree {
    pub fn field<T: Value>(&self, node: NodeId, name: &str) -> Field<'_, T> {
        match self.attribute(node, name) {
            None => Field::Absent,
            Some(raw) => match T::parse(raw) {
                Ok(v) => Field::Known(v),
                Err(e) => Field::Unrecognized { raw: std::borrow::Cow::Borrowed(raw), reason: e.to_string() },
            },
        }
    }
    pub fn set_value<T: Value>(&mut self, node: NodeId, name: &str, value: &T) -> Result<(), Error> {
        let mut text = String::new();
        value.format(&mut text);
        self.set_attribute(node, name, &text)
    }
}
