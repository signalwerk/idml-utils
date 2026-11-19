//! One cursor interface for XML-parser nodes and producer/arena nodes.
use crate::tree::{Attribute, NodeId, NodeKind, XmlTree};
use roxmltree::{Node, NodeType};
use std::borrow::Cow;

#[derive(Clone, Copy)]
pub(super) enum InputNode<'a, 'i> {
    Xml(Node<'a, 'i>),
    Tree(&'a XmlTree, NodeId),
}
#[derive(Clone, Copy)]
pub(super) enum InputAttribute<'a, 'i> {
    Xml(roxmltree::Attribute<'a, 'i>),
    Tree(&'a Attribute),
}
pub(super) struct Instruction<'a> {
    pub target: &'a str,
    pub value: Option<&'a str>,
}
impl<'a, 'i> InputAttribute<'a, 'i> {
    pub fn name(self) -> &'a str {
        match self {
            Self::Xml(a) => a.name(),
            Self::Tree(a) => &a.name.local,
        }
    }
    pub fn namespace(self) -> Option<&'a str> {
        match self {
            Self::Xml(a) => a.namespace(),
            Self::Tree(a) => (!a.name.namespace.is_empty()).then_some(a.name.namespace.as_str()),
        }
    }
    pub fn value(self) -> &'a str {
        match self {
            Self::Xml(a) => a.value(),
            Self::Tree(a) => &a.value,
        }
    }
}
impl<'a, 'i> InputNode<'a, 'i> {
    pub fn qualified(self) -> Cow<'a, str> {
        match self {
            Self::Xml(n) => super::qualified(n),
            Self::Tree(t, id) => match t.name(id) {
                Some(n) if n.namespace.is_empty() => Cow::Borrowed(&n.local),
                Some(n) if n.namespace == super::PACKAGING_NS => Cow::Owned(format!("idPkg:{}", n.local)),
                Some(n) => Cow::Owned(format!("{{{}}}{}", n.namespace, n.local)),
                None => Cow::Borrowed(""),
            },
        }
    }
    pub fn children(self) -> impl Iterator<Item = Self> {
        let xml = match self {
            Self::Xml(n) => Some(n.children()),
            _ => None,
        };
        let tree = match self {
            Self::Tree(t, id) => Some((t, t.children(id))),
            _ => None,
        };
        xml.into_iter().flatten().map(Self::Xml).chain(tree.into_iter().flat_map(|(t, ids)| ids.iter().map(move |id| Self::Tree(t, *id))))
    }
    pub fn attributes(self) -> impl Iterator<Item = InputAttribute<'a, 'i>> {
        let xml = match self {
            Self::Xml(n) => Some(n.attributes()),
            _ => None,
        };
        let tree = match self {
            Self::Tree(t, id) => Some(t.attributes(id)),
            _ => None,
        };
        xml.into_iter().flatten().map(InputAttribute::Xml).chain(tree.into_iter().flatten().map(InputAttribute::Tree))
    }
    pub fn is_element(&self) -> bool {
        self.node_type() == NodeType::Element
    }
    pub fn is_text(&self) -> bool {
        self.node_type() == NodeType::Text
    }
    pub fn node_type(self) -> NodeType {
        match self {
            Self::Xml(n) => n.node_type(),
            Self::Tree(t, id) => match t.kind(id) {
                Ok(NodeKind::Element(_)) => NodeType::Element,
                Ok(NodeKind::Text(_)) => NodeType::Text,
                Ok(NodeKind::Instruction { .. }) => NodeType::PI,
                Ok(NodeKind::Document) => NodeType::Root,
                _ => NodeType::Comment,
            },
        }
    }
    pub fn text(self) -> Option<&'a str> {
        match self {
            Self::Xml(n) => n.text(),
            Self::Tree(t, id) => match t.kind(id).ok()? {
                NodeKind::Text(t) | NodeKind::Comment(t) => Some(t),
                _ => None,
            },
        }
    }
    pub fn pi(self) -> Option<Instruction<'a>> {
        match self {
            Self::Xml(n) => n.pi().map(|p| Instruction { target: p.target, value: p.value }),
            Self::Tree(t, id) => match t.kind(id).ok()? {
                NodeKind::Instruction { target, value } => Some(Instruction { target, value: value.as_deref() }),
                _ => None,
            },
        }
    }
    pub fn parent(self) -> Option<Self> {
        match self {
            Self::Xml(n) => n.parent().map(Self::Xml),
            Self::Tree(t, id) => t.parent(id).map(|id| Self::Tree(t, id)),
        }
    }
    pub fn prev_siblings(self) -> std::vec::IntoIter<Self> {
        match self {
            Self::Xml(n) => n.prev_siblings().map(Self::Xml).collect::<Vec<_>>(),
            Self::Tree(t, id) => t
                .parent(id)
                .map(|p| {
                    let mut v = Vec::new();
                    for &n in t.children(p) {
                        v.push(Self::Tree(t, n));
                        if n == id {
                            break;
                        }
                    }
                    v
                })
                .unwrap_or_else(|| vec![self]),
        }
        .into_iter()
    }
    pub fn range(self) -> std::ops::Range<usize> {
        match self {
            Self::Xml(n) => n.range(),
            Self::Tree(_, _) => 0..0,
        }
    }
}
