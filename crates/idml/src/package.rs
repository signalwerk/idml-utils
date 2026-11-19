//! Authoritative source packages and transactional, explicitly validated conversion.
use crate::{
    Element, Error, ErrorKind, Location, Mode, Parsed, ReadOptions, Warning, archive,
    layout::{Compression, Entry},
    profile::{
        CompatibilityProfile, DomVersion, IssueKind, SchemaCatalog, SchemaForm, Support, ValidationIssue, ValidationOptions, ValidationReport,
        WorkBudget,
    },
    tree::{NodeId, XmlTree},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Source-backed IDML. Unknown and invalid schema values remain accessible;
/// schema acceptance is an explicit `validate` operation, not a parsing filter.
#[derive(Debug, Clone)]
pub struct Package {
    designmap: String,
    parts: BTreeMap<String, XmlTree>,
    files: Arc<BTreeMap<String, Vec<u8>>>,
    entries: Vec<Entry>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LossPolicy {
    #[default]
    Forbid,
    Allow,
}
/// A caller's decision about one reported loss in a conversion proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossDecision {
    Approve,
    Refuse,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeKind {
    Version,
    Wrap,
    Default,
    MapValue,
    RemoveAttribute,
    RemoveElement,
}
/// The general effect of an explicitly reported lossy change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossImpact {
    /// Content or an attribute is removed.
    DataRemoval,
    /// A value is replaced by a potentially different meaning.
    SemanticChange,
    /// The reviewed rule declares loss without a more specific classification.
    Other,
}
#[derive(Debug, Clone)]
pub struct ConversionChange {
    pub part: String,
    pub node: NodeId,
    pub location: Location,
    pub kind: ChangeKind,
    pub attribute: Option<String>,
    pub lossy: bool,
    pub reason: String,
    /// Reviewed migration ID, absent for label changes and generic removals.
    pub migration_rule: Option<String>,
    /// Evidence supplied with the reviewed migration, when available.
    pub evidence: Option<String>,
    pub original: String,
    pub replacement: Option<String>,
}
impl ConversionChange {
    pub fn loss_impact(&self) -> Option<LossImpact> {
        if !self.lossy {
            return None;
        }
        Some(match self.kind {
            ChangeKind::RemoveElement | ChangeKind::RemoveAttribute => LossImpact::DataRemoval,
            ChangeKind::MapValue => LossImpact::SemanticChange,
            _ => LossImpact::Other,
        })
    }
}
/// Identifies one loss in a particular immutable conversion proposal.
///
/// An ID remains valid for clones of that proposal, but not for a newly
/// planned conversion, even if its changes look identical. This prevents an
/// approval from silently transferring to a different target or source.
#[derive(Debug, Clone)]
pub struct LossId {
    proposal: Arc<()>,
    index: usize,
}
impl PartialEq for LossId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.proposal, &other.proposal) && self.index == other.index
    }
}
impl Eq for LossId {}
#[derive(Debug, Clone)]
pub struct ProfileCompatibility {
    pub profile: CompatibilityProfile,
    pub validation: ValidationReport,
}
/// A reviewable proposal. Applying it checks both validation and the exact
/// source state; a failed or stale plan never mutates the package.
#[derive(Debug, Clone)]
#[must_use = "review and apply the proposal, or explicitly discard it"]
pub struct ConversionPlan {
    pub(crate) target: String,
    loss_policy: LossPolicy,
    proposal: Arc<()>,
    approved_losses: BTreeSet<usize>,
    changes: Vec<ConversionChange>,
    pub(crate) validation: ValidationReport,
    pub(crate) input: SourceState,
    pub(crate) proposed: Package,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceState {
    designmap: String,
    files: Arc<BTreeMap<String, Vec<u8>>>,
    entries: Vec<Entry>,
    trees: Arc<BTreeMap<String, (NodeId, u64, String)>>,
}
impl ConversionPlan {
    pub(crate) fn staged(
        input: SourceState,
        catalog: &SchemaCatalog,
        target: &CompatibilityProfile,
        proposed: Package,
        options: &ValidationOptions,
    ) -> Self {
        let validation = proposed.validate_with_options(catalog, target, options);
        Self {
            target: target.id.clone(),
            loss_policy: LossPolicy::Forbid,
            proposal: Arc::new(()),
            approved_losses: BTreeSet::new(),
            changes: Vec::new(),
            validation,
            input,
            proposed,
        }
    }
    pub fn target(&self) -> &str {
        &self.target
    }
    /// The bulk policy: `Allow` approves all losses, while `Forbid` means
    /// losses require explicit approval and some may already be approved.
    /// Inspect individual decisions using [`Self::loss_is_approved`].
    pub fn loss_policy(&self) -> LossPolicy {
        self.loss_policy
    }
    pub fn changes(&self) -> &[ConversionChange] {
        &self.changes
    }
    pub fn validation(&self) -> &ValidationReport {
        &self.validation
    }
    /// The losses in this proposal, in change order. IDs belong to this
    /// proposal and can be used to approve or revoke individual changes.
    pub fn losses(&self) -> impl Iterator<Item = (LossId, &ConversionChange)> + '_ {
        self.changes
            .iter()
            .enumerate()
            .filter(|(_, change)| change.lossy)
            .map(|(index, change)| (LossId { proposal: self.proposal.clone(), index }, change))
    }
    /// Switch to individual approval, initially refusing every loss.
    /// This does not alter the preview or make an invalid target valid.
    pub fn require_loss_approval(&mut self) {
        self.loss_policy = LossPolicy::Forbid;
        self.approved_losses.clear();
    }
    /// Approve or revoke one loss in this proposal. If the plan previously
    /// allowed every loss, the first call preserves those approvals and then
    /// changes the specified decision.
    pub fn set_loss_approval(&mut self, id: &LossId, approved: bool) -> Result<(), Error> {
        if !Arc::ptr_eq(&self.proposal, &id.proposal) || !self.changes.get(id.index).is_some_and(|change| change.lossy) {
            return Err(Error::new(ErrorKind::Write, "loss ID does not belong to this conversion proposal"));
        }
        if self.loss_policy == LossPolicy::Allow {
            self.approved_losses.extend(self.changes.iter().enumerate().filter_map(|(i, c)| c.lossy.then_some(i)));
            self.loss_policy = LossPolicy::Forbid;
        }
        if approved {
            self.approved_losses.insert(id.index);
        } else {
            self.approved_losses.remove(&id.index);
        }
        Ok(())
    }
    /// Whether this proposal's current policy approves a reported loss.
    pub fn loss_is_approved(&self, id: &LossId) -> bool {
        Arc::ptr_eq(&self.proposal, &id.proposal)
            && self.changes.get(id.index).is_some_and(|change| change.lossy)
            && (self.loss_policy == LossPolicy::Allow || self.approved_losses.contains(&id.index))
    }
    /// Decide every loss in this proposal, replacing all earlier decisions,
    /// including a bulk `Allow`. `decide` sees each lossy change once, in change
    /// order, with its location, original fragment, replacement, impact and
    /// reviewed rule/evidence; capture [`Self::target`] if the decision depends
    /// on it. Approval never makes an invalid target or stale source applicable.
    /// Returns the number of approved losses.
    pub fn decide_losses(&mut self, mut decide: impl FnMut(&ConversionChange) -> LossDecision) -> usize {
        self.loss_policy = LossPolicy::Forbid;
        self.approved_losses = self
            .changes
            .iter()
            .enumerate()
            .filter(|(_, change)| change.lossy && decide(change) == LossDecision::Approve)
            .map(|(index, _)| index)
            .collect();
        self.approved_losses.len()
    }
    /// Losses that the current decisions leave unapproved, in change order.
    pub fn refused_losses(&self) -> impl Iterator<Item = (LossId, &ConversionChange)> + '_ {
        self.losses().filter(|(id, _)| !self.loss_is_approved(id))
    }
    pub fn can_apply(&self) -> bool {
        self.validation.is_supported() && self.losses().all(|(id, _)| self.loss_is_approved(&id))
    }
    pub fn preview(&self) -> &Package {
        &self.proposed
    }
    pub fn apply(self, package: &mut Package) -> Result<Vec<ConversionChange>, Error> {
        if !self.can_apply() {
            return Err(Error::new(ErrorKind::Schema, "conversion is blocked by unsupported/unknown capabilities or forbidden loss"));
        }
        if package.state()? != self.input {
            return Err(Error::new(ErrorKind::Write, "conversion plan is stale; source changed after planning"));
        }
        *package = self.proposed;
        Ok(self.changes)
    }
}
impl Package {
    pub(crate) fn state(&self) -> Result<SourceState, Error> {
        self.check_references()?;
        let trees = self
            .parts
            .iter()
            .map(|(p, t)| Ok((p.clone(), (t.root(), t.revision(), t.to_xml().map_err(|e| e.in_part(p))?))))
            .collect::<Result<BTreeMap<_, _>, Error>>()?;
        Ok(SourceState { designmap: self.designmap.clone(), files: self.files.clone(), entries: self.entries.clone(), trees: Arc::new(trees) })
    }
    /// Starts a minimal document at the caller's chosen version. No schema or
    /// installed InDesign version is inferred. Use `Authoring` for checked edits.
    pub fn new(version: DomVersion, self_id: impl Into<String>) -> Result<Self, Error> {
        let mut document = Element::new("Document");
        document.set_attr("Self", self_id);
        Self::from_elements(&document, &BTreeMap::new(), version, &ReadOptions::source())
    }
    pub fn read(bytes: &[u8], options: &ReadOptions) -> Result<Parsed<Self>, Error> {
        let mut files = BTreeMap::new();
        let mut entries = Vec::new();
        let mut warnings = Vec::new();
        for f in archive::read(bytes, options)? {
            entries.push(Entry { path: f.name.clone(), compression: f.compression });
            if files.insert(f.name.clone(), f.data).is_some() {
                return Err(Error::new(ErrorKind::Package, format!("duplicate entry {}", f.name)));
            }
        }
        let mut notice = |part: &str, message: &str| -> Result<(), Error> {
            let location = Location { part: part.into(), ..Default::default() };
            if options.mode == Mode::Strict {
                Err(Error::new(ErrorKind::Package, message).at(location))
            } else {
                warnings.push(Warning { message: message.into(), location });
                Ok(())
            }
        };
        if files.get("mimetype").map(Vec::as_slice) != Some(crate::MIMETYPE.as_bytes()) {
            notice("mimetype", "missing or nonstandard mimetype; source is retained")?;
        }
        let designmap = match files.get("META-INF/container.xml") {
            Some(c) => {
                if let Ok(xml) = std::str::from_utf8(c) {
                    crate::tree::check_xml_counts(xml, options).map_err(|e| e.in_part("META-INF/container.xml"))?;
                }
                crate::package::root_file(c).map_err(|e| Error::new(ErrorKind::Package, e))?
            }
            None => {
                notice("META-INF/container.xml", "missing container; assuming designmap.xml")?;
                "designmap.xml".into()
            }
        };
        let xml = files.remove(&designmap).ok_or_else(|| Error::new(ErrorKind::Package, "missing designmap"))?;
        let tree = XmlTree::parse(std::str::from_utf8(&xml).map_err(|_| Error::new(ErrorKind::Xml, "designmap is not UTF-8"))?, options)
            .map_err(|e| e.in_part(&designmap))?;
        if !tree.name(tree.root()).is_some_and(|n| n.namespace.is_empty() && n.local == "Document") {
            return Err(Error::new(ErrorKind::Package, "expected an IDML Document designmap"));
        }
        let paths = references(&tree);
        let mut parts = BTreeMap::from([(designmap.clone(), tree)]);
        for path in paths {
            if parts.contains_key(&path) {
                return Err(Error::new(ErrorKind::Package, "part referenced more than once"));
            }
            let xml = files.remove(&path).ok_or_else(|| Error::new(ErrorKind::Package, format!("missing part {path}")))?;
            let tree =
                XmlTree::parse(std::str::from_utf8(&xml).map_err(|_| Error::new(ErrorKind::Xml, "part is not UTF-8").in_part(&path))?, options)
                    .map_err(|e| e.in_part(&path))?;
            parts.insert(path, tree);
        }
        Ok(Parsed { value: Self { designmap, parts, files: Arc::new(files), entries }, warnings })
    }
    /// Producer roots may supply their own versions. `default_version` is
    /// used only where a version is absent; no version is inferred from a label.
    pub fn from_elements(
        designmap: &Element,
        parts: &BTreeMap<String, Element>,
        default_version: DomVersion,
        options: &ReadOptions,
    ) -> Result<Self, Error> {
        if parts.len().saturating_add(3) > options.max_entries {
            return Err(Error::new(ErrorKind::Limit, "package exceeds max_entries"));
        }
        check_producer_envelope(options, SOURCE_CONTAINER)?;
        let mut total_size = (crate::MIMETYPE.len() + SOURCE_CONTAINER.len()) as u64;
        let mut build = |element: &Element, instruction: Option<&str>| {
            let version = default_version.to_string();
            let mut defaults = Vec::new();
            if element.attr("xmlns:idPkg").is_none() {
                defaults.push(("xmlns:idPkg", crate::xml::PACKAGING_NS));
            }
            if element.attr("DOMVersion").is_none() {
                defaults.push(("DOMVersion", version.as_str()));
            }
            // The tree preflight sees only the budget still available to this
            // part, so an oversized total is rejected before arena allocation.
            let remaining = ReadOptions { max_size: options.max_size.saturating_sub(total_size), ..options.clone() };
            let (t, size) = XmlTree::from_element_with_instruction(element, &defaults, instruction, &remaining)?;
            total_size = total_size.saturating_add(size);
            if total_size > options.max_size {
                return Err(Error::new(ErrorKind::Limit, "producer package exceeds max_size"));
            }
            t.check_limits(options)?;
            Ok::<_, Error>(t)
        };
        // InDesign recognizes a designmap by its `aid` instruction; a new one gets the
        // instruction for its own DOMVersion.
        let aid = aid_instruction(Some(designmap.attr("DOMVersion").unwrap_or(&default_version.to_string())));
        let root = build(designmap, Some(&aid))?;
        if !root.name(root.root()).is_some_and(|n| n.namespace.is_empty() && n.local == "Document") {
            return Err(Error::new(ErrorKind::Package, "expected Document"));
        }
        let mut trees = BTreeMap::new();
        trees.insert("designmap.xml".into(), root);
        for (path, element) in parts {
            crate::PartPath::new(path.clone()).map_err(|e| Error::new(ErrorKind::Package, e.to_string()))?;
            if trees.insert(path.clone(), build(element, None)?).is_some() {
                return Err(Error::new(ErrorKind::Package, "duplicate part path"));
            }
        }
        let files = BTreeMap::from([
            ("mimetype".into(), crate::MIMETYPE.as_bytes().to_vec()),
            ("META-INF/container.xml".into(), SOURCE_CONTAINER.as_bytes().to_vec()),
        ]);
        let package = Self { designmap: "designmap.xml".into(), parts: trees, files: Arc::new(files), entries: Vec::new() };
        package.check_references()?;
        Ok(package)
    }
    pub fn designmap_path(&self) -> &str {
        &self.designmap
    }
    pub fn document(&self) -> &XmlTree {
        &self.parts[&self.designmap]
    }
    pub fn document_mut(&mut self) -> Result<&mut XmlTree, Error> {
        self.parts.get_mut(&self.designmap).ok_or_else(|| Error::new(ErrorKind::Package, "missing designmap"))
    }
    pub fn part(&self, path: &str) -> Option<&XmlTree> {
        self.parts.get(path)
    }
    pub fn part_mut(&mut self, path: &str) -> Option<&mut XmlTree> {
        self.parts.get_mut(path)
    }
    pub fn parts(&self) -> impl Iterator<Item = (&str, &XmlTree)> {
        self.parts.iter().map(|(p, t)| (p.as_str(), t))
    }
    /// Checks current entries and parts against resource limits without ZIP
    /// compression. Edited XML is serialized in memory to measure its size.
    pub fn check_limits(&self, options: &ReadOptions) -> Result<(), Error> {
        self.check_limits_with(options, None, None)
    }
    fn check_limits_with(&self, options: &ReadOptions, document: Option<&XmlTree>, added: Option<(&str, &XmlTree)>) -> Result<(), Error> {
        if self.parts.len().saturating_add(self.files.len()).saturating_add(usize::from(added.is_some())) > options.max_entries {
            return Err(Error::new(ErrorKind::Limit, "package exceeds max_entries"));
        }
        let mut total = 0u64;
        let mut charge = |path: &str, size: u64| -> Result<(), Error> {
            if size > options.max_entry_size {
                return Err(Error::new(ErrorKind::Limit, format!("{path}: entry exceeds max_entry_size")));
            }
            total = total.saturating_add(size);
            if total > options.max_size {
                return Err(Error::new(ErrorKind::Limit, "package exceeds max_size"));
            }
            Ok(())
        };
        for (path, data) in self.files.iter() {
            charge(path, data.len() as u64)?;
            if path == "META-INF/container.xml"
                && let Ok(xml) = std::str::from_utf8(data)
            {
                crate::tree::check_xml_counts(xml, options).map_err(|e| e.in_part(path))?;
                if crate::xml::nesting_exceeds(xml, options.max_depth) {
                    return Err(Error::new(ErrorKind::Limit, "container exceeds max_depth").in_part(path));
                }
            }
        }
        for (path, tree) in &self.parts {
            let tree = if path == &self.designmap { document.unwrap_or(tree) } else { tree };
            charge(path, tree.check_limits_and_size(options).map_err(|e| e.in_part(path))?)?;
        }
        if let Some((path, tree)) = added {
            charge(path, tree.check_limits_and_size(options).map_err(|e| e.in_part(path))?)?;
        }
        Ok(())
    }
    /// Adds a part and its designmap reference together. Only a missing
    /// DOMVersion inherits the document's explicit version; existing labels stay.
    pub fn add_part(&mut self, path: &str, element: &Element, options: &ReadOptions) -> Result<NodeId, Error> {
        crate::PartPath::new(path).map_err(|e| Error::new(ErrorKind::Package, e.to_string()))?;
        if self.parts.len().saturating_add(self.files.len()).saturating_add(1) > options.max_entries {
            return Err(Error::new(ErrorKind::Limit, "package exceeds max_entries"));
        }
        if path.is_empty() || self.parts.contains_key(path) || self.files.contains_key(path) {
            return Err(Error::new(ErrorKind::Package, "empty or duplicate part path"));
        }
        let version = self.document().dom_version().ok_or_else(|| Error::new(ErrorKind::Package, "new parts require an explicit version"))?;
        let mut defaults = Vec::new();
        if element.attr("xmlns:idPkg").is_none() {
            defaults.push(("xmlns:idPkg", crate::xml::PACKAGING_NS));
        }
        if element.attr("DOMVersion").is_none() {
            defaults.push(("DOMVersion", version));
        }
        let (tree, _) = XmlTree::from_element_with(element, &defaults, options)?;
        tree.check_limits(options)?;
        let name = tree.name(tree.root()).ok_or_else(|| Error::new(ErrorKind::Package, "missing part root"))?;
        if name.namespace != crate::xml::PACKAGING_NS {
            return Err(Error::new(ErrorKind::Package, "part roots must use the IDML packaging namespace"));
        }
        let mut reference = Element::new(format!("idPkg:{}", name.local));
        // Reuse the document's binding. InDesign 21.5.1.73 ignored newly
        // referenced story payloads when the reference redundantly redeclared
        // idPkg; source reads still preserve any existing declarations.
        if self.document().namespace_binding(self.document().root(), "idPkg").as_deref() != Some(crate::xml::PACKAGING_NS) {
            reference.set_attr("xmlns:idPkg", crate::xml::PACKAGING_NS);
        }
        reference.set_attr("src", path);
        let mut document = self.document().clone();
        let root = document.root();
        let index = part_reference_position(&document, &name.local);
        let id = document.insert_element(root, index, &reference)?;
        self.check_limits_with(options, Some(&document), Some((path, &tree)))?;
        *self.document_mut()? = document;
        self.parts.insert(path.into(), tree);
        Ok(id)
    }
    pub fn other_files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }
    pub fn other_files_mut(&mut self) -> &mut BTreeMap<String, Vec<u8>> {
        Arc::make_mut(&mut self.files)
    }
    pub fn versions(&self) -> BTreeMap<&str, crate::Field<'_, DomVersion>> {
        self.parts
            .iter()
            .map(|(p, t)| {
                let version = match t.dom_version() {
                    None => crate::Field::Absent,
                    Some(raw) => match raw.parse::<DomVersion>() {
                        Ok(version) => crate::Field::Known(version),
                        Err(e) => crate::Field::Unrecognized { raw: std::borrow::Cow::Borrowed(raw), reason: e.to_string() },
                    },
                };
                (p.as_str(), version)
            })
            .collect()
    }
    fn check_references(&self) -> Result<(), Error> {
        if let Some(path) = self.parts.keys().find(|p| self.files.contains_key(*p)) {
            return Err(Error::new(ErrorKind::Package, format!("duplicate entry {path}")));
        }
        let mut seen = BTreeSet::new();
        for path in references(self.document()) {
            if path == self.designmap || !seen.insert(path.clone()) {
                return Err(Error::new(ErrorKind::Package, format!("duplicate or cyclic part reference {path}")));
            }
            if !self.parts.contains_key(&path) {
                return Err(Error::new(ErrorKind::Package, format!("missing referenced part {path}")));
            }
        }
        Ok(())
    }
    pub fn write(&self) -> Result<Vec<u8>, Error> {
        self.check_references()?;
        let mut rendered = BTreeMap::new();
        for (path, tree) in &self.parts {
            let xml = tree.to_xml().map_err(|e| e.in_part(path))?;
            let xml = if !tree.original_xml().is_empty() {
                xml
            } else if let Some(instruction) = tree.producer_instruction() {
                format!("{}<?{instruction}?>\n{xml}", crate::xml::DECLARATION)
            } else {
                format!("{}{xml}", crate::xml::DECLARATION)
            };
            rendered.insert(path.clone(), xml.into_bytes());
        }
        let layout: BTreeMap<_, _> = self.entries.iter().enumerate().map(|(i, e)| (e.path.as_str(), (i, e.compression))).collect();
        let mut files: Vec<_> = self
            .files
            .iter()
            .chain(rendered.iter())
            .map(|(p, b)| {
                (
                    p.as_str(),
                    b.as_slice(),
                    layout.get(p.as_str()).map_or(if p == "mimetype" { Compression::Stored } else { Compression::Deflated }, |(_, c)| *c),
                )
            })
            .collect();
        files.sort_by_key(|(p, _, _)| layout.get(p).map_or((1, usize::from(*p != "mimetype")), |(i, _)| (0, *i)));
        archive::write(&files)
    }
    pub fn validate(&self, catalog: &SchemaCatalog, profile: &CompatibilityProfile) -> ValidationReport {
        self.validate_with_options(catalog, profile, &ValidationOptions::default())
    }
    /// Validate all parts under one aggregate work budget. Exhaustion makes
    /// the package result unknown even if earlier parts reported errors.
    pub fn validate_with_options(&self, catalog: &SchemaCatalog, profile: &CompatibilityProfile, options: &ValidationOptions) -> ValidationReport {
        let budget = WorkBudget::new(options);
        let mut report = ValidationReport::default();
        for (path, tree) in &self.parts {
            let mut r = catalog.validate_with_budget(profile, tree, &budget);
            for issue in &mut r.issues {
                issue.location.part = path.clone();
            }
            if budget.exhausted() {
                return r;
            }
            report.issues.extend(r.issues);
        }
        report
    }
    /// Observed compatibility with each available package snapshot. Version
    /// labels are hypothetically retargeted; this neither edits nor migrates data.
    /// Results are independent: support is not assumed to be monotone by version.
    pub fn compatibility(&self, catalog: &SchemaCatalog, effective: bool) -> Result<Vec<ProfileCompatibility>, Error> {
        self.compatibility_with_options(catalog, effective, &ValidationOptions::default())
    }
    /// Check each candidate independently using the specified per-candidate budget.
    pub fn compatibility_with_options(
        &self,
        catalog: &SchemaCatalog,
        effective: bool,
        options: &ValidationOptions,
    ) -> Result<Vec<ProfileCompatibility>, Error> {
        let mut results = Vec::new();
        for profile in catalog.profiles().iter().filter(|p| p.snapshot.form == SchemaForm::Package && p.amendments_sha256.is_some() == effective) {
            let mut candidate = self.clone();
            for tree in candidate.parts.values_mut() {
                tree.reversion(&profile.snapshot.dom_version.to_string())?;
            }
            results.push(ProfileCompatibility { profile: profile.clone(), validation: candidate.validate_with_options(catalog, profile, options) });
        }
        results.sort_by(|a, b| (a.profile.snapshot.dom_version, &a.profile.id).cmp(&(b.profile.snapshot.dom_version, &b.profile.id)));
        Ok(results)
    }
    pub fn write_checked(&self, catalog: &SchemaCatalog, profile: &CompatibilityProfile) -> Result<Vec<u8>, Error> {
        self.write_checked_with_options(catalog, profile, &ValidationOptions::default())
    }
    /// Write only after exact validation under the caller's work limits.
    pub fn write_checked_with_options(
        &self,
        catalog: &SchemaCatalog,
        profile: &CompatibilityProfile,
        options: &ValidationOptions,
    ) -> Result<Vec<u8>, Error> {
        self.check_envelope()?;
        let report = self.validate_with_options(catalog, profile, options);
        if profile.snapshot.form != SchemaForm::Package || !report.is_supported() {
            let reason = report.issues.first().map_or("package does not conform to the selected exact profile", |issue| issue.message.as_str());
            let kind = if profile.snapshot.form == SchemaForm::Package
                && report.support() == Support::Unknown
                && report.issues.iter().any(|i| i.kind == IssueKind::Limit)
            {
                ErrorKind::Limit
            } else {
                ErrorKind::Schema
            };
            return Err(Error::new(kind, reason));
        }
        self.write()
    }
    pub(crate) fn check_envelope(&self) -> Result<(), Error> {
        if self.files.get("mimetype").map(Vec::as_slice) != Some(crate::MIMETYPE.as_bytes())
            || self.files.get("META-INF/container.xml").and_then(|c| crate::package::root_file(c).ok()).as_deref() != Some(self.designmap.as_str())
        {
            return Err(Error::new(ErrorKind::Package, "checked output requires a valid mimetype and container pointing at this designmap"));
        }
        Ok(())
    }
    /// Applies only reviewed rewrites; validates every retained DOM part.
    /// Unsupported and uncertain content blocks application by default.
    pub fn plan_conversion(&self, catalog: &SchemaCatalog, target: &CompatibilityProfile, loss_policy: LossPolicy) -> Result<ConversionPlan, Error> {
        self.plan_conversion_with_migrations(catalog, target, loss_policy, crate::migration::MigrationRegistry::builtin()?)
    }
    pub fn plan_conversion_with_options(
        &self,
        catalog: &SchemaCatalog,
        target: &CompatibilityProfile,
        loss_policy: LossPolicy,
        options: &ValidationOptions,
    ) -> Result<ConversionPlan, Error> {
        self.plan_conversion_with_migrations_and_options(catalog, target, loss_policy, crate::migration::MigrationRegistry::builtin()?, options)
    }
    /// Prepare all currently available rewrites, including optional removals,
    /// for review. Every lossy change initially requires individual approval.
    pub fn plan_conversion_for_review(&self, catalog: &SchemaCatalog, target: &CompatibilityProfile) -> Result<ConversionPlan, Error> {
        self.plan_conversion_for_review_with_migrations(catalog, target, crate::migration::MigrationRegistry::builtin()?)
    }
    pub fn plan_conversion_for_review_with_options(
        &self,
        catalog: &SchemaCatalog,
        target: &CompatibilityProfile,
        options: &ValidationOptions,
    ) -> Result<ConversionPlan, Error> {
        let mut plan = self.plan_conversion_with_options(catalog, target, LossPolicy::Allow, options)?;
        plan.require_loss_approval();
        Ok(plan)
    }
    /// Like [`Self::plan_conversion_for_review`], with application-supplied migrations.
    pub fn plan_conversion_for_review_with_migrations(
        &self,
        catalog: &SchemaCatalog,
        target: &CompatibilityProfile,
        migrations: &crate::migration::MigrationRegistry,
    ) -> Result<ConversionPlan, Error> {
        let mut plan = self.plan_conversion_with_migrations(catalog, target, LossPolicy::Allow, migrations)?;
        plan.require_loss_approval();
        Ok(plan)
    }
    pub fn plan_conversion_with_migrations(
        &self,
        catalog: &SchemaCatalog,
        target: &CompatibilityProfile,
        loss_policy: LossPolicy,
        migrations: &crate::migration::MigrationRegistry,
    ) -> Result<ConversionPlan, Error> {
        self.plan_conversion_with_migrations_and_options(catalog, target, loss_policy, migrations, &ValidationOptions::default())
    }
    /// Each grammar validation uses these limits; the bounded route search may
    /// validate several candidate trees, each with a fresh work budget.
    pub fn plan_conversion_with_migrations_and_options(
        &self,
        catalog: &SchemaCatalog,
        target: &CompatibilityProfile,
        loss_policy: LossPolicy,
        migrations: &crate::migration::MigrationRegistry,
        options: &ValidationOptions,
    ) -> Result<ConversionPlan, Error> {
        let input = self.state()?;
        let mut proposed = self.clone();
        let mut changes = Vec::new();
        if target.snapshot.form != SchemaForm::Package {
            return Ok(ConversionPlan {
                target: target.id.clone(),
                loss_policy,
                proposal: Arc::new(()),
                approved_losses: BTreeSet::new(),
                changes,
                validation: ValidationReport {
                    issues: vec![ValidationIssue {
                        support: Support::Unknown,
                        kind: IssueKind::Profile,
                        node: self.document().root(),
                        attribute: None,
                        message: "a consolidated snapshot cannot certify a package conversion".into(),
                        location: Location::default(),
                    }],
                },
                input,
                proposed,
            });
        }
        for (path, tree) in &mut proposed.parts {
            let (candidate, mut edits) = crate::migration::retarget(tree, path, catalog, target, migrations, options)?;
            let old = tree.attribute(tree.root(), "DOMVersion").unwrap_or("").to_owned();
            let new = target.snapshot.dom_version.to_string();
            if old != new {
                let id = tree.root();
                let mut location = tree.location(id);
                location.part = path.clone();
                changes.push(ConversionChange {
                    part: path.clone(),
                    node: id,
                    location,
                    kind: ChangeKind::Version,
                    attribute: Some("DOMVersion".into()),
                    lossy: false,
                    reason: "explicit target profile".into(),
                    migration_rule: None,
                    evidence: None,
                    original: old,
                    replacement: Some(new.clone()),
                });
                tree.reversion(&new)?;
            }
            *tree = candidate;
            changes.append(&mut edits);
        }
        // Explicit lossy conversion can remove rejected attributes/elements.
        // Required-data failures remain blocking; no unknown value is defaulted.
        if loss_policy == LossPolicy::Allow {
            let report = proposed.validate_with_options(catalog, target, options);
            let mut removed = BTreeSet::new();
            for issue in report.issues {
                if !matches!(issue.kind, IssueKind::Element | IssueKind::Attribute | IssueKind::Value) {
                    continue;
                }
                let Some(tree) = proposed.parts.get_mut(&issue.location.part) else { continue };
                if tree.kind(issue.node).is_err() {
                    continue;
                }
                if let Some(attribute) = issue.attribute {
                    let original = tree.attribute_fragment(issue.node, &attribute).unwrap_or_default();
                    changes.push(ConversionChange {
                        part: issue.location.part.clone(),
                        node: issue.node,
                        location: issue.location,
                        kind: ChangeKind::RemoveAttribute,
                        attribute: Some(attribute.clone()),
                        lossy: true,
                        reason: issue.message,
                        migration_rule: None,
                        evidence: None,
                        original,
                        replacement: None,
                    });
                    tree.remove_attribute(issue.node, &attribute)?;
                } else if issue.node != tree.root() && removed.insert(issue.node) {
                    let original = tree.fragment(issue.node)?;
                    changes.push(ConversionChange {
                        part: issue.location.part.clone(),
                        node: issue.node,
                        location: issue.location,
                        kind: ChangeKind::RemoveElement,
                        attribute: None,
                        lossy: true,
                        reason: issue.message,
                        migration_rule: None,
                        evidence: None,
                        original,
                        replacement: None,
                    });
                    tree.remove_node(issue.node)?;
                }
            }
        }
        let mut validation = proposed.validate_with_options(catalog, target, options);
        if let Err(error) = proposed.check_envelope() {
            validation.issues.push(ValidationIssue {
                support: Support::Unsupported,
                kind: IssueKind::Content,
                node: proposed.document().root(),
                attribute: None,
                message: format!("checked package envelope: {}", error.message()),
                location: Location { part: proposed.designmap.clone(), ..Location::default() },
            });
        }
        Ok(ConversionPlan {
            target: target.id.clone(),
            loss_policy,
            proposal: Arc::new(()),
            approved_losses: BTreeSet::new(),
            changes,
            validation,
            input,
            proposed,
        })
    }
}

const SOURCE_CONTAINER: &str = "<?xml version=\"1.0\"?><container xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\" version=\"1.0\"><rootfiles><rootfile full-path=\"designmap.xml\" media-type=\"text/xml\"/></rootfiles></container>";

/// Canonical assembly slots shared by the observed package designmap grammars.
/// This only positions a newly added reference; it never reorders source nodes
/// or establishes target support. Unrecognized elements keep their relative
/// position, and an unfamiliar part type is appended for explicit validation.
fn part_reference_position(document: &XmlTree, part_type: &str) -> usize {
    fn slot(namespace: &str, local: &str) -> Option<u8> {
        if namespace == crate::xml::PACKAGING_NS {
            return Some(match local {
                "Graphic" => 2,
                "Fonts" => 3,
                "Styles" => 6,
                "Preferences" => 12,
                "Tags" => 14,
                "MasterSpread" => 16,
                "Spread" => 17,
                "BackingStory" => 22,
                "Story" => 23,
                "Mapping" => 29,
                _ => return None,
            });
        }
        if !namespace.is_empty() {
            return None;
        }
        Some(match local {
            "Properties" => 0,
            "Language" => 1,
            "KinsokuTable" => 4,
            "MojikumiTable" => 5,
            "NumberingList" => 7,
            "NamedGrid" => 8,
            "MotionPreset" => 9,
            "Condition" => 10,
            "ConditionSet" => 11,
            "ConditionalTextPreference"
            | "EndnoteOption"
            | "TextFrameFootnoteOptionsObject"
            | "LinkedStoryOption"
            | "LinkedPageItemOption"
            | "WatermarkPreference"
            | "MetadataPacketPreference"
            | "TaggedPDFPreference"
            | "AdjustLayoutPreference"
            | "HTMLFXLExportPreference"
            | "PublishExportPreference"
            | "Html5ExportPreference" => 12,
            "TextVariable" => 13,
            "Layer" => 15,
            "Section" => 18,
            "DocumentUser" => 19,
            "CrossReferenceFormat" => 20,
            "Index" => 21,
            "HyperlinkPageDestination" => 24,
            "HyperlinkURLDestination" => 25,
            "HyperlinkExternalPageDestination" => 26,
            "HyperlinkPageItemSource" => 27,
            "Hyperlink" => 28,
            "Bookmark" => 30,
            "PDFCommentPageDestination" | "PDFCommentPageItemDestination" | "PDFCommentTextDestination" | "PDFComment" => 31,
            "PreflightProfile"
            | "DataMergeImagePlaceholder"
            | "HyphenationException"
            | "IndexingSortOption"
            | "ABullet"
            | "Assignment"
            | "ParaStyleMapping"
            | "CharStyleMapping"
            | "TableStyleMapping"
            | "CellStyleMapping"
            | "Article"
            | "ColorGroup" => 32,
            _ => return None,
        })
    }
    let children = || document.elements(document.root());
    let Some(target) = slot(crate::xml::PACKAGING_NS, part_type) else { return children().count() };
    children()
        .position(|node| document.name(node).and_then(|n| slot(&n.namespace, &n.local)).is_some_and(|s| s > target))
        .unwrap_or_else(|| children().count())
}

pub(crate) fn check_producer_envelope(options: &ReadOptions, container: &str) -> Result<(), Error> {
    if crate::MIMETYPE.len() as u64 > options.max_entry_size || container.len() as u64 > options.max_entry_size {
        return Err(Error::new(ErrorKind::Limit, "producer metadata exceeds max_entry_size"));
    }
    if (crate::MIMETYPE.len() as u64).saturating_add(container.len() as u64) > options.max_size {
        return Err(Error::new(ErrorKind::Limit, "producer metadata exceeds max_size"));
    }
    if crate::xml::nesting_exceeds(container, options.max_depth) {
        return Err(Error::new(ErrorKind::Limit, "producer container exceeds max_depth"));
    }
    crate::tree::check_xml_counts(container, options).map_err(|e| e.in_part("META-INF/container.xml"))
}
fn references(tree: &XmlTree) -> Vec<String> {
    tree.elements(tree.root())
        .filter(|id| tree.name(*id).is_some_and(|n| n.namespace == crate::xml::PACKAGING_NS))
        .filter_map(|id| tree.attribute(id, "src").map(str::to_owned))
        .collect()
}

/// The designmap path named by `META-INF/container.xml`.
pub(crate) fn root_file(container: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(container).map_err(|_| "not UTF-8".to_owned())?;
    if crate::xml::nesting_exceeds(text, 8) {
        return Err("nested too deeply".to_owned());
    }
    let doc = roxmltree::Document::parse(text).map_err(|e| e.to_string())?;
    doc.descendants()
        .find(|n| n.tag_name().name() == "rootfile")
        .and_then(|n| n.attribute("full-path"))
        .map(str::to_owned)
        .ok_or_else(|| "no rootfile".to_owned())
}

/// The InDesign builds of the published schema snapshots, by DOMVersion.
const SNAPSHOT_BUILDS: [(&str, &str); 7] = [
    ("7.0", "7.0(535)"),
    ("7.5", "7.5(333)"),
    ("8.0", "8.0(370)"),
    ("10.0", "10.0(70)"),
    ("11.0", "11.0(72)"),
    ("15.0", "15.0(209)"),
    ("21.5", "21.5(64)"),
];

/// The designmap's `aid` processing instruction (without `<?` and `?>`) for a new package
/// of this DOMVersion. The product names the snapshot build when one is known; otherwise the
/// version with build 0. A read designmap keeps its own instruction.
pub(crate) fn aid_instruction(dom_version: Option<&str>) -> String {
    let version = dom_version.unwrap_or("6.0");
    let product = SNAPSHOT_BUILDS.iter().find(|(v, _)| *v == version).map_or_else(|| format!("{version}(0)"), |(_, p)| (*p).to_owned());
    format!("aid style=\"50\" type=\"document\" readerVersion=\"6.0\" featureSet=\"257\" product=\"{product}\" ")
}

#[cfg(test)]
mod aid_tests {
    use super::*;

    #[test]
    fn new_designmaps_carry_the_aid_instruction() -> Result<(), Error> {
        let package = Package::new("8.0".parse()?, "d")?;
        let bytes = package.write()?;
        let read = Package::read(&bytes, &ReadOptions::source())?.value;
        let xml = read.document().original_xml();
        assert!(xml.contains("<?aid style=\"50\" type=\"document\" readerVersion=\"6.0\" featureSet=\"257\" product=\"8.0(370)\" ?>"), "{xml}");
        // Writing a read package again keeps its single instruction.
        let again = Package::read(&read.write()?, &ReadOptions::source())?.value;
        assert_eq!(again.document().original_xml().matches("<?aid").count(), 1);
        assert!(aid_instruction(Some("19.0")).contains("product=\"19.0(0)\""));
        Ok(())
    }
}
