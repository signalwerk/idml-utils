//! Reading: a cursor over one element, driven by generated code.
//!
//! Attributes are looked up by name; child elements are consumed in order.
//! When the generated code is done with an element, every attribute, child
//! and text must have been consumed. Anything left over is an error in strict
//! mode and a warning in lenient mode.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use roxmltree::{Document, Node, NodeType};

use super::{Content, PACKAGING_NS, Root};
use crate::error::{Error, ErrorKind, Location, Warning};
use crate::value::{PartPath, Value};
use crate::{Mode, Part, ReadOptions};

/// Warnings kept in detail; later ones are only counted.
const MAX_WARNINGS: usize = 1000;

/// State shared by every reader of one read operation.
pub(crate) struct Ctx {
    pub options: ReadOptions,
    warnings: Vec<Warning>,
    suppressed: usize,
    /// The package's files, for designmap part references.
    pub files: BTreeMap<String, Vec<u8>>,
    /// The files read so far.
    pub used: BTreeSet<String>,
    /// The part being read, for locations.
    pub part: String,
}

impl Ctx {
    pub fn new(options: &ReadOptions, part: &str) -> Self {
        Ctx { options: options.clone(), warnings: Vec::new(), suppressed: 0, files: BTreeMap::new(), used: BTreeSet::new(), part: part.to_owned() }
    }

    fn full(&self) -> bool {
        self.warnings.len() >= MAX_WARNINGS
    }

    pub fn warn(&mut self, message: String, location: Location) {
        if self.full() {
            self.suppressed += 1;
        } else {
            self.warnings.push(Warning { message, location });
        }
    }

    /// The warnings, with a last one counting those not kept.
    pub fn into_warnings(mut self) -> Vec<Warning> {
        if self.suppressed > 0 {
            let message = format!("{} more warnings not listed", self.suppressed);
            self.warnings.push(Warning { message, location: Location::default() });
        }
        self.warnings
    }

    fn lenient(&self) -> bool {
        self.options.mode == Mode::Lenient
    }
}

/// Parses one part and reads its root element as `T`.
pub(crate) fn read_part<T: Content>(ctx: &mut Ctx, xml: &str, root: &str) -> Result<T, Error> {
    let part = ctx.part.clone();
    let location = || Location { part: part.clone(), ..Location::default() };
    let xml = xml.strip_prefix('\u{feff}').unwrap_or(xml);
    if nesting_exceeds(xml, ctx.options.max_depth) {
        let message = format!("elements nested deeper than {}", ctx.options.max_depth);
        return Err(Error::new(ErrorKind::Limit, message).at(location()));
    }
    let doc = Document::parse(xml).map_err(|e| Error::new(ErrorKind::Xml, e.to_string()).at(location()))?;
    match crate::upgrade::upgrade(ctx, &doc, xml) {
        Some(upgraded) => {
            let error = |e: roxmltree::Error| Error::new(ErrorKind::Xml, format!("upgraded from an older DOMVersion: {e}")).at(location());
            let doc = Document::parse(&upgraded).map_err(error)?;
            read_root(ctx, &doc, root)
        }
        None => read_root(ctx, &doc, root),
    }
}

fn read_root<T: Content>(ctx: &mut Ctx, doc: &Document, root: &str) -> Result<T, Error> {
    let mut r = Reader::new(ctx, doc, doc.root_element());
    if r.name(r.node) != root {
        return Err(r.error(format!("expected root element <{root}>, found <{}>", r.name(r.node))));
    }
    let value = T::read(&mut r)?;
    r.finish()?;
    Ok(value)
}

pub(crate) struct Reader<'c, 'a, 'i> {
    ctx: &'c mut Ctx,
    doc: &'a Document<'i>,
    node: Node<'a, 'i>,
    used: Vec<bool>,
    kids: Vec<(Node<'a, 'i>, Cow<'a, str>)>,
    next: usize,
    text_read: bool,
}

/// A way to read the content of an element with a choice.
pub(crate) type Alternative<'a, 'i, T> = fn(&mut Reader<'_, 'a, 'i>) -> Result<T, Error>;

/// Where a reader is, so a choice can try an alternative and go back.
struct Mark<'a, 'i> {
    used: Vec<bool>,
    kids: Vec<(Node<'a, 'i>, Cow<'a, str>)>,
    next: usize,
    text_read: bool,
    /// Warnings kept and suppressed.
    warnings: (usize, usize),
}

impl<'c, 'a, 'i> Reader<'c, 'a, 'i> {
    fn new(ctx: &'c mut Ctx, doc: &'a Document<'i>, node: Node<'a, 'i>) -> Self {
        let kids = node.children().filter(Node::is_element).map(|n| (n, qualified(n))).collect();
        Reader { ctx, doc, node, used: vec![false; node.attributes().len()], kids, next: 0, text_read: false }
    }

    fn name(&self, node: Node<'a, 'i>) -> Cow<'a, str> {
        qualified(node)
    }

    fn location(&self, node: Node<'a, 'i>) -> Location {
        let pos = self.doc.text_pos_at(node.range().start);
        let mut path = Vec::new();
        let mut n = Some(node);
        while let Some(e) = n.filter(Node::is_element) {
            let name = qualified(e);
            // `prev_siblings` starts at `e`, so this is the 1-based position.
            let position = e.prev_siblings().filter(|s| s.is_element() && qualified(*s) == name).count();
            path.push(if position <= 1 { name.into_owned() } else { format!("{name}[{position}]") });
            n = e.parent();
        }
        path.reverse();
        Location { part: self.ctx.part.clone(), line: pos.row, column: pos.col, element: path.join("/") }
    }

    pub fn error(&self, message: impl Into<String>) -> Error {
        Error::new(ErrorKind::Schema, message).at(self.location(self.node))
    }

    fn warn(&mut self, node: Node<'a, 'i>, message: String) {
        // Locating is linear in the number of siblings: skip it once full.
        let location = if self.ctx.full() { Location::default() } else { self.location(node) };
        self.ctx.warn(message, location);
    }

    /// An error in strict mode; a warning in lenient mode.
    fn reject(&mut self, node: Node<'a, 'i>, message: String) -> Result<(), Error> {
        if self.ctx.lenient() {
            self.warn(node, message);
            Ok(())
        } else {
            Err(Error::new(ErrorKind::Schema, message).at(self.location(node)))
        }
    }

    // -- attributes ----------------------------------------------------------

    fn find_attr(&mut self, name: &str) -> Option<&'a str> {
        let (i, a) = self.node.attributes().enumerate().find(|(_, a)| a.namespace().is_none() && a.name() == name)?;
        if let Some(u) = self.used.get_mut(i) {
            *u = true;
        }
        Some(a.value())
    }

    pub fn attr<V: Value>(&mut self, name: &str) -> Result<V, Error> {
        match self.opt_attr(name)? {
            Some(v) => Ok(v),
            None => Err(self.error(format!("missing attribute {name}"))),
        }
    }

    pub fn opt_attr<V: Value>(&mut self, name: &str) -> Result<Option<V>, Error> {
        let Some(text) = self.find_attr(name) else { return Ok(None) };
        match V::parse(text) {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                let message = format!("attribute {name}: {e}");
                // A lenient read drops an invalid optional value.
                self.reject(self.node, message).map(|()| None)
            }
        }
    }

    /// An attribute with a literal value; `token` compares whitespace-normalized.
    pub fn fixed(&mut self, name: &str, value: &str, token: bool) -> Result<(), Error> {
        let found = self.find_attr(name);
        let matches = found.is_some_and(|f| if token { normalize(f) == value } else { f == value });
        if matches {
            return Ok(());
        }
        let message = match found {
            Some(f) => format!("attribute {name} is {f:?}, expected {value:?}"),
            None => format!("missing attribute {name}={value:?}"),
        };
        // Older or newer InDesign versions write their own DOMVersion.
        if name == "DOMVersion" && self.ctx.lenient() {
            self.warn(self.node, message);
            return Ok(());
        }
        Err(self.error(message))
    }

    // -- text ----------------------------------------------------------------

    /// The element's text content. `<?ACE n?>` processing instructions in it
    /// stand for the characters U+0000–U+001F that XML cannot contain.
    pub fn text<V: Value>(&mut self) -> Result<V, Error> {
        self.text_read = true;
        let mut text = String::new();
        for n in self.node.children() {
            match n.node_type() {
                NodeType::Text => text.push_str(n.text().unwrap_or_default()),
                NodeType::PI => match n.pi().and_then(|pi| (pi.target == "ACE").then_some(pi.value)) {
                    Some(code) => match code.and_then(ace_char) {
                        Some(c) => text.push(c),
                        None => self.reject(n, format!("invalid ACE code {:?}", code.unwrap_or_default()))?,
                    },
                    None => self.warn(n, "processing instruction ignored".to_owned()),
                },
                NodeType::Element => return Err(self.error("element content where text is expected")),
                NodeType::Comment | NodeType::Root => {}
            }
        }
        V::parse(&text).map_err(|e| self.error(e.to_string()))
    }

    // -- children ------------------------------------------------------------

    /// The qualified name of the next child element.
    pub fn peek(&self) -> Option<&str> {
        self.kids.get(self.next).map(|(_, name)| name.as_ref())
    }

    /// Drops child elements that this content can never contain: an error in
    /// strict mode, skipped with a warning in lenient mode.
    pub fn known(&mut self, names: &[&str]) -> Result<(), Error> {
        let mut kept = Vec::with_capacity(self.kids.len());
        for (node, name) in std::mem::take(&mut self.kids) {
            if names.contains(&name.as_ref()) {
                kept.push((node, name));
            } else {
                self.reject(node, format!("unexpected element <{name}>"))?;
            }
        }
        self.kids = kept;
        Ok(())
    }

    /// Reads the next child, which must be `name`.
    pub fn elem<T>(&mut self, name: &str, f: impl FnOnce(&mut Reader<'_, 'a, 'i>) -> Result<T, Error>) -> Result<T, Error> {
        let Some((node, found)) = self.kids.get(self.next).cloned() else {
            return Err(self.error(format!("missing element <{name}>")));
        };
        if found != name {
            return Err(Error::new(ErrorKind::Schema, format!("expected <{name}>, found <{found}>")).at(self.location(node)));
        }
        self.next += 1;
        let mut child = Reader::new(self.ctx, self.doc, node);
        let value = f(&mut child)?;
        child.finish()?;
        Ok(value)
    }

    pub fn opt_elem<T>(&mut self, name: &str, f: impl FnOnce(&mut Reader<'_, 'a, 'i>) -> Result<T, Error>) -> Result<Option<T>, Error> {
        if self.peek() == Some(name) { self.elem(name, f).map(Some) } else { Ok(None) }
    }

    pub fn elems<T>(&mut self, name: &str, mut f: impl FnMut(&mut Reader<'_, 'a, 'i>) -> Result<T, Error>) -> Result<Vec<T>, Error> {
        let mut out = Vec::new();
        while self.peek() == Some(name) {
            out.push(self.elem(name, &mut f)?);
        }
        Ok(out)
    }

    pub fn array<T, const N: usize>(&mut self, name: &str, mut f: impl FnMut(&mut Reader<'_, 'a, 'i>) -> Result<T, Error>) -> Result<[T; N], Error> {
        let mut out = Vec::with_capacity(N);
        for _ in 0..N {
            out.push(self.elem(name, &mut f)?);
        }
        out.try_into().map_err(|_| self.error(format!("expected {N} <{name}> elements")))
    }

    // In-place reading of generated content types (see `Content`).

    /// Reads the next child, which must be `name`, into `slot`.
    pub fn elem_into<T: Content>(&mut self, name: &str, slot: &mut T) -> Result<(), Error> {
        self.elem(name, |r| T::read_into(r, slot))
    }

    pub fn opt_elem_into<T: Content>(&mut self, name: &str, slot: &mut Option<T>) -> Result<(), Error> {
        if self.peek() != Some(name) {
            return Ok(());
        }
        let value = slot.get_or_insert_with(T::placeholder);
        self.elem(name, |r| T::read_into(r, value))
    }

    pub fn elems_into<T: Content>(&mut self, name: &str, out: &mut Vec<T>) -> Result<(), Error> {
        while self.peek() == Some(name) {
            self.push_into(name, out)?;
        }
        Ok(())
    }

    /// Reads the next child, which must be `name`, as a new item of `out`.
    pub fn push_into<T: Content>(&mut self, name: &str, out: &mut Vec<T>) -> Result<(), Error> {
        out.resize_with(out.len() + 1, T::placeholder);
        let Some(value) = out.last_mut() else { return Err(self.error("empty list")) };
        self.elem(name, |r| T::read_into(r, value))
    }

    /// Reads the next child, which must be `name`, into a new box.
    pub fn boxed<T: Content>(&mut self, name: &str) -> Result<Box<T>, Error> {
        let mut value = new_box::<T>();
        self.elem(name, |r| T::read_into(r, &mut value))?;
        Ok(value)
    }

    /// An interleaved element that may occur at most once.
    pub fn once_into<T: Content>(&mut self, name: &str, slot: &mut Option<T>) -> Result<(), Error> {
        if slot.is_some() {
            return self.duplicate(name);
        }
        let value = slot.get_or_insert_with(T::placeholder);
        self.elem(name, |r| T::read_into(r, value))
    }

    /// An interleaved element that must occur exactly once; `seen` records it.
    pub fn once_required_into<T: Content>(&mut self, name: &str, slot: &mut T, seen: &mut bool) -> Result<(), Error> {
        if *seen {
            return self.duplicate(name);
        }
        *seen = true;
        self.elem(name, |r| T::read_into(r, slot))
    }

    pub fn require(&self, seen: bool, name: &str) -> Result<(), Error> {
        if seen { Ok(()) } else { Err(self.error(format!("missing element <{name}>"))) }
    }

    /// A second occurrence of an element allowed once: an error, or in
    /// lenient mode skipped with a warning.
    fn duplicate(&mut self, name: &str) -> Result<(), Error> {
        let Some((node, _)) = self.kids.get(self.next).cloned() else { return Ok(()) };
        self.reject(node, format!("<{name}> occurs more than once"))?;
        self.next += 1;
        Ok(())
    }

    /// Reads an interleaved element that may occur at most once.
    pub fn once<T>(&mut self, name: &str, slot: &mut Option<T>, f: impl FnOnce(&mut Reader<'_, 'a, 'i>) -> Result<T, Error>) -> Result<(), Error> {
        let node = self.kids.get(self.next).map(|(n, _)| *n);
        let value = self.elem(name, f)?;
        if slot.is_some() {
            let message = format!("<{name}> occurs more than once");
            if let Some(node) = node {
                self.reject(node, message)?;
            }
            return Ok(());
        }
        *slot = Some(value);
        Ok(())
    }

    /// A required interleaved element after the interleave was read.
    pub fn required<T>(&self, value: Option<T>, name: &str) -> Result<T, Error> {
        value.ok_or_else(|| self.error(format!("missing element <{name}>")))
    }

    /// Tries each alternative and takes the first that matches the whole
    /// content; in lenient mode, failing that, the first that matches.
    pub fn choice<T>(&mut self, what: &str, alternatives: &[Alternative<'a, 'i, T>]) -> Result<T, Error> {
        let start = self.mark();
        let mut partial: Option<(T, Mark<'a, 'i>)> = None;
        let mut first_error = None;
        for f in alternatives {
            match f(self) {
                Ok(v) if self.complete() => return Ok(v),
                Ok(v) => {
                    if partial.is_none() {
                        partial = Some((v, self.mark()));
                    }
                }
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
            self.reset(&start);
        }
        match partial {
            Some((v, mark)) if self.ctx.lenient() => {
                self.reset(&mark);
                Ok(v)
            }
            _ => {
                let detail = first_error.map(|e| format!(": {}", e.message())).unwrap_or_default();
                Err(self.error(format!("content matches no alternative of {what}{detail}")))
            }
        }
    }

    fn mark(&self) -> Mark<'a, 'i> {
        Mark {
            used: self.used.clone(),
            kids: self.kids.clone(),
            next: self.next,
            text_read: self.text_read,
            warnings: (self.ctx.warnings.len(), self.ctx.suppressed),
        }
    }

    fn reset(&mut self, mark: &Mark<'a, 'i>) {
        self.used.clone_from(&mark.used);
        self.kids.clone_from(&mark.kids);
        self.next = mark.next;
        self.text_read = mark.text_read;
        self.ctx.warnings.truncate(mark.warnings.0);
        self.ctx.suppressed = mark.warnings.1;
    }

    fn complete(&self) -> bool {
        self.used.iter().all(|u| *u) && self.next == self.kids.len() && (self.text_read || !self.has_text())
    }

    fn has_text(&self) -> bool {
        self.node.children().any(|n| n.is_text() && !n.text().unwrap_or_default().trim_matches([' ', '\t', '\n', '\r']).is_empty())
    }

    /// Checks that everything was consumed.
    fn finish(&mut self) -> Result<(), Error> {
        let unused: Vec<String> = self
            .node
            .attributes()
            .zip(&self.used)
            .filter(|(_, used)| !**used)
            .map(|(a, _)| match a.namespace() {
                Some(ns) => format!("{{{ns}}}{}", a.name()),
                None => a.name().to_owned(),
            })
            .collect();
        for name in unused {
            self.reject(self.node, format!("unexpected attribute {name}"))?;
        }
        for (node, name) in self.kids.get(self.next..).unwrap_or_default().to_vec() {
            self.reject(node, format!("unexpected element <{name}>"))?;
        }
        if !self.text_read && self.has_text() {
            self.reject(self.node, "unexpected text".to_owned())?;
        }
        Ok(())
    }

    // -- package parts -------------------------------------------------------

    /// `<idPkg:… src="path"/>`: reads the referenced part.
    pub fn part<T: Root>(&mut self) -> Result<Part<T>, Error> {
        let src: PartPath = self.attr("src")?;
        let content = self.load(src.as_str())?;
        Ok(Part { src, content })
    }

    /// A part reference whose path the schema fixes.
    pub fn fixed_part<T: Root>(&mut self, src: &str) -> Result<T, Error> {
        self.fixed("src", src, true)?;
        self.load(src)
    }

    fn load<T: Root>(&mut self, path: &str) -> Result<T, Error> {
        if !self.ctx.used.insert(path.to_owned()) {
            return Err(Error::new(ErrorKind::Package, format!("{path} is referenced twice")).at(self.location(self.node)));
        }
        let Some(bytes) = self.ctx.files.get(path) else {
            return Err(Error::new(ErrorKind::Package, format!("missing part {path}")).at(self.location(self.node)));
        };
        let xml = String::from_utf8(bytes.clone())
            .map_err(|_| Error::new(ErrorKind::Xml, "not UTF-8").at(Location { part: path.to_owned(), ..Location::default() }))?;
        let outer = std::mem::replace(&mut self.ctx.part, path.to_owned());
        let result = read_part(self.ctx, &xml, T::ELEMENT);
        self.ctx.part = outer;
        result
    }
}

/// Whether elements in `xml` nest deeper than `max`. Checked before parsing
/// because the parser recurses once per level: a hostile document could
/// otherwise exhaust the stack.
pub(crate) fn nesting_exceeds(xml: &str, max: usize) -> bool {
    let b = xml.as_bytes();
    let find = |from: usize, pat: &[u8]| b.get(from..).and_then(|rest| rest.windows(pat.len()).position(|w| w == pat)).map(|p| from + p);
    let mut depth = 0usize;
    let mut i = 0;
    while let Some(start) = b.get(i..).and_then(|rest| rest.iter().position(|&c| c == b'<')).map(|p| i + p) {
        let rest = b.get(start..).unwrap_or_default();
        let end = if rest.starts_with(b"<!--") {
            find(start, b"-->").map(|e| e + 3)
        } else if rest.starts_with(b"<![CDATA[") {
            find(start, b"]]>").map(|e| e + 3)
        } else if rest.starts_with(b"<?") {
            find(start, b"?>").map(|e| e + 2)
        } else if rest.starts_with(b"<!") || rest.starts_with(b"</") {
            if rest.starts_with(b"</") {
                depth = depth.saturating_sub(1);
            }
            find(start, b">").map(|e| e + 1)
        } else {
            // A start tag: find its end outside quoted attribute values.
            let mut quote = None;
            let mut end = None;
            for (j, &c) in rest.iter().enumerate().skip(1) {
                match (quote, c) {
                    (Some(q), c) if c == q => quote = None,
                    (Some(_), _) => {}
                    (None, b'"' | b'\'') => quote = Some(c),
                    (None, b'>') => {
                        end = Some(start + j + 1);
                        if rest.get(j - 1) != Some(&b'/') {
                            depth += 1;
                        }
                        break;
                    }
                    (None, _) => {}
                }
            }
            end
        };
        if depth > max {
            return true;
        }
        // Malformed input: the parser reports it.
        let Some(end) = end else { return false };
        i = end;
    }
    false
}

/// A boxed placeholder. Kept out of line so that the placeholder's stack
/// temporary is not part of the caller's frame.
#[inline(never)]
fn new_box<T: Content>() -> Box<T> {
    Box::new(T::placeholder())
}

/// `prefix:local` for the packaging namespace, `local` without namespace,
/// `{ns}local` otherwise.
pub(crate) fn qualified<'a>(node: Node<'a, '_>) -> Cow<'a, str> {
    let name = node.tag_name();
    match name.namespace() {
        None => Cow::Borrowed(name.name()),
        Some(PACKAGING_NS) => Cow::Owned(format!("idPkg:{}", name.name())),
        Some(ns) => Cow::Owned(format!("{{{ns}}}{}", name.name())),
    }
}

/// The `token` whitespace rule.
fn normalize(text: &str) -> String {
    text.split([' ', '\t', '\n', '\r']).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")
}

/// The character of `<?ACE n?>`: `n` is hexadecimal and names a character
/// that XML 1.0 cannot contain.
fn ace_char(code: &str) -> Option<char> {
    let c = u32::from_str_radix(code.trim(), 16).ok().and_then(char::from_u32)?;
    super::needs_ace(c).then_some(c)
}

#[cfg(test)]
mod tests {
    use super::nesting_exceeds;

    #[test]
    fn measures_nesting() {
        let xml = r#"<?xml version="1.0"?><!-- <a><a> --><a x="/>" y='>'><b/><c><![CDATA[<d><d>]]><?pi <e>?></c></a>"#;
        assert!(!nesting_exceeds(xml, 2));
        assert!(nesting_exceeds(xml, 1));
        assert!(nesting_exceeds(&"<a>".repeat(100_000), 64));
    }
}
