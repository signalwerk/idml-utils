//! Untyped XML elements, for producers that build parts by element and
//! attribute name.

/// An XML element of a package part: a name, attributes in order, and either
/// child elements or text (IDML has no mixed content).
///
/// [`Package::from_elements`](crate::Package::from_elements) builds authoritative
/// arena nodes directly, preserving order and explicit versions. Validate the
/// assembled package or use `Authoring` for exact-target schema checks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Element {
    /// The qualified name, such as `Story` or `idPkg:Spread`.
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub children: Vec<Element>,
    /// Text content; `Some("")` is written with an end tag (`<a></a>`).
    /// Characters XML cannot contain, such as U+0018, are written as
    /// `<?ACE n?>` processing instructions.
    pub text: Option<String>,
    /// Whether the text is written as CDATA sections.
    pub cdata: bool,
}

impl Element {
    pub fn new(name: impl Into<String>) -> Self {
        Element { name: name.into(), ..Element::default() }
    }

    /// The value of attribute `name`.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attributes.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    /// Sets attribute `name`. An attribute the element already has keeps its
    /// position.
    pub fn set_attr(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let value = value.into();
        match self.attributes.iter_mut().find(|(n, _)| *n == name) {
            Some(a) => a.1 = value,
            None => self.attributes.push((name, value)),
        }
    }

    /// Removes attribute `name`.
    pub fn remove_attr(&mut self, name: &str) {
        self.attributes.retain(|(n, _)| n != name);
    }

    /// The element as XML text, written as in a package part (without the
    /// XML declaration).
    pub fn to_xml(&self) -> Result<String, crate::Error> {
        let mut out = String::new();
        crate::xml::serialize(&mut out, self, &[], None)?;
        Ok(out)
    }

    /// Every element of this subtree, this one first, in document order.
    pub fn descendants(&self) -> impl Iterator<Item = &Element> {
        let mut stack = vec![self];
        std::iter::from_fn(move || {
            let e = stack.pop()?;
            stack.extend(e.children.iter().rev());
            Some(e)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attributes_keep_their_position() {
        let mut e = Element::new("a");
        e.set_attr("x", "1");
        e.set_attr("y", "2");
        e.set_attr("x", "3");
        assert_eq!(e.attributes, [("x".into(), "3".into()), ("y".into(), "2".into())]);
        assert_eq!(e.attr("y"), Some("2"));
        e.remove_attr("x");
        assert_eq!(e.attr("x"), None);
    }

    #[test]
    fn descendants_are_in_document_order() {
        let mut a = Element::new("a");
        let mut b = Element::new("b");
        b.children.push(Element::new("c"));
        a.children.push(b);
        a.children.push(Element::new("d"));
        let names: Vec<&str> = a.descendants().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c", "d"]);
        assert_eq!(a.to_xml().unwrap(), "<a>\n\t<b>\n\t\t<c />\n\t</b>\n\t<d />\n</a>");
    }
}
