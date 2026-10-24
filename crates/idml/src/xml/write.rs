//! Writing: an XML serializer driven by generated code.
//!
//! Elements with element content are indented with tabs, as InDesign writes
//! them; elements with text content are written without added whitespace.
//! IDML has no mixed content, so indentation never changes a value.

use std::fmt::Write as _;

use super::{PACKAGING_NS, Root, needs_ace};
use crate::error::{Error, ErrorKind, Location};
use crate::value::Value;

/// Guards the stack against absurdly deep models.
const MAX_DEPTH: usize = 512;

pub(crate) struct Writer {
    out: String,
    part: String,
    stack: Vec<Open>,
    scratch: String,
    /// The parts referenced from this one, as (path, XML).
    parts: Vec<(String, String)>,
}

struct Open {
    name: &'static str,
    start_open: bool,
    elements: bool,
}

/// A serialized part and the parts it references.
pub(crate) struct Written {
    pub xml: String,
    pub parts: Vec<(String, String)>,
}

impl Writer {
    /// Writes a complete XML document with root element `root`, preceded by
    /// the processing instruction `pi` if given.
    pub fn document(part: &str, root: &'static str, pi: Option<&str>, f: impl FnOnce(&mut Writer) -> Result<(), Error>) -> Result<Written, Error> {
        let mut w = Writer { out: String::new(), part: part.to_owned(), stack: Vec::new(), scratch: String::new(), parts: Vec::new() };
        w.out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
        if let Some(pi) = pi {
            let _ = writeln!(w.out, "<?{pi}?>");
        }
        w.elem(root, |w| {
            w.fixed("xmlns:idPkg", PACKAGING_NS)?;
            f(w)
        })?;
        w.out.push('\n');
        Ok(Written { xml: w.out, parts: w.parts })
    }

    fn error(&self, message: String) -> Error {
        let element = self.stack.iter().map(|o| o.name).collect::<Vec<_>>().join("/");
        Error::new(ErrorKind::Package, message).at(Location { part: self.part.clone(), element, ..Location::default() })
    }

    pub fn elem(&mut self, name: &'static str, f: impl FnOnce(&mut Writer) -> Result<(), Error>) -> Result<(), Error> {
        let depth = self.stack.len();
        if depth >= MAX_DEPTH {
            return Err(self.error("elements nested too deeply".to_owned()));
        }
        if let Some(parent) = self.stack.last_mut() {
            if parent.start_open {
                self.out.push('>');
                parent.start_open = false;
            }
            parent.elements = true;
            self.out.push('\n');
            indent(&mut self.out, depth);
        }
        self.out.push('<');
        self.out.push_str(name);
        self.stack.push(Open { name, start_open: true, elements: false });
        f(self)?;
        let open = self.stack.pop().ok_or_else(|| self.error("unbalanced element".to_owned()))?;
        if open.start_open {
            self.out.push_str("/>");
        } else {
            if open.elements {
                self.out.push('\n');
                indent(&mut self.out, depth);
            }
            self.out.push_str("</");
            self.out.push_str(name);
            self.out.push('>');
        }
        Ok(())
    }

    fn start_tag(&self) -> Result<(), Error> {
        match self.stack.last() {
            Some(open) if open.start_open => Ok(()),
            _ => Err(self.error("attribute after content".to_owned())),
        }
    }

    fn put_attr(&mut self, name: &str, value: &str) -> Result<(), Error> {
        self.start_tag()?;
        self.out.push(' ');
        self.out.push_str(name);
        self.out.push_str("=\"");
        if let Err(c) = escape_attr(value, &mut self.out) {
            return Err(self.error(format!("attribute {name} contains U+{:04X}, which XML cannot represent", u32::from(c))));
        }
        self.out.push('"');
        Ok(())
    }

    pub fn attr<V: Value>(&mut self, name: &str, value: &V) -> Result<(), Error> {
        let mut text = std::mem::take(&mut self.scratch);
        text.clear();
        value.format(&mut text);
        let result = self.put_attr(name, &text);
        self.scratch = text;
        result
    }

    pub fn opt_attr<V: Value>(&mut self, name: &str, value: &Option<V>) -> Result<(), Error> {
        match value {
            Some(v) => self.attr(name, v),
            None => Ok(()),
        }
    }

    pub fn fixed(&mut self, name: &str, value: &str) -> Result<(), Error> {
        self.put_attr(name, value)
    }

    pub fn text<V: Value>(&mut self, value: &V) -> Result<(), Error> {
        let mut text = std::mem::take(&mut self.scratch);
        text.clear();
        value.format(&mut text);
        if let Some(open) = self.stack.last_mut()
            && open.start_open
        {
            self.out.push('>');
            open.start_open = false;
        }
        let result =
            escape_text(&text, &mut self.out).map_err(|c| self.error(format!("text contains U+{:04X}, which XML cannot represent", u32::from(c))));
        self.scratch = text;
        result
    }

    /// `<name src="path"/>` here, and the part itself as a separate document.
    pub fn part<T: Root>(&mut self, name: &'static str, src: &str, content: &T) -> Result<(), Error> {
        self.elem(name, |w| w.fixed("src", src))?;
        let written = Writer::document(src, T::ELEMENT, None, |w| content.write(w))?;
        self.parts.push((src.to_owned(), written.xml));
        self.parts.extend(written.parts);
        Ok(())
    }
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
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c if is_xml_char(c) => out.push(c),
            c => return Err(c),
        }
    }
    Ok(())
}

/// Escapes text; control characters become `<?ACE n?>`.
fn escape_text(s: &str, out: &mut String) -> Result<(), char> {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            c if needs_ace(c) => {
                let _ = write!(out, "<?ACE {:x}?>", u32::from(c));
            }
            c if is_xml_char(c) => out.push(c),
            c => return Err(c),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes() {
        let mut out = String::new();
        escape_attr("a\"<&>\t\n\r b", &mut out).unwrap();
        assert_eq!(out, "a&quot;&lt;&amp;&gt;&#9;&#10;&#13; b");
        out.clear();
        escape_text("x\u{7}y\u{18}\r\n<", &mut out).unwrap();
        assert_eq!(out, "x<?ACE 7?>y<?ACE 18?>&#13;\n&lt;");
        assert_eq!(escape_text("\u{FFFE}", &mut out), Err('\u{FFFE}'));
        assert_eq!(escape_attr("\u{1}", &mut out), Err('\u{1}'));
    }
}
