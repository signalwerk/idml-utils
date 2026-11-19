//! Borrowed typed views. Every operation addresses the authoritative node ID.
use crate::{
    Element, Error, ErrorKind,
    profile::{CompatibilityProfile, SchemaCatalog, Support},
    tree::{Field, NodeId, XmlTree},
    value::Value,
};
use std::marker::PhantomData;

pub trait ElementType {
    const NAMESPACE: &'static str;
    const LOCAL: &'static str;
    fn parse_attribute(name: &str, text: &str) -> Result<(), crate::ValueError>;
}
#[derive(Debug)]
pub struct View<'a, T: ElementType> {
    tree: &'a XmlTree,
    id: NodeId,
    marker: PhantomData<T>,
}
impl<T: ElementType> Copy for View<'_, T> {}
impl<T: ElementType> Clone for View<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<'a, T: ElementType> View<'a, T> {
    pub fn new(tree: &'a XmlTree, id: NodeId) -> Option<Self> {
        tree.name(id).filter(|n| n.namespace == T::NAMESPACE && n.local == T::LOCAL).map(|_| Self { tree, id, marker: PhantomData })
    }
    pub fn node_id(&self) -> NodeId {
        self.id
    }
    pub fn xml_tree(&self) -> &'a XmlTree {
        self.tree
    }
    pub fn attribute<V: Value>(&self, name: &str) -> Field<'a, V> {
        self.tree.field(self.id, name)
    }
    pub fn text<V: Value>(&self) -> Field<'a, V> {
        match self.tree.text(self.id) {
            Ok(raw) => match V::parse(&raw) {
                Ok(v) => Field::Known(v),
                Err(e) => Field::Unrecognized { raw: std::borrow::Cow::Owned(raw), reason: e.to_string() },
            },
            Err(e) => Field::Unrecognized { raw: std::borrow::Cow::Owned(self.tree.fragment(self.id).unwrap_or_default()), reason: e.to_string() },
        }
    }
    pub fn children<U: ElementType>(&self) -> impl Iterator<Item = View<'a, U>> + 'a {
        let tree = self.tree;
        let id = self.id;
        tree.elements(id).filter_map(move |id| View::new(tree, id))
    }
    pub fn descendants<U: ElementType>(&self) -> impl Iterator<Item = View<'a, U>> + 'a {
        let tree = self.tree;
        let id = self.id;
        tree.descendants(id).filter_map(move |id| View::new(tree, id))
    }
}
#[derive(Debug)]
pub struct ViewMut<'a, T: ElementType> {
    tree: &'a mut XmlTree,
    id: NodeId,
    marker: PhantomData<T>,
}
impl<'a, T: ElementType> ViewMut<'a, T> {
    pub fn new(tree: &'a mut XmlTree, id: NodeId) -> Option<Self> {
        if tree.name(id).is_some_and(|n| n.namespace == T::NAMESPACE && n.local == T::LOCAL) {
            Some(Self { tree, id, marker: PhantomData })
        } else {
            None
        }
    }
    pub fn as_view(&self) -> View<'_, T> {
        View { tree: self.tree, id: self.id, marker: PhantomData }
    }
    pub fn set_attribute<V: Value>(&mut self, name: &str, value: &V) -> Result<(), Error> {
        let mut text = String::new();
        value.format(&mut text);
        T::parse_attribute(name, &text).map_err(|e| Error::new(ErrorKind::Schema, e.to_string()))?;
        self.tree.set_attribute(self.id, name, &text)
    }
    pub fn set_checked<V: Value>(&mut self, catalog: &SchemaCatalog, profile: &CompatibilityProfile, name: &str, value: &V) -> Result<(), Error> {
        let mut text = String::new();
        value.format(&mut text);
        if self.tree.dom_version().and_then(|v| v.parse().ok()) != Some(profile.snapshot.dom_version) {
            return Err(Error::new(ErrorKind::Schema, "setter profile does not match the document version"));
        }
        if catalog.attribute_support(profile, self.tree, self.id, name, &text) != Support::Supported {
            return Err(Error::new(ErrorKind::Schema, "attribute/value is unsupported or unknown in this profile"));
        }
        T::parse_attribute(name, &text).map_err(|e| Error::new(ErrorKind::Schema, e.to_string()))?;
        self.tree.set_attribute(self.id, name, &text)
    }
    pub fn remove_attribute(&mut self, name: &str) -> Result<(), Error> {
        self.tree.remove_attribute(self.id, name)
    }
    pub fn set_text(&mut self, text: &str) -> Result<(), Error> {
        self.tree.set_text(self.id, text)
    }
}
#[derive(Debug, Clone)]
pub struct Builder<T: ElementType> {
    element: Element,
    marker: PhantomData<T>,
}
impl<T: ElementType> Default for Builder<T> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T: ElementType> Builder<T> {
    pub fn new() -> Self {
        let mut element = Element::new(if T::NAMESPACE == crate::xml::PACKAGING_NS { format!("idPkg:{}", T::LOCAL) } else { T::LOCAL.into() });
        if !T::NAMESPACE.is_empty() {
            element.set_attr(if T::NAMESPACE == crate::xml::PACKAGING_NS { "xmlns:idPkg" } else { "xmlns" }, T::NAMESPACE);
        }
        Self { element, marker: PhantomData }
    }
    /// Named setters check the shared vocabulary. Validate the assembled part
    /// against an exact profile to check required data and version capabilities.
    pub fn set_attribute(&mut self, name: &str, text: &str) -> Result<(), Error> {
        T::parse_attribute(name, text).map_err(|e| Error::new(ErrorKind::Schema, e.to_string()))?;
        self.element.set_attr(name, text);
        Ok(())
    }
    pub fn assign_value<V: Value>(&mut self, name: &str, value: &V) -> Result<(), Error> {
        let mut text = String::new();
        value.format(&mut text);
        self.set_attribute(name, &text)
    }
    /// Explicit extension data is retained without pretending it is understood.
    pub fn set_raw(&mut self, name: &str, text: &str) {
        self.element.set_attr(name, text);
    }
    pub fn push(&mut self, child: Element) {
        self.element.children.push(child);
    }
    pub fn text(&mut self, text: impl Into<String>) {
        self.element.text = Some(text.into());
    }
    pub fn build(self) -> Element {
        self.element
    }
}
impl XmlTree {
    pub fn view<T: ElementType>(&self, id: NodeId) -> Option<View<'_, T>> {
        View::new(self, id)
    }
    pub fn view_mut<T: ElementType>(&mut self, id: NodeId) -> Option<ViewMut<'_, T>> {
        ViewMut::new(self, id)
    }
    pub fn views<T: ElementType>(&self) -> impl Iterator<Item = View<'_, T>> {
        self.descendants(self.root()).filter_map(|id| View::new(self, id))
    }
}
