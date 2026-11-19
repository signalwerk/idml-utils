//! How a package is written, apart from its content: the order of entries,
//! attributes and child elements, compression, and the few places where
//! InDesign's own output varies.
//!
//! `OwnedPackage::read` (feature `owned`) records the layout of what it
//! reads, and writing follows it, so a package that is read and written again
//! keeps its bytes where its content is unchanged. A new package has an empty
//! layout and is written as InDesign writes packages.

use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;

#[cfg(feature = "owned")]
use roxmltree::Node;

#[cfg(feature = "owned")]
use crate::xml::qualified;

/// How a package is written (see the module documentation).
///
/// Layouts follow elements by name and position: the layout of the third
/// `<CharacterStyleRange>` of a `<ParagraphStyleRange>` applies to whatever
/// is its third `<CharacterStyleRange>` when written. An attribute or child
/// order is applied only where it fits the content written, so a layout never
/// makes the output invalid; content added after reading is written in
/// schema order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    /// The rootfile path recorded in container.xml; defaults to designmap.xml.
    pub designmap_path: Option<String>,
    /// The designmap's `aid` processing instruction (the text between `<?`
    /// and `?>`), if not the one for this library's InDesign version.
    pub aid: Option<String>,
    /// `META-INF/container.xml`, if not InDesign's.
    pub container: Option<String>,
    /// The package entries in order, with their compression. Entries not
    /// listed follow in InDesign's order: `mimetype` stored, the others
    /// compressed.
    pub entries: Vec<Entry>,
    /// The layout of each XML part, by path (`designmap.xml`,
    /// `Stories/Story_u1d8.xml`, …).
    pub parts: BTreeMap<String, PartLayout>,
}

/// A package entry: its path and how it is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub compression: Compression,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    Stored,
    Deflated,
}

/// How one XML part is written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartLayout {
    /// Whether a line feed follows the root element. InDesign writes one
    /// after the designmap, spreads, master spreads and stories, and none
    /// after resources and the `XML/` parts.
    pub final_newline: bool,
    pub root: ElementLayout,
}

/// The order of an element's attributes and children.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElementLayout {
    /// Attribute names in the order written. Attributes not listed are
    /// written first, in schema order.
    pub attributes: Vec<Arc<str>>,
    /// Child element names in the order written, each with its own layout.
    /// Applied when the children written are some of these; otherwise the
    /// children are written in schema order.
    pub children: Vec<(Arc<str>, ElementLayout)>,
    /// For an element without content: written with an end tag (`<a></a>`)
    /// rather than as an empty-element tag (`<a />`).
    pub end_tag: bool,
    /// For an element with text: whether the text is written as CDATA
    /// sections. `None` follows InDesign, which writes the text of
    /// `<Contents>` and a few other elements so.
    pub cdata: Option<bool>,
}

impl Layout {
    /// No layout: everything written as InDesign writes it.
    pub const EMPTY: Layout = Layout { designmap_path: None, aid: None, container: None, entries: Vec::new(), parts: BTreeMap::new() };
}

impl PartLayout {
    /// The layout InDesign gives the part at `path`.
    #[cfg(feature = "owned")]
    pub(crate) fn indesign(path: &str) -> PartLayout {
        let final_newline = !(path.starts_with("Resources/") || path.starts_with("XML/"));
        PartLayout { final_newline, root: ElementLayout::default() }
    }

    #[cfg(feature = "owned")]
    pub(crate) fn from_tree(tree: &crate::XmlTree) -> PartLayout {
        let mut layouts = std::collections::HashMap::new();
        let ids: Vec<_> = tree.descendants(tree.root()).filter(|n| tree.name(*n).is_some()).collect();
        for id in ids.into_iter().rev() {
            let children = tree
                .elements(id)
                .filter_map(|child| {
                    layouts.remove(&child).map(|layout| (Arc::from(tree.name(child).map(|n| n.qualified()).unwrap_or_default()), layout))
                })
                .collect();
            let layout = ElementLayout {
                attributes: tree.attributes(id).map(|a| Arc::from(a.name.qualified())).collect(),
                children,
                end_tag: tree.explicit_end(id),
                cdata: tree.cdata(id),
            };
            layouts.insert(id, layout);
        }
        PartLayout { final_newline: tree.original_xml().ends_with('\n'), root: layouts.remove(&tree.root()).unwrap_or_default() }
    }

    /// The layout of a part as read from `text`, whose root element is `root`.
    #[cfg(feature = "owned")]
    pub(crate) fn of(text: &str, root: Node<'_, '_>) -> PartLayout {
        let mut names = Names::default();
        PartLayout { final_newline: text.ends_with('\n'), root: ElementLayout::of(text, root, &mut names) }
    }
}

/// FNV-1a: the maps here hash short names, many times.
#[derive(Default)]
struct Fnv(u64);

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut h = if self.0 == 0 { 0xcbf2_9ce4_8422_2325 } else { self.0 };
        for &b in bytes {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        self.0 = h;
    }
}

type FnvMap<K, V> = HashMap<K, V, BuildHasherDefault<Fnv>>;

/// The names of one part, each allocated once.
#[derive(Default)]
#[cfg(feature = "owned")]
struct Names(FnvMap<String, Arc<str>>);

#[cfg(feature = "owned")]
impl Names {
    fn get(&mut self, name: &str) -> Arc<str> {
        if let Some(n) = self.0.get(name) {
            return n.clone();
        }
        let n: Arc<str> = Arc::from(name);
        self.0.insert(name.to_owned(), n.clone());
        n
    }
}

impl ElementLayout {
    #[cfg(feature = "owned")]
    fn of(text: &str, node: Node<'_, '_>, names: &mut Names) -> ElementLayout {
        let attributes = node.attributes().filter(|a| a.namespace().is_none()).map(|a| names.get(a.name())).collect();
        let children: Vec<(Arc<str>, ElementLayout)> =
            node.children().filter(Node::is_element).map(|n| (names.get(&qualified(n)), ElementLayout::of(text, n, names))).collect();
        let end_tag = node.first_child().is_none() && text.get(node.range()).is_some_and(|source| !source.ends_with("/>"));
        let cdata = match node.first_child() {
            Some(t) if t.is_text() && children.is_empty() => Some(text.get(t.range()).is_some_and(|source| source.starts_with("<![CDATA["))),
            _ => None,
        };
        ElementLayout { attributes, children, end_tag, cdata }
    }

    /// Sorts `attributes`, given in schema order, into this layout's order.
    pub(crate) fn order_attributes<V>(&self, attributes: &mut [(&str, V)]) {
        if self.attributes.is_empty() || self.attributes.iter().map(|a| &**a).eq(attributes.iter().map(|(n, _)| *n)) {
            return;
        }
        let rank: FnvMap<&str, usize> = self.attributes.iter().enumerate().map(|(i, a)| (&**a, i + 1)).collect();
        attributes.sort_by_key(|(name, _)| rank.get(name).copied().unwrap_or(0));
    }

    /// The order to write children with these names in, given in schema
    /// order, as indices into `names`, each with its layout; `None` keeps
    /// schema order.
    pub(crate) fn order_children<'l>(&'l self, names: &[&str]) -> Vec<(usize, Option<&'l ElementLayout>)> {
        if self.children.iter().map(|(n, _)| &**n).eq(names.iter().copied()) {
            return self.children.iter().enumerate().map(|(i, (_, layout))| (i, Some(layout))).collect();
        }
        // The k-th child named N here is the k-th child named N written.
        let mut written: FnvMap<&str, Vec<usize>> = FnvMap::default();
        for (i, name) in names.iter().enumerate() {
            written.entry(name).or_default().push(i);
        }
        let mut taken: FnvMap<&str, usize> = FnvMap::default();
        let mut order = Vec::with_capacity(names.len());
        for (name, layout) in &self.children {
            let k = taken.entry(&**name).or_default();
            if let Some(&i) = written.get(&**name).and_then(|w| w.get(*k)) {
                order.push((i, Some(layout)));
            }
            *k += 1;
        }
        if order.len() == names.len() {
            return order;
        }
        // Children the layout does not have: schema order, with the
        // layouts of those it has.
        let mut seen: FnvMap<&str, usize> = FnvMap::default();
        let by_name = self.by_name();
        names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let k = seen.entry(name).or_default();
                let layout = by_name.get(name).and_then(|l| l.get(*k)).copied();
                *k += 1;
                (i, layout)
            })
            .collect()
    }

    fn by_name(&self) -> FnvMap<&str, Vec<&ElementLayout>> {
        let mut out: FnvMap<&str, Vec<&ElementLayout>> = FnvMap::default();
        for (name, layout) in &self.children {
            out.entry(&**name).or_default().push(layout);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(children: &[&str]) -> ElementLayout {
        ElementLayout {
            children: children.iter().map(|c| (Arc::from(*c), ElementLayout { end_tag: true, ..ElementLayout::default() })).collect(),
            ..ElementLayout::default()
        }
    }

    #[test]
    fn children_follow_the_layout_by_name_and_occurrence() {
        let l = layout(&["Content", "Properties", "Content", "Br"]);
        let order: Vec<usize> = l.order_children(&["Properties", "Content", "Content", "Br"]).into_iter().map(|(i, _)| i).collect();
        assert_eq!(order, [1, 0, 2, 3]);
        // Fewer children: the layout's order of those written.
        let order: Vec<usize> = l.order_children(&["Properties", "Content"]).into_iter().map(|(i, _)| i).collect();
        assert_eq!(order, [1, 0]);
        // A child the layout lacks: schema order.
        let order = l.order_children(&["Properties", "Content", "Table"]);
        assert_eq!(order.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [0, 1, 2]);
        assert!(order[0].1.is_some() && order[2].1.is_none());
    }

    #[test]
    fn attributes_not_in_the_layout_come_first() {
        let l = ElementLayout { attributes: vec![Arc::from("B"), Arc::from("A")], ..ElementLayout::default() };
        let mut attributes = [("A", 1), ("New", 2), ("B", 3)];
        l.order_attributes(&mut attributes);
        assert_eq!(attributes.map(|(n, _)| n), ["New", "B", "A"]);
    }
}
