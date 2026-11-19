//! A conservative, read-only index of document identities and relationships.
//!
//! This index is independent of XML parsing and schema validation. It inventories
//! unqualified `Self` attributes on unqualified elements and interprets only the
//! closed catalog of attributes in [`ReferenceKind`]. Each listed relationship had
//! consistent target element types in the local audit corpus; that observation
//! backs the expected targets, not a claim about InDesign's behavior. An
//! unresolved reference is a fact about the indexed package, not proof that a
//! document is invalid: the package may be incomplete or a target may be supplied
//! by an extension. Unlisted attributes, `Properties` children (such as typed
//! `BasedOn` values), namespaced extensions and rendering are outside its scope.
//!
//! External [`ExternalLink`] URIs are recorded as stored and never resolved
//! against a file system. [`SemanticGraph::text_threads`] reconstructs text
//! threads from resolved links and reports which link patterns it treats as
//! inconsistent; other cycles in the reference graph are legitimate.
//!
//! ```
//! # fn main() -> Result<(), idml::Error> {
//! let package = idml::Package::new("8.0".parse()?, "document")?;
//! let graph = package.semantic_graph();
//! assert_eq!(graph.objects_named("document").count(), 1);
//! assert!(graph.text_threads().is_empty());
//! # Ok(())
//! # }
//! ```

use crate::{Location, NodeId, Package};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// An element in a particular revision of a package part.
///
/// Node IDs survive edits, but the relationships and locations in a graph are a
/// snapshot. `location` returns `None` after that part changes or is replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceNode {
    pub part: String,
    pub node: NodeId,
    revision: u64,
}

impl SourceNode {
    /// Resolve this source handle while its tree remains at the indexed revision.
    pub fn location(&self, package: &Package) -> Option<Location> {
        let tree = package.part(&self.part)?;
        if !self.is_current(package) {
            return None;
        }
        let mut location = tree.location(self.node);
        location.part.clone_from(&self.part);
        Some(location)
    }

    /// Whether the part still exists at the revision this handle was indexed from.
    pub fn is_current(&self, package: &Package) -> bool {
        package.part(&self.part).is_some_and(|tree| tree.revision() == self.revision && tree.name(self.node).is_some())
    }
}

/// An element with an unqualified `Self` attribute, including unfamiliar types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticObject {
    pub source: SourceNode,
    pub element: String,
    pub self_id: String,
}

/// The closed set of attribute relationships interpreted by this index.
///
/// The catalog is deliberately small. Each entry names the unqualified attribute,
/// the unqualified elements that carry it for this index, the element types its
/// value was observed to name, and whether native exports used `n` as an explicit
/// nil marker. Other attributes with similar names are not interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum ReferenceKind {
    /// A whitespace-separated token in the designmap document's `StoryList`.
    StoryList,
    ParentStory,
    PreviousTextFrame,
    NextTextFrame,
    AppliedParagraphStyle,
    AppliedCharacterStyle,
    AppliedObjectStyle,
    AppliedTableStyle,
    AppliedCellStyle,
    /// The layer of a page item or guide.
    ItemLayer,
    /// The master spread applied to a page.
    AppliedMaster,
    /// A swatch-like fill: color, swatch, tint, gradient or pasted smooth shade.
    FillColor,
    /// A swatch-like stroke: color, swatch, tint, gradient or pasted smooth shade.
    StrokeColor,
    /// The `Source` attribute of a `Hyperlink`; on other elements it is unrelated.
    HyperlinkSource,
}

const TEXT_CONTAINERS: &[&str] = &["TextFrame", "TextPath", "EndnoteTextFrame"];
const SWATCHES: &[&str] = &["Color", "Swatch", "Tint", "Gradient", "PastedSmoothShade"];

impl ReferenceKind {
    /// Every interpreted relationship, in catalog order.
    pub const ALL: [ReferenceKind; 14] = [
        Self::StoryList,
        Self::ParentStory,
        Self::PreviousTextFrame,
        Self::NextTextFrame,
        Self::AppliedParagraphStyle,
        Self::AppliedCharacterStyle,
        Self::AppliedObjectStyle,
        Self::AppliedTableStyle,
        Self::AppliedCellStyle,
        Self::ItemLayer,
        Self::AppliedMaster,
        Self::FillColor,
        Self::StrokeColor,
        Self::HyperlinkSource,
    ];

    /// The unqualified attribute that stores this relationship.
    pub fn attribute_name(self) -> &'static str {
        match self {
            Self::StoryList => "StoryList",
            Self::ParentStory => "ParentStory",
            Self::PreviousTextFrame => "PreviousTextFrame",
            Self::NextTextFrame => "NextTextFrame",
            Self::AppliedParagraphStyle => "AppliedParagraphStyle",
            Self::AppliedCharacterStyle => "AppliedCharacterStyle",
            Self::AppliedObjectStyle => "AppliedObjectStyle",
            Self::AppliedTableStyle => "AppliedTableStyle",
            Self::AppliedCellStyle => "AppliedCellStyle",
            Self::ItemLayer => "ItemLayer",
            Self::AppliedMaster => "AppliedMaster",
            Self::FillColor => "FillColor",
            Self::StrokeColor => "StrokeColor",
            Self::HyperlinkSource => "Source",
        }
    }

    /// Unqualified elements on which the attribute is interpreted. Empty means
    /// any unqualified element; `StoryList` is read only on the designmap root.
    pub fn owners(self) -> &'static [&'static str] {
        match self {
            Self::StoryList => &["Document"],
            Self::ParentStory | Self::PreviousTextFrame | Self::NextTextFrame => TEXT_CONTAINERS,
            Self::HyperlinkSource => &["Hyperlink"],
            _ => &[],
        }
    }

    /// Element types a single resolved target is expected to have. Another type
    /// yields [`ReferenceResolution::UnexpectedTarget`], an observation only.
    pub fn expected_targets(self) -> &'static [&'static str] {
        match self {
            Self::StoryList => &["Story", "XmlStory"],
            Self::ParentStory => &["Story"],
            Self::PreviousTextFrame | Self::NextTextFrame => TEXT_CONTAINERS,
            Self::AppliedParagraphStyle => &["ParagraphStyle"],
            Self::AppliedCharacterStyle => &["CharacterStyle"],
            Self::AppliedObjectStyle => &["ObjectStyle"],
            Self::AppliedTableStyle => &["TableStyle"],
            Self::AppliedCellStyle => &["CellStyle"],
            Self::ItemLayer => &["Layer"],
            Self::AppliedMaster => &["MasterSpread"],
            Self::FillColor | Self::StrokeColor => SWATCHES,
            Self::HyperlinkSource => &["HyperlinkTextSource", "HyperlinkPageItemSource", "CrossReferenceSource"],
        }
    }

    /// Whether `n` is read as IDML's explicit nil marker for this relationship.
    /// This is limited to relationships where native exports used it; elsewhere
    /// `n` is an ordinary token that may be unresolved.
    pub fn accepts_nil(self) -> bool {
        matches!(
            self,
            Self::PreviousTextFrame
                | Self::NextTextFrame
                | Self::AppliedParagraphStyle
                | Self::AppliedCharacterStyle
                | Self::AppliedObjectStyle
                | Self::AppliedMaster
        )
    }

    /// The scalar relationship stored by `attribute` on an `element`, if any.
    fn for_attribute(element: &str, attribute: &str) -> Option<Self> {
        let kind = match attribute {
            "ParentStory" => Self::ParentStory,
            "PreviousTextFrame" => Self::PreviousTextFrame,
            "NextTextFrame" => Self::NextTextFrame,
            "AppliedParagraphStyle" => Self::AppliedParagraphStyle,
            "AppliedCharacterStyle" => Self::AppliedCharacterStyle,
            "AppliedObjectStyle" => Self::AppliedObjectStyle,
            "AppliedTableStyle" => Self::AppliedTableStyle,
            "AppliedCellStyle" => Self::AppliedCellStyle,
            "ItemLayer" => Self::ItemLayer,
            "AppliedMaster" => Self::AppliedMaster,
            "FillColor" => Self::FillColor,
            "StrokeColor" => Self::StrokeColor,
            "Source" => Self::HyperlinkSource,
            _ => return None,
        };
        let owners = kind.owners();
        (owners.is_empty() || owners.contains(&element)).then_some(kind)
    }
}

/// What the raw reference resolves to within the indexed parts.
///
/// Object indices refer to [`SemanticGraph::objects`]. A single candidate of an
/// unfamiliar element type is `UnexpectedTarget`, not an assertion of invalidity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferenceResolution {
    /// The explicit `n` marker, for relationships where [`ReferenceKind::accepts_nil`].
    Unlinked,
    Resolved(usize),
    /// No indexed unqualified `Self` matches; an external/extension target or
    /// incomplete package is also possible.
    Unresolved,
    Ambiguous(Vec<usize>),
    UnexpectedTarget(usize),
}

/// One attribute reference; a `StoryList` contributes one entry per token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticReference {
    pub source: SourceNode,
    pub kind: ReferenceKind,
    /// The XML-decoded attribute value, before splitting a list.
    pub raw_attribute: Arc<str>,
    /// The exact decoded token used for lookup.
    pub target: String,
    /// Zero-based token index in `StoryList`, or zero for scalar attributes.
    pub token_index: usize,
    pub resolution: ReferenceResolution,
}

impl SemanticReference {
    pub fn attribute_name(&self) -> &'static str {
        self.kind.attribute_name()
    }
}

/// A repeated `Self` identity. This can make references ambiguous; its presence
/// is reported without claiming a schema violation or native-app failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateIdentity {
    pub self_id: String,
    /// Indices into [`SemanticGraph::objects`].
    pub objects: Vec<usize>,
}

/// A `Link` element's external resource reference, as stored.
///
/// The index neither resolves the URI nor checks that a file exists: availability
/// depends on the environment that opens the document, and an unavailable file
/// does not make the package malformed. The URI scheme and `StoredState` (for
/// example `Normal`, `Embedded` or `Contained`) are kept as raw values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalLink {
    pub source: SourceNode,
    /// The XML-decoded `LinkResourceURI`.
    pub uri: String,
    /// The XML-decoded `StoredState`, when present.
    pub stored_state: Option<String>,
}

impl ExternalLink {
    /// The URI scheme before the first `:`, when one is present.
    pub fn scheme(&self) -> Option<&str> {
        self.uri.split_once(':').map(|(scheme, _)| scheme).filter(|s| !s.is_empty())
    }
}

/// A text-thread pattern this index treats as inconsistent.
///
/// Frame values are indices into [`SemanticGraph::objects`]. In the local audit
/// corpus every resolved native `NextTextFrame` link was mirrored by the target's
/// `PreviousTextFrame`, stayed within one `ParentStory`, and no story had more
/// than one thread. These issues describe departures from that observed pattern;
/// they are not claims about how InDesign treats such a document.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ThreadIssue {
    /// The frame's link of this kind resolves to a container whose opposite link
    /// does not resolve back to this frame.
    NotReciprocal { frame: usize, kind: ReferenceKind },
    /// The frame's link of this kind is neither nil nor resolved to one text
    /// container, so the thread cannot be followed past this frame.
    UnfollowedLink { frame: usize, kind: ReferenceKind },
    /// The frame stores a different raw `ParentStory` than the thread's first frame.
    ParentStoryMismatch { frame: usize },
    /// The thread continues into a frame already reached from an earlier thread
    /// start, so two frames name the same next frame.
    JoinsAnotherThread { frame: usize },
    /// Following `NextTextFrame` returns to this frame. The sequence has no first
    /// frame; the thread starts at its first frame in part and tree order.
    Cycle { frame: usize },
    /// Another thread names the same resolved story.
    StoryInSeveralThreads,
}

/// One text thread reconstructed from resolved `NextTextFrame` links.
///
/// Reciprocal `PreviousTextFrame`/`NextTextFrame` pairs and story/frame relations
/// form legitimate cycles in the raw reference graph and are not reported. Only a
/// `NextTextFrame` sequence that returns to one of its frames is a [`ThreadIssue::Cycle`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextThread {
    /// Text container indices into [`SemanticGraph::objects`], in thread order.
    pub frames: Vec<usize>,
    /// The story when every frame stores the same `ParentStory` and it resolves
    /// to exactly one `Story`.
    pub story: Option<usize>,
    pub issues: Vec<ThreadIssue>,
}

/// Snapshot of the recognized source identities and relationships.
///
/// This is intentionally not a complete document model or semantic validator.
/// A graph with no unresolved references makes no claim about unindexed features,
/// schema acceptance, rendering, story threading rules, or external assets.
#[derive(Debug, Clone, Default)]
pub struct SemanticGraph {
    objects: Vec<SemanticObject>,
    references: Vec<SemanticReference>,
    by_identity: BTreeMap<String, Vec<usize>>,
    duplicates: Vec<DuplicateIdentity>,
    external_links: Vec<ExternalLink>,
}

impl SemanticGraph {
    pub fn objects(&self) -> &[SemanticObject] {
        &self.objects
    }

    pub fn references(&self) -> &[SemanticReference] {
        &self.references
    }

    pub fn duplicates(&self) -> &[DuplicateIdentity] {
        &self.duplicates
    }

    /// `Link` resources in part and tree order. These are never `references`.
    pub fn external_links(&self) -> &[ExternalLink] {
        &self.external_links
    }

    /// All indexed elements with this exact identity, in part and tree order.
    pub fn objects_named<'a>(&'a self, identity: &str) -> impl Iterator<Item = &'a SemanticObject> + 'a {
        self.by_identity.get(identity).into_iter().flatten().map(|i| &self.objects[*i])
    }

    /// Resolve an object index from a reference or duplicate diagnostic.
    pub fn object(&self, index: usize) -> Option<&SemanticObject> {
        self.objects.get(index)
    }

    /// References whose source element is this object, in attribute order.
    pub fn references_from<'a>(&'a self, object: usize) -> impl Iterator<Item = &'a SemanticReference> + 'a {
        let source = self.objects.get(object).map(|o| &o.source);
        self.references.iter().filter(move |r| source == Some(&r.source))
    }
}

impl Package {
    /// Build a read-only source index. This does not validate or modify the package.
    pub fn semantic_graph(&self) -> SemanticGraph {
        let mut graph = SemanticGraph::default();
        for (part, tree) in self.parts() {
            for node in tree.descendants(tree.root()) {
                let Some(name) = tree.name(node).filter(|n| n.namespace.is_empty()) else { continue };
                let Some(identity) = tree.attribute_ns(node, "", "Self") else { continue };
                let index = graph.objects.len();
                graph.by_identity.entry(identity.to_owned()).or_default().push(index);
                graph.objects.push(SemanticObject {
                    source: SourceNode { part: part.to_owned(), node, revision: tree.revision() },
                    element: name.local.clone(),
                    self_id: identity.to_owned(),
                });
            }
        }
        graph.duplicates = graph
            .by_identity
            .iter()
            .filter(|(_, indices)| indices.len() > 1)
            .map(|(self_id, objects)| DuplicateIdentity { self_id: self_id.clone(), objects: objects.clone() })
            .collect();

        for (part, tree) in self.parts() {
            for node in tree.descendants(tree.root()) {
                let Some(name) = tree.name(node).filter(|n| n.namespace.is_empty()) else { continue };
                let source = || SourceNode { part: part.to_owned(), node, revision: tree.revision() };
                if part == self.designmap_path()
                    && node == tree.root()
                    && name.local == "Document"
                    && let Some(raw) = tree.attribute_ns(node, "", "StoryList")
                {
                    let source = source();
                    let raw: Arc<str> = Arc::from(raw);
                    for (index, target) in raw.split_ascii_whitespace().enumerate() {
                        graph.add_reference(&source, ReferenceKind::StoryList, &raw, target, index);
                    }
                }
                if name.local == "Link"
                    && let Some(uri) = tree.attribute_ns(node, "", "LinkResourceURI")
                {
                    graph.external_links.push(ExternalLink {
                        source: source(),
                        uri: uri.to_owned(),
                        stored_state: tree.attribute_ns(node, "", "StoredState").map(str::to_owned),
                    });
                }
                for attribute in tree.attributes(node) {
                    if !attribute.name.namespace.is_empty() {
                        continue;
                    }
                    if let Some(kind) = ReferenceKind::for_attribute(&name.local, &attribute.name.local) {
                        let raw = attribute.value.as_str();
                        graph.add_reference(&source(), kind, &Arc::from(raw), raw, 0);
                    }
                }
            }
        }
        graph
    }
}

impl SemanticGraph {
    fn add_reference(&mut self, source: &SourceNode, kind: ReferenceKind, raw: &Arc<str>, target: &str, token_index: usize) {
        let resolution = if target == "n" && kind.accepts_nil() {
            ReferenceResolution::Unlinked
        } else {
            match self.by_identity.get(target).map(Vec::as_slice) {
                None | Some([]) => ReferenceResolution::Unresolved,
                Some([index]) => {
                    if kind.expected_targets().contains(&self.objects[*index].element.as_str()) {
                        ReferenceResolution::Resolved(*index)
                    } else {
                        ReferenceResolution::UnexpectedTarget(*index)
                    }
                }
                Some(indices) => ReferenceResolution::Ambiguous(indices.to_vec()),
            }
        };
        self.references.push(SemanticReference {
            source: source.clone(),
            kind,
            raw_attribute: raw.clone(),
            target: target.to_owned(),
            token_index,
            resolution,
        });
    }

    /// Reconstruct text threads from the indexed container links.
    ///
    /// Every text container object (`TextFrame`, `TextPath`, `EndnoteTextFrame`
    /// with a `Self`) appears in exactly one thread. Threads start at containers
    /// that no resolved `NextTextFrame` names, in part and tree order, followed by
    /// any remaining cycles. Absent and nil links end a thread without an issue.
    pub fn text_threads(&self) -> Vec<TextThread> {
        #[derive(Default, Clone, Copy)]
        struct Links<'g> {
            previous: Option<&'g ReferenceResolution>,
            next: Option<&'g ReferenceResolution>,
            story: Option<&'g SemanticReference>,
        }
        enum Step {
            End,
            Frame(usize),
            Unfollowed,
        }
        fn step(link: Option<&ReferenceResolution>) -> Step {
            match link {
                None | Some(ReferenceResolution::Unlinked) => Step::End,
                Some(ReferenceResolution::Resolved(target)) => Step::Frame(*target),
                Some(_) => Step::Unfollowed,
            }
        }

        let containers: Vec<usize> = (0..self.objects.len()).filter(|i| TEXT_CONTAINERS.contains(&self.objects[*i].element.as_str())).collect();
        let by_source: HashMap<(&str, NodeId), usize> =
            containers.iter().map(|&i| ((self.objects[i].source.part.as_str(), self.objects[i].source.node), i)).collect();
        let mut links: HashMap<usize, Links<'_>> = containers.iter().map(|&i| (i, Links::default())).collect();
        for reference in &self.references {
            let Some(object) = by_source.get(&(reference.source.part.as_str(), reference.source.node)) else { continue };
            let Some(entry) = links.get_mut(object) else { continue };
            match reference.kind {
                ReferenceKind::PreviousTextFrame => entry.previous = Some(&reference.resolution),
                ReferenceKind::NextTextFrame => entry.next = Some(&reference.resolution),
                ReferenceKind::ParentStory => entry.story = Some(reference),
                _ => {}
            }
        }
        let get = |frame: usize| links.get(&frame).copied().unwrap_or_default();
        let points_to = |link: Option<&ReferenceResolution>, frame: usize| matches!(link, Some(ReferenceResolution::Resolved(t)) if *t == frame);

        let mut targeted = std::collections::HashSet::new();
        for &frame in &containers {
            if let Step::Frame(next) = step(get(frame).next) {
                targeted.insert(next);
            }
        }
        // Thread index owning each visited frame.
        let mut owner: HashMap<usize, usize> = HashMap::new();
        let mut threads: Vec<TextThread> = Vec::new();
        let starts = containers.iter().copied().filter(|f| !targeted.contains(f)).chain(containers.iter().copied());
        for start in starts {
            if owner.contains_key(&start) {
                continue;
            }
            let index = threads.len();
            let mut thread = TextThread { frames: Vec::new(), story: None, issues: Vec::new() };
            let mut frame = start;
            loop {
                if let Some(&existing) = owner.get(&frame) {
                    thread.issues.push(if existing == index { ThreadIssue::Cycle { frame } } else { ThreadIssue::JoinsAnotherThread { frame } });
                    break;
                }
                owner.insert(frame, index);
                thread.frames.push(frame);
                match step(get(frame).next) {
                    Step::End => break,
                    Step::Unfollowed => {
                        thread.issues.push(ThreadIssue::UnfollowedLink { frame, kind: ReferenceKind::NextTextFrame });
                        break;
                    }
                    Step::Frame(next) => {
                        if !points_to(get(next).previous, frame) {
                            thread.issues.push(ThreadIssue::NotReciprocal { frame, kind: ReferenceKind::NextTextFrame });
                        }
                        frame = next;
                    }
                }
            }
            threads.push(thread);
        }

        let mut by_story: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (index, thread) in threads.iter_mut().enumerate() {
            for &frame in &thread.frames {
                match step(get(frame).previous) {
                    Step::Frame(previous) if !points_to(get(previous).next, frame) => {
                        thread.issues.push(ThreadIssue::NotReciprocal { frame, kind: ReferenceKind::PreviousTextFrame });
                    }
                    Step::Unfollowed => thread.issues.push(ThreadIssue::UnfollowedLink { frame, kind: ReferenceKind::PreviousTextFrame }),
                    _ => {}
                }
            }
            let raw_story = |frame: usize| get(frame).story.map(|r| r.raw_attribute.as_ref());
            let first = thread.frames.first().copied();
            let mut consistent = true;
            for &frame in thread.frames.iter().skip(1) {
                if raw_story(frame) != first.and_then(raw_story) {
                    consistent = false;
                    thread.issues.push(ThreadIssue::ParentStoryMismatch { frame });
                }
            }
            if consistent && let Some(ReferenceResolution::Resolved(story)) = first.and_then(|f| get(f).story).map(|r| &r.resolution) {
                thread.story = Some(*story);
                by_story.entry(*story).or_default().push(index);
            }
        }
        for indices in by_story.values().filter(|indices| indices.len() > 1) {
            for &index in indices {
                threads[index].issues.push(ThreadIssue::StoryInSeveralThreads);
            }
        }
        threads
    }
}
