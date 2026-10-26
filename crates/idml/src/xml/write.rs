//! Writing: generated code builds an element tree, which is serialized as
//! InDesign writes IDML.
//!
//! Elements with element content are indented with tabs; elements with
//! text content are written without added whitespace. IDML has no mixed
//! content, so indentation never changes a value. The tree is complete before
//! it is serialized, so that a [`Layout`] can order attributes and children.

use std::fmt::Write as _;

use super::{PACKAGING_NS, Root, needs_ace};
use crate::element::Element;
use crate::error::{Error, ErrorKind, Location};
use crate::layout::{ElementLayout, Layout, PartLayout};
use crate::value::Value;

/// Guards the stack against absurdly deep models.
const MAX_DEPTH: usize = 512;

/// The XML declaration of every part.
pub(crate) const DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

/// Elements whose text InDesign writes as CDATA sections.
const CDATA: &[&str] = &[
    "Contents",
    "EPSTextData",
    "GaijiRefMaps",
    "DocsFlowImportSettings",
    "WordsFlowBaseDocument",
    "WordsFlowLeftDocument",
    "WordsFlowRightDocument",
];

/// The longest CDATA section InDesign writes, in bytes.
const CDATA_SECTION: usize = 262_144;

pub(crate) struct Writer<'l> {
    part: String,
    layout: &'l Layout,
    stack: Vec<Node>,
    root: Option<Node>,
    /// The parts referenced from this one, as (path, XML).
    parts: Vec<(String, String)>,
}

/// An element as generated code writes it: its names are the schema's.
#[derive(Default)]
struct Node {
    name: &'static str,
    attributes: Vec<(&'static str, String)>,
    children: Vec<Node>,
    text: Option<String>,
    cdata: bool,
}

/// An element tree to serialize: an [`Element`] or the writer's own.
pub(crate) trait Tree: Sized {
    fn name(&self) -> &str;
    fn attributes(&self) -> impl Iterator<Item = (&str, &str)>;
    fn children(&self) -> &[Self];
    fn text(&self) -> Option<&str>;
    fn cdata(&self) -> bool;
}

impl Tree for Element {
    fn name(&self) -> &str {
        &self.name
    }

    fn attributes(&self) -> impl Iterator<Item = (&str, &str)> {
        self.attributes.iter().map(|(n, v)| (n.as_str(), v.as_str()))
    }

    fn children(&self) -> &[Self] {
        &self.children
    }

    fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    fn cdata(&self) -> bool {
        self.cdata
    }
}

impl Tree for Node {
    fn name(&self) -> &str {
        self.name
    }

    fn attributes(&self) -> impl Iterator<Item = (&str, &str)> {
        self.attributes.iter().map(|(n, v)| (*n, v.as_str()))
    }

    fn children(&self) -> &[Self] {
        &self.children
    }

    fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    fn cdata(&self) -> bool {
        self.cdata
    }
}

/// A serialized part and the parts it references.
pub(crate) struct Written {
    pub xml: String,
    pub parts: Vec<(String, String)>,
}

impl<'l> Writer<'l> {
    /// Writes the part `part` as a complete XML document with root element
    /// `root`, preceded by the processing instruction `pi` if given.
    pub fn document(
        layout: &'l Layout,
        part: &str,
        root: &'static str,
        pi: Option<&str>,
        f: impl FnOnce(&mut Writer<'l>) -> Result<(), Error>,
    ) -> Result<Written, Error> {
        let mut w = Writer { part: part.to_owned(), layout, stack: Vec::new(), root: None, parts: Vec::new() };
        w.elem(root, |w| {
            w.fixed("xmlns:idPkg", PACKAGING_NS)?;
            f(w)
        })?;
        let root = w.root.take().ok_or_else(|| w.error("no root element".to_owned()))?;
        let indesign;
        let part_layout = match layout.parts.get(part) {
            Some(l) => l,
            None => {
                indesign = PartLayout::indesign(part);
                &indesign
            }
        };
        let mut out = String::from(DECLARATION);
        if let Some(pi) = pi {
            let _ = writeln!(out, "<?{pi}?>");
        }
        serialize(&mut out, &root, &[], Some(&part_layout.root)).map_err(|e| e.in_part(part))?;
        if part_layout.final_newline {
            out.push('\n');
        }
        Ok(Written { xml: out, parts: w.parts })
    }

    fn error(&self, message: String) -> Error {
        let element = self.stack.iter().map(|e| e.name).collect::<Vec<_>>().join("/");
        Error::new(ErrorKind::Package, message).at(Location { part: self.part.clone(), element, ..Location::default() })
    }

    pub fn elem(&mut self, name: &'static str, f: impl FnOnce(&mut Writer<'l>) -> Result<(), Error>) -> Result<(), Error> {
        if self.stack.len() >= MAX_DEPTH {
            return Err(self.error("elements nested too deeply".to_owned()));
        }
        if self.stack.last().is_some_and(|parent| parent.text.is_some()) {
            return Err(self.error("element after text".to_owned()));
        }
        self.stack.push(Node { name, ..Node::default() });
        f(self)?;
        let element = self.stack.pop().ok_or_else(|| self.error("unbalanced element".to_owned()))?;
        match self.stack.last_mut() {
            Some(parent) => parent.children.push(element),
            None => self.root = Some(element),
        }
        Ok(())
    }

    fn put_attr(&mut self, name: &'static str, value: String) -> Result<(), Error> {
        if let Some(c) = value.chars().find(|c| !is_xml_char(*c)) {
            return Err(self.error(format!("attribute {name} contains U+{:04X}, which XML cannot represent", u32::from(c))));
        }
        match self.stack.last_mut() {
            Some(e) if e.children.is_empty() && e.text.is_none() => {
                e.attributes.push((name, value));
                Ok(())
            }
            _ => Err(self.error("attribute after content".to_owned())),
        }
    }

    pub fn attr<V: Value>(&mut self, name: &'static str, value: &V) -> Result<(), Error> {
        let mut text = String::new();
        value.format(&mut text);
        self.put_attr(name, text)
    }

    pub fn opt_attr<V: Value>(&mut self, name: &'static str, value: &Option<V>) -> Result<(), Error> {
        match value {
            Some(v) => self.attr(name, v),
            None => Ok(()),
        }
    }

    /// An attribute with a literal value. `DOMVersion` is written as the
    /// layout says.
    pub fn fixed(&mut self, name: &'static str, value: &str) -> Result<(), Error> {
        let value = match (name, &self.layout.dom_version) {
            ("DOMVersion", Some(v)) => v.clone(),
            _ => value.to_owned(),
        };
        self.put_attr(name, value)
    }

    pub fn text<V: Value>(&mut self, value: &V) -> Result<(), Error> {
        let mut text = String::new();
        value.format(&mut text);
        if let Some(c) = text.chars().find(|c| !is_xml_char(*c) && !needs_ace(*c)) {
            return Err(self.error(format!("text contains U+{:04X}, which XML cannot represent", u32::from(c))));
        }
        match self.stack.last_mut() {
            Some(e) if e.children.is_empty() => {
                // An empty text is no content: `<a />`, unless the layout
                // says `<a></a>`.
                if !text.is_empty() {
                    match &mut e.text {
                        Some(t) => t.push_str(&text),
                        None => e.text = Some(text),
                    }
                    e.cdata = CDATA.contains(&e.name);
                }
                Ok(())
            }
            _ => Err(self.error("text after an element".to_owned())),
        }
    }

    /// `<name src="path"/>` here, and the part itself as a separate document.
    pub fn part<T: Root>(&mut self, name: &'static str, src: &str, content: &T) -> Result<(), Error> {
        self.elem(name, |w| w.fixed("src", src))?;
        let written = Writer::document(self.layout, src, T::ELEMENT, None, |w| content.write(w))?;
        self.parts.push((src.to_owned(), written.xml));
        self.parts.extend(written.parts);
        Ok(())
    }
}

/// Appends `e` to `out`, ordered as `layout` says. `extra` attributes are
/// written first (namespace declarations).
pub(crate) fn serialize<T: Tree>(out: &mut String, e: &T, extra: &[(&str, &str)], layout: Option<&ElementLayout>) -> Result<(), Error> {
    let mut path = Vec::new();
    write_element(out, e, extra, layout, &mut path)
}

fn write_element<'e, T: Tree>(
    out: &mut String,
    e: &'e T,
    extra: &[(&str, &str)],
    layout: Option<&ElementLayout>,
    path: &mut Vec<&'e str>,
) -> Result<(), Error> {
    if path.len() >= MAX_DEPTH {
        return Err(Error::new(ErrorKind::Package, "elements nested too deeply"));
    }
    let name = e.name();
    path.push(name);
    let error =
        |path: &[&str], message: String| Error::new(ErrorKind::Package, message).at(Location { element: path.join("/"), ..Location::default() });
    out.push('<');
    out.push_str(name);
    let mut attributes: Vec<(&str, &str)> = extra.iter().copied().chain(e.attributes()).collect();
    if let Some(l) = layout {
        l.order_attributes(&mut attributes);
    }
    for (name, value) in attributes {
        out.push(' ');
        out.push_str(name);
        out.push_str("=\"");
        escape_attr(value, out).map_err(|c| error(path, format!("attribute {name} contains U+{:04X}, which XML cannot represent", u32::from(c))))?;
        out.push('"');
    }
    let depth = path.len() - 1;
    let children = e.children();
    match (e.text(), children.is_empty()) {
        (None, true) if layout.is_some_and(|l| l.end_tag) => out.push('>'),
        (None, true) => {
            out.push_str(" />");
            path.pop();
            return Ok(());
        }
        (Some(text), true) => {
            out.push('>');
            let cdata = layout.and_then(|l| l.cdata).unwrap_or(e.cdata()) && !text.is_empty() && text.chars().all(|c| is_xml_char(c) && c != '\r');
            if cdata {
                write_cdata(text, out);
            } else {
                escape_text(text, out).map_err(|c| error(path, format!("text contains U+{:04X}, which XML cannot represent", u32::from(c))))?;
            }
        }
        (None, false) => {
            out.push('>');
            let order = match layout {
                Some(l) => l.order_children(&children.iter().map(Tree::name).collect::<Vec<_>>()),
                None => (0..children.len()).map(|i| (i, None)).collect(),
            };
            for (i, child_layout) in order {
                let Some(child) = children.get(i) else { continue };
                out.push('\n');
                indent(out, depth + 1);
                write_element(out, child, &[], child_layout, path)?;
            }
            out.push('\n');
            indent(out, depth);
        }
        (Some(_), false) => return Err(error(path, "element with both text and elements".to_owned())),
    }
    out.push_str("</");
    out.push_str(name);
    out.push('>');
    path.pop();
    Ok(())
}

fn indent(out: &mut String, depth: usize) {
    out.extend(std::iter::repeat_n('\t', depth));
}

/// XML 1.0 `Char` (surrogates cannot occur in a Rust string).
fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | ' '..='\u{FFFD}' | '\u{10000}'..)
}

/// Escapes an attribute value; tab, line feed and carriage return become
/// character references so that attribute normalization keeps them.
fn escape_attr(s: &str, out: &mut String) -> Result<(), char> {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#x9;"),
            '\n' => out.push_str("&#xa;"),
            '\r' => out.push_str("&#xd;"),
            c if is_xml_char(c) => out.push(c),
            c => return Err(c),
        }
    }
    Ok(())
}

/// Escapes text; a carriage return becomes a character reference (XML reads
/// a literal one as a line feed), other control characters `<?ACE n?>`.
fn escape_text(s: &str, out: &mut String) -> Result<(), char> {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#xd;"),
            c if needs_ace(c) => {
                let _ = write!(out, "<?ACE {:x}?>", u32::from(c));
            }
            c if is_xml_char(c) => out.push(c),
            c => return Err(c),
        }
    }
    Ok(())
}

/// Text as CDATA sections of at most [`CDATA_SECTION`] bytes. A `]]>` in
/// the text ends a section after its `]]`; the `>` is a section of its own.
fn write_cdata(text: &str, out: &mut String) {
    let mut pieces = text.split("]]>").peekable();
    while let Some(piece) = pieces.next() {
        if pieces.peek().is_some() {
            sections(&format!("{piece}]]"), out);
            sections(">", out);
        } else {
            sections(piece, out);
        }
    }
}

fn sections(mut rest: &str, out: &mut String) {
    while !rest.is_empty() {
        let mut n = CDATA_SECTION.min(rest.len());
        while !rest.is_char_boundary(n) {
            n -= 1;
        }
        let (section, tail) = rest.split_at(n);
        out.push_str("<![CDATA[");
        out.push_str(section);
        out.push_str("]]>");
        rest = tail;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(name: &str, attributes: &[(&str, &str)], children: Vec<Element>, text: Option<&str>) -> Element {
        Element {
            name: name.into(),
            attributes: attributes.iter().map(|(n, v)| ((*n).into(), (*v).into())).collect(),
            children,
            text: text.map(Into::into),
            cdata: name == "Contents",
        }
    }

    #[test]
    fn escapes() {
        let mut out = String::new();
        escape_attr("a\"<&>\t\n\r b", &mut out).unwrap();
        assert_eq!(out, "a&quot;&lt;&amp;&gt;&#x9;&#xa;&#xd; b");
        out.clear();
        escape_text("x\u{7}y\u{18}\r\n<", &mut out).unwrap();
        assert_eq!(out, "x<?ACE 7?>y<?ACE 18?>&#xd;\n&lt;");
        assert_eq!(escape_text("\u{FFFE}", &mut out), Err('\u{FFFE}'));
        assert_eq!(escape_attr("\u{1}", &mut out), Err('\u{1}'));
    }

    #[test]
    fn serializes_as_indesign_does() {
        let story = element(
            "Story",
            &[("Self", "u1")],
            vec![
                element("Content", &[], vec![], Some("a & b")),
                element("Br", &[], vec![], None),
                element("Content", &[], vec![], Some("")),
                element("Contents", &[], vec![], Some("x]]>y")),
            ],
            None,
        );
        let mut out = String::new();
        serialize(&mut out, &story, &[("xmlns:idPkg", PACKAGING_NS)], None).unwrap();
        let expected = format!(
            "<Story xmlns:idPkg=\"{PACKAGING_NS}\" Self=\"u1\">\n\t<Content>a &amp; b</Content>\n\t<Br />\n\t<Content></Content>\n\t\
             <Contents><![CDATA[x]]]]><![CDATA[>]]><![CDATA[y]]></Contents>\n</Story>"
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn splits_long_cdata_sections() {
        let mut out = String::new();
        write_cdata(&"a".repeat(CDATA_SECTION + 1), &mut out);
        assert_eq!(out.matches("<![CDATA[").count(), 2);
        assert!(out.ends_with("<![CDATA[a]]>"));
    }
}
