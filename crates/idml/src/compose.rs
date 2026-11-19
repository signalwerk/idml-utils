//! Small producers for new story content, independent of the owned projection.
//!
//! These builders produce source elements using the shared vocabulary. They do
//! not choose a schema, inject style defaults or rebuild existing stories. Add
//! the result inside an [`Authoring::plan_edit`](crate::Authoring::plan_edit)
//! session to check the caller's exact target and retain transactional behavior.
use crate::{Element, Error, ErrorKind, NodeId, Package, ReadOptions, Token, view::Builder, views};

/// A new unthreaded rectangular text frame. Local contour coordinates and its
/// transform into the containing spread are explicit, in points. Layers, styles
/// and preferences are caller-selected through the generated formatting API.
#[derive(Debug, Clone)]
#[must_use]
pub struct TextFrameBuilder {
    identity: Token,
    story: Token,
    bounds: crate::geometry::Rect,
    transform: crate::geometry::Transform,
    attributes: Builder<views::TextFrame>,
}
impl TextFrameBuilder {
    pub fn rectangle(
        identity: impl Into<String>,
        story: impl Into<String>,
        bounds: crate::geometry::Rect,
        transform: crate::geometry::Transform,
    ) -> Result<Self, Error> {
        let token = |value| Token::new(value).map_err(|e| Error::new(ErrorKind::Write, e.to_string()));
        let mut attributes = Builder::<views::TextFrame>::new();
        attributes.set_content_type(&views::values::ContentType::TextType)?;
        attributes.set_previous_text_frame(&"n".to_owned())?;
        attributes.set_next_text_frame(&"n".to_owned())?;
        Ok(Self { identity: token(identity.into())?, story: token(story.into())?, bounds, transform, attributes })
    }
    pub fn identity(&self) -> &str {
        self.identity.as_str()
    }
    pub fn story(&self) -> &str {
        self.story.as_str()
    }
    /// Override optional formatting and explicit thread links. The constructor's
    /// identity, story, contour and transform remain authoritative.
    pub fn format(mut self, edit: impl FnOnce(&mut Builder<views::TextFrame>) -> Result<(), Error>) -> Result<Self, Error> {
        edit(&mut self.attributes)?;
        Ok(self)
    }
    pub fn build(mut self) -> Result<Element, Error> {
        self.attributes.set_self_(&self.identity.into_string())?;
        self.attributes.set_parent_story(&self.story.into_string())?;
        self.attributes.set_item_transform(&self.transform.values().to_vec())?;
        let mut points = Builder::<views::PathPointArray>::new();
        for corner in self.bounds.corners() {
            let mut point = Builder::<views::PathPointType>::new();
            let corner = corner.to_vec();
            point.set_anchor(&corner)?;
            point.set_left_direction(&corner)?;
            point.set_right_direction(&corner)?;
            points.push(point.build());
        }
        let mut contour = Builder::<views::GeometryPathType>::new();
        contour.set_path_open(&false)?;
        contour.push(points.build());
        let mut path = Builder::<views::PathGeometry>::new();
        path.push(contour.build());
        let mut properties = Element::new("Properties");
        properties.children.push(path.build());
        self.attributes.push(properties);
        Ok(self.attributes.build())
    }
}

/// A character range containing literal text and explicitly requested breaks.
/// Formatting uses the same generated vocabulary as source views.
#[derive(Debug, Clone)]
#[must_use]
pub struct TextRun {
    attributes: Builder<views::CharacterStyleRange>,
    content: Vec<Element>,
}
impl TextRun {
    /// No newline normalization or implicit paragraph break is performed.
    pub fn new(text: impl Into<String>) -> Self {
        let mut content = Builder::<views::Content>::new();
        content.text(text);
        Self { attributes: Builder::new(), content: vec![content.build()] }
    }
    /// Set formatting using generated named setters. These check vocabulary;
    /// the final Authoring plan checks the exact target grammar and context.
    pub fn format(mut self, edit: impl FnOnce(&mut Builder<views::CharacterStyleRange>) -> Result<(), Error>) -> Result<Self, Error> {
        edit(&mut self.attributes)?;
        Ok(self)
    }
    /// Append an explicit IDML `<Br/>` after this run's text.
    pub fn with_break(mut self) -> Self {
        self.content.push(Builder::<views::Br>::new().build());
        self
    }
    pub fn build(mut self) -> Element {
        for child in self.content {
            self.attributes.push(child);
        }
        self.attributes.build()
    }
}

/// A paragraph style range with ordered character ranges. It does not insert a
/// trailing `<Br/>`: request one on the intended run, including its formatting.
#[derive(Debug, Clone, Default)]
#[must_use]
pub struct Paragraph {
    attributes: Builder<views::ParagraphStyleRange>,
    runs: Vec<TextRun>,
}
impl Paragraph {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn text(text: impl Into<String>) -> Self {
        Self::new().run(TextRun::new(text))
    }
    pub fn run(mut self, run: TextRun) -> Self {
        self.runs.push(run);
        self
    }
    pub fn format(mut self, edit: impl FnOnce(&mut Builder<views::ParagraphStyleRange>) -> Result<(), Error>) -> Result<Self, Error> {
        edit(&mut self.attributes)?;
        Ok(self)
    }
    pub fn build(mut self) -> Element {
        for run in self.runs {
            self.attributes.push(run.build());
        }
        self.attributes.build()
    }
}

/// A producer for one identified story. The ID is a list token so it can also be
/// registered in the designmap's `StoryList` without an ambiguous encoding.
#[derive(Debug, Clone)]
#[must_use]
pub struct StoryBuilder {
    identity: Token,
    attributes: Builder<views::Story>,
    paragraphs: Vec<Paragraph>,
}
impl StoryBuilder {
    pub fn new(identity: impl Into<String>) -> Result<Self, Error> {
        let identity = Token::new(identity).map_err(|e| Error::new(ErrorKind::Write, e.to_string()))?;
        Ok(Self { identity, attributes: Builder::new(), paragraphs: Vec::new() })
    }
    pub fn identity(&self) -> &str {
        self.identity.as_str()
    }
    pub fn paragraph(mut self, paragraph: Paragraph) -> Self {
        self.paragraphs.push(paragraph);
        self
    }
    /// Configure story attributes. The producer's explicit identity wins over
    /// any Self value set by this closure.
    pub fn format(mut self, edit: impl FnOnce(&mut Builder<views::Story>) -> Result<(), Error>) -> Result<Self, Error> {
        edit(&mut self.attributes)?;
        Ok(self)
    }
    pub fn build(mut self) -> Result<Element, Error> {
        self.attributes.set_self_(&self.identity.into_string())?;
        for paragraph in self.paragraphs {
            self.attributes.push(paragraph.build());
        }
        let mut part = Builder::<views::StoryFile>::new();
        part.push(self.attributes.build());
        Ok(part.build())
    }
}

impl Package {
    /// Insert a newly authored frame into exactly one identified Spread. The
    /// story must resolve to one Story in this package. The frame's transform
    /// determines placement; this method does not infer page membership.
    pub fn add_text_frame(&mut self, part: &str, spread_identity: &str, frame: TextFrameBuilder, options: &ReadOptions) -> Result<NodeId, Error> {
        let graph = self.semantic_graph();
        if graph.objects_named(frame.identity()).next().is_some() {
            return Err(Error::new(ErrorKind::Write, "new text frame identity already exists"));
        }
        let mut stories = graph.objects_named(frame.story());
        if !stories.next().is_some_and(|o| o.element == "Story") || stories.next().is_some() {
            return Err(Error::new(ErrorKind::Write, "new text frame requires an unambiguous Story identity"));
        }
        let tree = self.part(part).ok_or_else(|| Error::new(ErrorKind::Package, "missing spread part"))?;
        let mut spreads = tree.descendants(tree.root()).filter(|node| {
            tree.name(*node).is_some_and(|n| n.namespace.is_empty() && n.local == "Spread")
                && tree.attribute_ns(*node, "", "Self") == Some(spread_identity)
        });
        let spread = spreads.next().ok_or_else(|| Error::new(ErrorKind::Write, "spread identity was not found"))?;
        if spreads.next().is_some() {
            return Err(Error::new(ErrorKind::Write, "spread identity is ambiguous"));
        }
        drop(spreads);
        let index = tree.elements(spread).count();
        let mut draft = self.clone();
        let tree = draft.part_mut(part).ok_or_else(|| Error::new(ErrorKind::Package, "missing spread part"))?;
        let node = tree.insert_element(spread, index, &frame.build()?)?;
        draft.check_limits(options)?;
        *self = draft;
        Ok(node)
    }
    /// Add a newly authored story, its part reference and its `StoryList` token
    /// together. Reject identity/path collisions without changing the package.
    /// Only the absent part version inherits the explicit document version.
    ///
    /// Use this in an Authoring draft for exact-target checks. This operation
    /// does not replace or project an existing story, or select a newer target.
    pub fn add_story(&mut self, path: &str, story: StoryBuilder, options: &ReadOptions) -> Result<NodeId, Error> {
        let identity = story.identity().to_owned();
        if self.parts().any(|(_, tree)| tree.descendants(tree.root()).any(|id| tree.attribute_ns(id, "", "Self") == Some(&identity))) {
            return Err(Error::new(ErrorKind::Write, format!("story identity already exists: {identity}")));
        }
        let root = self.document().root();
        let list = self.document().attribute_ns(root, "", "StoryList").unwrap_or("");
        if list.split([' ', '\t', '\r', '\n']).any(|token| token == identity) {
            return Err(Error::new(ErrorKind::Write, format!("story is already listed: {identity}")));
        }
        let updated = if list.is_empty() { identity } else { format!("{list} {identity}") };
        // A draft keeps XML/source changes atomic even if a future setter or
        // package invariant fails after the new part is prepared.
        let mut draft = self.clone();
        let reference = draft.add_part(path, &story.build()?, options)?;
        draft.document_mut()?.set_attribute(root, "StoryList", &updated)?;
        draft.check_limits(options)?;
        *self = draft;
        Ok(reference)
    }
}
