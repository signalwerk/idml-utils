//! Borrowed, source-backed access to stories and text containers.
//!
//! These views expose stored values and node identity without choosing a target
//! version, resolving default values, projecting into the owned model, or
//! interpreting units and geometry. They enumerate unqualified source elements;
//! they do not certify their schema context or application behavior.
//! [`SourceDocument::text_flows`] orders text containers by their stored thread
//! links; it does not compose lines or decide which text fits in which frame.
//!
//! ```
//! # fn main() -> Result<(), idml::Error> {
//! let package = idml::Package::new("8.0".parse()?, "document")?;
//! for story in package.source_document().stories() {
//!     println!("{:?} at {}", story.self_id(), story.source().location());
//! }
//! let flows = package.source_document().text_flows();
//! for thread in flows.threads() {
//!     println!("{} containers, {:?}", flows.containers(thread).len(), thread.issues);
//! }
//! # Ok(())
//! # }
//! ```

use crate::semantic::{SemanticGraph, TextThread};
use crate::tree::NodeKind;
use crate::value::Value;
use crate::{Error, ErrorKind, Field, Location, NodeId, Package, XmlTree};
use std::borrow::Cow;

/// A borrowed source node. Its lifetime prevents mutation of its package.
#[derive(Debug, Clone, Copy)]
pub struct Source<'a> {
    part: &'a str,
    tree: &'a XmlTree,
    node: NodeId,
}

impl<'a> Source<'a> {
    pub fn part(&self) -> &'a str {
        self.part
    }
    pub fn node_id(&self) -> NodeId {
        self.node
    }
    pub fn tree(&self) -> &'a XmlTree {
        self.tree
    }
    pub fn location(&self) -> Location {
        let mut location = self.tree.location(self.node);
        location.part = self.part.to_owned();
        location
    }
    /// The preserved XML fragment, including uninterpreted nodes and attributes.
    pub fn fragment(&self) -> Result<String, Error> {
        self.tree.fragment(self.node)
    }
    /// Read a stored unqualified attribute, without supplying a default.
    pub fn attribute<T: Value>(&self, name: &str) -> Field<'a, T> {
        self.tree.field(self.node, name)
    }
}

/// A read-only document facade borrowing the original package.
#[derive(Debug, Clone, Copy)]
pub struct SourceDocument<'a> {
    package: &'a Package,
}

impl Package {
    pub fn source_document(&self) -> SourceDocument<'_> {
        SourceDocument { package: self }
    }
}

impl<'a> SourceDocument<'a> {
    /// The original package remains available for raw access and validation.
    pub fn package(&self) -> &'a Package {
        self.package
    }

    /// All unqualified `Story` elements in parsed parts, in part and tree order.
    pub fn stories(&self) -> impl Iterator<Item = Story<'a>> + 'a {
        self.package.parts().flat_map(|(part, tree)| {
            tree.descendants(tree.root()).filter_map(move |node| {
                let name = tree.name(node)?;
                (name.namespace.is_empty() && name.local == "Story").then_some(Story { source: Source { part, tree, node } })
            })
        })
    }

    /// Unqualified `TextFrame`, `TextPath` and `EndnoteTextFrame` source elements.
    pub fn text_containers(&self) -> impl Iterator<Item = TextContainer<'a>> + 'a {
        self.package.parts().flat_map(|(part, tree)| {
            tree.descendants(tree.root()).filter_map(move |node| {
                let name = tree.name(node)?;
                let kind = if name.namespace.is_empty() {
                    match name.local.as_str() {
                        "TextFrame" => TextContainerKind::TextFrame,
                        "TextPath" => TextContainerKind::TextPath,
                        "EndnoteTextFrame" => TextContainerKind::EndnoteTextFrame,
                        _ => return None,
                    }
                } else {
                    return None;
                };
                Some(TextContainer { source: Source { part, tree, node }, kind })
            })
        })
    }

    /// Text threads reconstructed from stored `PreviousTextFrame`,
    /// `NextTextFrame` and `ParentStory` links, with borrowed container views.
    /// Thread issues are observations; see [`crate::semantic::ThreadIssue`].
    pub fn text_flows(&self) -> TextFlows<'a> {
        let graph = self.package.semantic_graph();
        let threads = graph.text_threads();
        TextFlows { package: self.package, graph, threads }
    }
}

/// Text threads of one borrowed package revision.
///
/// The flows own the [`SemanticGraph`] snapshot they were built from. Because the
/// package stays borrowed, every object index in a thread resolves to the same
/// source nodes for the lifetime of this value.
#[derive(Debug, Clone)]
pub struct TextFlows<'a> {
    package: &'a Package,
    graph: SemanticGraph,
    threads: Vec<TextThread>,
}

impl<'a> TextFlows<'a> {
    /// The snapshot used for object indices in threads and their issues.
    pub fn graph(&self) -> &SemanticGraph {
        &self.graph
    }

    /// Threads in the order documented by [`SemanticGraph::text_threads`].
    pub fn threads(&self) -> &[TextThread] {
        &self.threads
    }

    fn source(&self, object: usize) -> Option<(Source<'a>, &str)> {
        let object = self.graph.object(object)?;
        let (part, tree) = self.package.parts().find(|(part, _)| *part == object.source.part)?;
        Some((Source { part, tree, node: object.source.node }, object.element.as_str()))
    }

    /// The thread's containers in `NextTextFrame` order. A thread from another
    /// snapshot can yield fewer or unrelated containers; use this value's threads.
    pub fn containers(&self, thread: &TextThread) -> Vec<TextContainer<'a>> {
        thread
            .frames
            .iter()
            .filter_map(|&frame| {
                let (source, element) = self.source(frame)?;
                let kind = match element {
                    "TextFrame" => TextContainerKind::TextFrame,
                    "TextPath" => TextContainerKind::TextPath,
                    "EndnoteTextFrame" => TextContainerKind::EndnoteTextFrame,
                    _ => return None,
                };
                Some(TextContainer { source, kind })
            })
            .collect()
    }

    /// The story every container names, when it resolves to exactly one `Story`.
    pub fn story(&self, thread: &TextThread) -> Option<Story<'a>> {
        let (source, element) = self.source(thread.story?)?;
        (element == "Story").then_some(Story { source })
    }

    /// Threads whose resolved story is this source story. None means the story
    /// is not placed in a resolvable thread; it may still be overset, unplaced
    /// or referenced by links outside the indexed relationships.
    pub fn threads_of<'s>(&'s self, story: &Story<'_>) -> impl Iterator<Item = &'s TextThread> + 's {
        let key = (story.source.part.to_owned(), story.source.node);
        self.threads
            .iter()
            .filter(move |thread| thread.story.and_then(|s| self.graph.object(s)).is_some_and(|o| o.source.part == key.0 && o.source.node == key.1))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Story<'a> {
    source: Source<'a>,
}

impl<'a> Story<'a> {
    pub fn source(&self) -> Source<'a> {
        self.source
    }
    pub fn self_id(&self) -> Field<'a, String> {
        self.source.attribute("Self")
    }
    pub fn track_changes(&self) -> Field<'a, bool> {
        self.source.attribute("TrackChanges")
    }
    /// Walk direct main-story content through paragraph/character style ranges.
    /// Other structures (including tables, footnotes, anchored objects, unknown
    /// elements and processing instructions) are emitted as opaque source nodes.
    /// This does not infer text flow across frames or expand nested sub-stories.
    pub fn content(&self) -> StoryContent<'a> {
        let mut stack = Vec::new();
        stack.extend(self.source.tree.children(self.source.node).iter().rev().copied());
        StoryContent { source: self.source, stack }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextContainerKind {
    TextFrame,
    TextPath,
    EndnoteTextFrame,
}

#[derive(Debug, Clone, Copy)]
pub struct TextContainer<'a> {
    source: Source<'a>,
    kind: TextContainerKind,
}

impl<'a> TextContainer<'a> {
    pub fn source(&self) -> Source<'a> {
        self.source
    }
    pub fn kind(&self) -> TextContainerKind {
        self.kind
    }
    pub fn self_id(&self) -> Field<'a, String> {
        self.source.attribute("Self")
    }
    pub fn parent_story(&self) -> Field<'a, String> {
        self.source.attribute("ParentStory")
    }
    pub fn previous_text_frame(&self) -> Field<'a, String> {
        self.source.attribute("PreviousTextFrame")
    }
    pub fn next_text_frame(&self) -> Field<'a, String> {
        self.source.attribute("NextTextFrame")
    }
    /// The stored transform values; coordinates and units are not converted.
    pub fn item_transform(&self) -> Field<'a, Vec<f64>> {
        self.source.attribute("ItemTransform")
    }
}

/// A source-preserving event; opaque nodes may be inspected through their tree
/// or fragment without pretending they are ordinary main-story text.
#[derive(Debug, Clone)]
pub enum StoryEvent<'a> {
    Text { source: Source<'a>, value: Field<'a, String> },
    Break { source: Source<'a> },
    Opaque { source: Source<'a> },
}

/// An explicit replacement of a simple main-story Content node. It owns its
/// replacement but keeps the source identity/revision it was selected from.
/// Construct through [`StoryEvent::replace_text`], then apply a batch atomically
/// with [`Package::replace_text`], optionally inside an Authoring draft.
#[derive(Debug, Clone)]
#[must_use = "apply this explicit replacement in a source-edit batch"]
pub struct TextReplacement {
    part: String,
    node: NodeId,
    revision: u64,
    text: String,
}

impl StoryEvent<'_> {
    /// Prepare a replacement only for decoded, simple Content. Opaque nodes and
    /// invalid ACE instructions cannot silently disappear through this API.
    pub fn replace_text(&self, text: impl Into<String>) -> Result<TextReplacement, Error> {
        match self {
            Self::Text { source, value: Field::Known(_) }
                if source.tree.name(source.node).is_some_and(|n| n.namespace.is_empty() && n.local == "Content")
                    && is_simple_content(source.tree, source.node)
                    && source.tree.text(source.node).is_ok() =>
            {
                Ok(TextReplacement { part: source.part.to_owned(), node: source.node, revision: source.tree.revision(), text: text.into() })
            }
            _ => Err(Error::new(ErrorKind::Write, "text replacement requires a simple, recognized Content event")),
        }
    }
}

fn is_simple_content(tree: &XmlTree, node: NodeId) -> bool {
    tree.children(node).iter().all(|child| match tree.kind(*child) {
        Ok(NodeKind::Text(_)) => true,
        Ok(NodeKind::Instruction { target, .. }) => target == "ACE",
        _ => false,
    })
}

impl Package {
    /// Apply an explicit batch of source Content edits. All selections are
    /// checked before any mutation, so multiple selections in one part work
    /// together. A stale/duplicate selection or invalid replacement changes
    /// nothing. Unknown siblings and text outside the selections are retained.
    /// This source operation does not change versions or validate a schema.
    pub fn replace_text(&mut self, replacements: &[TextReplacement]) -> Result<(), Error> {
        if replacements.is_empty() {
            return Ok(());
        }
        let mut seen = std::collections::BTreeSet::new();
        for edit in replacements {
            let Some(tree) = self.part(&edit.part) else {
                return Err(Error::new(ErrorKind::Write, "text selection part no longer exists"));
            };
            if tree.revision() != edit.revision || !tree.name(edit.node).is_some_and(|n| n.namespace.is_empty() && n.local == "Content") {
                return Err(Error::new(ErrorKind::Write, "text selection is stale or belongs to another source"));
            }
            if !seen.insert((&edit.part, edit.node)) {
                return Err(Error::new(ErrorKind::Write, "the same Content node was selected more than once"));
            }
        }
        let mut draft = self.clone();
        for edit in replacements {
            let tree = draft.part_mut(&edit.part).ok_or_else(|| Error::new(ErrorKind::Write, "text selection part no longer exists"))?;
            tree.set_text(edit.node, &edit.text)?;
        }
        *self = draft;
        Ok(())
    }
}

/// Iterative source-order traversal, bounded by the source tree rather than the
/// call stack. It never descends into opaque structures.
pub struct StoryContent<'a> {
    source: Source<'a>,
    stack: Vec<NodeId>,
}

impl<'a> Iterator for StoryContent<'a> {
    type Item = StoryEvent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(node) = self.stack.pop() {
            let tree = self.source.tree;
            let source = Source { node, ..self.source };
            if let Some(name) = tree.name(node).filter(|n| n.namespace.is_empty()) {
                match name.local.as_str() {
                    "ParagraphStyleRange" | "CharacterStyleRange" => {
                        self.stack.extend(tree.children(node).iter().rev().copied());
                        continue;
                    }
                    "Content" => {
                        if is_simple_content(tree, node) {
                            let value = match tree.text(node) {
                                Ok(value) => Field::Known(value),
                                Err(error) => {
                                    Field::Unrecognized { raw: Cow::Owned(tree.fragment(node).unwrap_or_default()), reason: error.to_string() }
                                }
                            };
                            return Some(StoryEvent::Text { source, value });
                        }
                    }
                    "Br" if tree.children(node).is_empty() => return Some(StoryEvent::Break { source }),
                    _ => {}
                }
            }
            return Some(StoryEvent::Opaque { source });
        }
        None
    }
}
