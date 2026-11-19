//! Exact schema identities, three-state capability checks and full grammar validation.
use crate::{
    Error, ErrorKind, Location, NodeId, XmlTree,
    tree::{Name, NodeKind},
    value::Value,
};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt,
    str::FromStr,
    sync::OnceLock,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DomVersion {
    pub major: u16,
    pub minor: u16,
}
impl FromStr for DomVersion {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        let bad = || Error::new(ErrorKind::Schema, format!("invalid DOMVersion {s:?}"));
        let (a, b) = s.split_once('.').ok_or_else(bad)?;
        if a.is_empty() || b.is_empty() || !a.bytes().chain(b.bytes()).all(|c| c.is_ascii_digit()) {
            return Err(bad());
        }
        Ok(Self { major: a.parse().map_err(|_| bad())?, minor: b.parse().map_err(|_| bad())? })
    }
}
impl fmt::Display for DomVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Supported,
    Unsupported,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaForm {
    Package,
    Consolidated,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginIdentity {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotIdentity {
    pub dom_version: DomVersion,
    pub build: Option<String>,
    pub form: SchemaForm,
    pub locale: Option<String>,
    pub plugins_complete: bool,
    pub plugins: Option<Vec<PluginIdentity>>,
    pub schema_sha256: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityProfile {
    pub id: String,
    pub snapshot: SnapshotIdentity,
    pub amendments_sha256: Option<String>,
    pub fingerprint: String,
    pub source: String,
    index: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueKind {
    Profile,
    Version,
    Element,
    Attribute,
    Value,
    Content,
    /// Validation could not finish within the configured work or depth limit.
    Limit,
}
/// Limits the work performed while validating schema patterns. Work is an
/// implementation-defined unit covering traversal, matching and value checks;
/// it is not a time limit. Exhaustion yields `Support::Unknown`, never a
/// schema rejection. The same budget can cover every part of a package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidationOptions {
    pub max_work: usize,
    /// Maximum nesting of grammar operations, capped at 256 for stack safety.
    pub max_depth: usize,
}
impl Default for ValidationOptions {
    fn default() -> Self {
        Self { max_work: 50_000_000, max_depth: 256 }
    }
}
pub(crate) struct WorkBudget {
    remaining: Cell<usize>,
    depth: Cell<usize>,
    max_depth: usize,
    exhausted: Cell<bool>,
    limit: Cell<&'static str>,
    stage: Cell<&'static str>,
}
impl WorkBudget {
    pub(crate) fn new(options: &ValidationOptions) -> Self {
        Self {
            remaining: Cell::new(options.max_work),
            depth: Cell::new(0),
            max_depth: options.max_depth.min(256),
            exhausted: Cell::new(false),
            limit: Cell::new("work"),
            stage: Cell::new("schema validation"),
        }
    }
    fn spend(&self, amount: usize, stage: &'static str) -> bool {
        if self.exhausted.get() {
            return false;
        }
        if amount > self.remaining.get() {
            self.exhausted.set(true);
            self.limit.set("work");
            self.stage.set(stage);
            return false;
        }
        self.remaining.set(self.remaining.get() - amount);
        true
    }
    fn enter(&self, stage: &'static str) -> bool {
        if !self.spend(1, stage) {
            return false;
        }
        if self.depth.get() >= self.max_depth {
            self.exhausted.set(true);
            self.limit.set("depth");
            self.stage.set(stage);
            return false;
        }
        self.depth.set(self.depth.get() + 1);
        true
    }
    fn leave(&self) {
        self.depth.set(self.depth.get() - 1);
    }
    pub(crate) fn exhausted(&self) -> bool {
        self.exhausted.get()
    }
}
#[derive(Debug, Clone)]
pub struct ValidationIssue {
    pub support: Support,
    pub kind: IssueKind,
    pub node: NodeId,
    pub attribute: Option<String>,
    pub message: String,
    pub location: Location,
}
#[derive(Debug, Clone, Default)]
pub struct ValidationReport {
    pub issues: Vec<ValidationIssue>,
}
impl ValidationReport {
    pub fn support(&self) -> Support {
        if self.issues.iter().any(|i| i.support == Support::Unsupported) {
            Support::Unsupported
        } else if self.issues.is_empty() {
            Support::Supported
        } else {
            Support::Unknown
        }
    }
    pub fn is_supported(&self) -> bool {
        self.support() == Support::Supported
    }
}
#[derive(Debug, Clone)]
pub enum ProfileResolution<'a> {
    Exact(&'a CompatibilityProfile),
    Ambiguous(Vec<&'a CompatibilityProfile>),
    Unknown { before: Option<DomVersion>, after: Option<DomVersion> },
}
impl<'a> ProfileResolution<'a> {
    /// Require one exact profile without choosing between builds or inventing
    /// a profile for an unobserved version. Use `resolve` directly to inspect
    /// ambiguous candidates and observation bounds programmatically.
    pub fn require_exact(self) -> Result<&'a CompatibilityProfile, Error> {
        match self {
            Self::Exact(profile) => Ok(profile),
            Self::Ambiguous(profiles) => Err(Error::new(
                ErrorKind::Schema,
                format!(
                    "multiple exact package profiles match; select one explicitly by ID: {}",
                    profiles.iter().map(|profile| profile.id.as_str()).collect::<Vec<_>>().join(", ")
                ),
            )),
            Self::Unknown { before, after } => Err(Error::new(
                ErrorKind::Schema,
                format!(
                    "no exact package profile is available (nearest observed versions: before {}, after {}); neighboring snapshots do not establish compatibility",
                    before.map_or_else(|| "unknown".into(), |v| v.to_string()),
                    after.map_or_else(|| "unknown".into(), |v| v.to_string())
                ),
            )),
        }
    }
}
#[derive(Debug, Clone)]
struct Grammar {
    file: String,
    root: u32,
    definitions: BTreeMap<u32, u32>,
}
#[derive(Debug)]
pub struct SchemaCatalog {
    strings: Vec<String>,
    patterns: Vec<Vec<u32>>,
    profiles: Vec<CompatibilityProfile>,
    grammars: Vec<Vec<Grammar>>,
}
static BUILTIN: OnceLock<Result<SchemaCatalog, Error>> = OnceLock::new();
impl SchemaCatalog {
    pub fn builtin() -> Result<&'static Self, Error> {
        BUILTIN.get_or_init(|| Self::from_compressed(crate::profile_data::DATA)).as_ref().map_err(Clone::clone)
    }
    pub fn profiles(&self) -> &[CompatibilityProfile] {
        &self.profiles
    }
    pub fn profile(&self, id: &str) -> Option<&CompatibilityProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }
    /// Resolve the effective package profile for precisely `version`.
    /// Unknown or ambiguous versions return an error; no nearest or newest
    /// profile is selected. Use [`Self::resolve`] to examine the candidates.
    pub fn effective_profile(&self, version: DomVersion) -> Result<&CompatibilityProfile, Error> {
        self.resolve(version, true)
            .require_exact()
            .map_err(|error| Error::new(ErrorKind::Schema, format!("cannot resolve effective profile for {version}: {}", error.message())))
    }
    pub fn resolve(&self, version: DomVersion, effective: bool) -> ProfileResolution<'_> {
        let candidates: Vec<_> = self
            .profiles
            .iter()
            .filter(|p| p.snapshot.dom_version == version && p.amendments_sha256.is_some() == effective && p.snapshot.form == SchemaForm::Package)
            .collect();
        match candidates.len() {
            1 => ProfileResolution::Exact(candidates[0]),
            0 => ProfileResolution::Unknown {
                before: self
                    .profiles
                    .iter()
                    .filter(|p| p.amendments_sha256.is_some() == effective && p.snapshot.form == SchemaForm::Package)
                    .map(|p| p.snapshot.dom_version)
                    .filter(|v| *v < version)
                    .max(),
                after: self
                    .profiles
                    .iter()
                    .filter(|p| p.amendments_sha256.is_some() == effective && p.snapshot.form == SchemaForm::Package)
                    .map(|p| p.snapshot.dom_version)
                    .filter(|v| *v > version)
                    .min(),
            },
            _ => ProfileResolution::Ambiguous(candidates),
        }
    }
    pub fn from_compressed(bytes: &[u8]) -> Result<Self, Error> {
        let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(bytes, 128 << 20).map_err(|_| bad_catalog())?;
        struct Decoder<'a> {
            b: &'a [u8],
            at: usize,
        }
        impl Decoder<'_> {
            fn word(&mut self) -> Result<u32, Error> {
                let end = self.at.checked_add(4).ok_or_else(bad_catalog)?;
                let bytes = self.b.get(self.at..end).ok_or_else(bad_catalog)?;
                self.at = end;
                Ok(u32::from_le_bytes(bytes.try_into().map_err(|_| bad_catalog())?))
            }
            fn count(&mut self) -> Result<usize, Error> {
                let n = self.word()? as usize;
                if n > self.b.len() / 4 + 1 {
                    return Err(bad_catalog());
                }
                Ok(n)
            }
        }
        if !raw.starts_with(b"IDMLPROF2") {
            return Err(bad_catalog());
        }
        let mut d = Decoder { b: &raw, at: 9 };
        let count = d.count()?;
        let mut strings = Vec::with_capacity(count);
        for _ in 0..count {
            let len = d.word()? as usize;
            let end = d.at.checked_add(len).ok_or_else(bad_catalog)?;
            strings.push(std::str::from_utf8(d.b.get(d.at..end).ok_or_else(bad_catalog)?).map_err(|_| bad_catalog())?.to_owned());
            d.at = end;
        }
        let count = d.count()?;
        let mut patterns = Vec::with_capacity(count);
        for _ in 0..count {
            let n = d.count()?;
            let mut p = Vec::with_capacity(n);
            for _ in 0..n {
                p.push(d.word()?)
            }
            patterns.push(p);
        }
        let count = d.count()?;
        let mut profiles = Vec::new();
        let mut grammars = Vec::new();
        let string = |id: u32| strings.get(id as usize).cloned().ok_or_else(bad_catalog);
        for index in 0..count {
            let id = string(d.word()?)?;
            let dom = string(d.word()?)?.parse()?;
            let build = string(d.word()?)?;
            let hash = string(d.word()?)?;
            let amendments = string(d.word()?)?;
            let fingerprint = string(d.word()?)?;
            let source = string(d.word()?)?;
            let form = match d.word()? {
                0 => SchemaForm::Package,
                1 => SchemaForm::Consolidated,
                _ => return Err(bad_catalog()),
            };
            let locale = string(d.word()?)?;
            let inventory_present = d.word()? == 1;
            let plugins_complete = d.word()? == 1;
            let n = d.count()?;
            let mut plugins = Vec::new();
            for _ in 0..n {
                plugins.push(PluginIdentity { name: string(d.word()?)?, version: string(d.word()?)? });
            }
            profiles.push(CompatibilityProfile {
                id,
                snapshot: SnapshotIdentity {
                    dom_version: dom,
                    build: (!build.is_empty()).then_some(build),
                    form,
                    locale: (!locale.is_empty()).then_some(locale),
                    plugins: inventory_present.then_some(plugins),
                    plugins_complete,
                    schema_sha256: hash,
                },
                amendments_sha256: (!amendments.is_empty()).then_some(amendments),
                fingerprint,
                source,
                index,
            });
            let n = d.count()?;
            let mut gs = Vec::new();
            for _ in 0..n {
                let file = string(d.word()?)?;
                let root = d.word()?;
                let n = d.count()?;
                let mut definitions = BTreeMap::new();
                for _ in 0..n {
                    definitions.insert(d.word()?, d.word()?);
                }
                gs.push(Grammar { file, root, definitions });
            }
            grammars.push(gs);
        }
        if d.at != raw.len() {
            return Err(bad_catalog());
        }
        let catalog = Self { strings, patterns, profiles, grammars };
        catalog.check_catalog()?;
        Ok(catalog)
    }
    fn check_catalog(&self) -> Result<(), Error> {
        let ps = |p: u32| (p as usize) < self.patterns.len();
        let ss = |s: u32| (s as usize) < self.strings.len();
        for p in &self.patterns {
            let valid = match p.first() {
                Some(0..=2) => p.len() == 1,
                Some(3 | 4) => p.len() == 5 && ps(p[1]) && ss(p[2]) && ss(p[3]) && p[4] <= 1,
                Some(5..=7) => p.len() > 1 && p[1..].iter().all(|p| ps(*p)),
                Some(8..=11) => p.len() == 2 && ps(p[1]),
                Some(12) => p.len() == 2 && ss(p[1]),
                Some(13) => p.len() == 3 && p[1] <= 1 && ss(p[2]),
                Some(14) => p.len() >= 2 && p.len() % 2 == 0 && p[1..].iter().all(|s| ss(*s)),
                _ => false,
            };
            if !valid {
                return Err(bad_catalog());
            }
        }
        for gs in &self.grammars {
            for g in gs {
                if !ps(g.root) || g.definitions.iter().any(|(s, p)| !ss(*s) || !ps(*p)) {
                    return Err(bad_catalog());
                }
            }
        }
        Ok(())
    }
    fn grammar<'a>(&'a self, profile: &CompatibilityProfile, tree: &XmlTree) -> Option<&'a Grammar> {
        if self.profiles.get(profile.index)? != profile {
            return None;
        }
        let name = tree.name(tree.root())?;
        self.grammars[profile.index].iter().find(|g| self.pattern_name(g.root, name))
    }
    fn pattern_name(&self, p: u32, name: &Name) -> bool {
        let p = &self.patterns[p as usize];
        matches!(p[0], 3 | 4) && (p[4] == 1 || (self.strings[p[2] as usize] == name.namespace && self.strings[p[3] as usize] == name.local))
    }
    fn outside_profile(&self, kind: u32, name: &Name) -> Support {
        if self
            .patterns
            .iter()
            .any(|p| p[0] == kind && p[4] == 0 && self.strings[p[2] as usize] == name.namespace && self.strings[p[3] as usize] == name.local)
        {
            Support::Unsupported
        } else {
            Support::Unknown
        }
    }
    fn outside_profile_bounded(&self, kind: u32, name: &Name, budget: &WorkBudget) -> Support {
        if !budget.spend(self.patterns.len(), "vocabulary lookup") { Support::Unknown } else { self.outside_profile(kind, name) }
    }
    fn issue(
        &self,
        tree: &XmlTree,
        node: NodeId,
        support: Support,
        kind: IssueKind,
        attribute: Option<String>,
        message: impl Into<String>,
    ) -> ValidationIssue {
        ValidationIssue { support, kind, node, attribute, message: message.into(), location: tree.location(node) }
    }
    fn charge_location(tree: &XmlTree, node: NodeId, budget: &WorkBudget) -> bool {
        let mut current = Some(node);
        while let Some(id) = current {
            let parent = tree.parent(id);
            let siblings = parent.map_or(0, |p| tree.children(p).len());
            let name = tree.name(id).map_or(0, |n| n.local.len().saturating_add(n.namespace.len()));
            if !budget.spend(1usize.saturating_add(siblings).saturating_add(name), "diagnostic location") {
                return false;
            }
            current = parent;
        }
        true
    }
    pub fn validate(&self, profile: &CompatibilityProfile, tree: &XmlTree) -> ValidationReport {
        self.validate_with_options(profile, tree, &ValidationOptions::default())
    }
    /// Validate against an exact profile under explicit work and nesting limits.
    /// Exhaustion produces a single `Unknown` limit diagnostic.
    pub fn validate_with_options(&self, profile: &CompatibilityProfile, tree: &XmlTree, options: &ValidationOptions) -> ValidationReport {
        self.validate_with_budget(profile, tree, &WorkBudget::new(options))
    }
    pub(crate) fn validate_with_budget(&self, profile: &CompatibilityProfile, tree: &XmlTree, budget: &WorkBudget) -> ValidationReport {
        let report = self.validate_inner(profile, tree, budget);
        if budget.exhausted() {
            ValidationReport {
                issues: vec![self.issue(
                    tree,
                    tree.root(),
                    Support::Unknown,
                    IssueKind::Limit,
                    None,
                    format!("schema validation {} limit exceeded during {}", budget.limit.get(), budget.stage.get()),
                )],
            }
        } else {
            report
        }
    }
    fn validate_inner(&self, profile: &CompatibilityProfile, tree: &XmlTree, budget: &WorkBudget) -> ValidationReport {
        if !budget.spend(1 + self.grammars.get(profile.index).map_or(0, Vec::len), "profile resolution") {
            return ValidationReport::default();
        }
        let Some(g) = self.grammar(profile, tree) else {
            return ValidationReport {
                issues: vec![self.issue(
                    tree,
                    tree.root(),
                    Support::Unknown,
                    IssueKind::Profile,
                    None,
                    "no grammar for this root in the exact profile",
                )],
            };
        };
        let mut report = ValidationReport::default();
        if tree.dom_version().and_then(|v| v.parse::<DomVersion>().ok()) != Some(profile.snapshot.dom_version) {
            report.issues.push(self.issue(
                tree,
                tree.root(),
                Support::Unsupported,
                IssueKind::Version,
                Some("DOMVersion".into()),
                "document DOMVersion differs from the selected profile",
            ));
        }
        let expected = self.expected(g, tree, budget);
        if budget.exhausted() {
            return report;
        }
        let ids: Vec<_> = tree.descendants(tree.root()).filter(|n| tree.name(*n).is_some()).collect();
        let mut engine = Engine::new(self, g, budget);
        let mut matches = HashMap::<NodeId, BTreeSet<u32>>::new();
        let mut failed = BTreeSet::new();
        for node in ids.into_iter().rev() {
            if !budget.spend(1, "node matching") {
                return report;
            }
            let candidates = expected.get(&node).cloned().unwrap_or_default();
            if candidates.is_empty() {
                let support = tree.name(node).map_or(Support::Unknown, |n| self.outside_profile_bounded(3, n, budget));
                if !Self::charge_location(tree, node, budget) {
                    return report;
                }
                report.issues.push(self.issue(
                    tree,
                    node,
                    support,
                    IssueKind::Element,
                    None,
                    "element is outside this profile's vocabulary at this location",
                ));
                failed.insert(node);
                continue;
            }
            let attrs: Vec<_> = tree.attributes(node).collect();
            let mut good = BTreeSet::new();
            for &p in &candidates {
                if !budget.spend(1 + attrs.len() + tree.children(node).len(), "node matching") {
                    return report;
                }
                let body = self.patterns[p as usize][1];
                let mut state = engine.compile(body, &mut BTreeSet::new());
                for a in &attrs {
                    state = engine.derive(state, &Event::Attribute(&a.name, &a.value), &matches);
                }
                let leaf = tree.elements(node).next().is_none();
                if leaf {
                    for child in tree.children(node) {
                        if let Ok(NodeKind::Text(text)) = tree.kind(*child)
                            && !budget.spend(text.len(), "text matching")
                        {
                            return report;
                        }
                    }
                }
                if leaf
                    && let Ok(text) = tree.text(node)
                    && !text.is_empty()
                {
                    if !budget.spend(text.len(), "text matching") {
                        return report;
                    }
                    let derived = engine.derive(state, &Event::Text(&text), &matches);
                    // RELAX NG weak matching (section 6.2.7): whitespace-only
                    // content also matches as no content, so `<X>\n</X>`
                    // satisfies `empty` and `Y*` like `<X/>`. Text decoded from
                    // ACE instructions is character data, not whitespace.
                    let whitespace = text.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r'))
                        && !tree.children(node).iter().any(|c| matches!(tree.kind(*c), Ok(NodeKind::Instruction { target, .. }) if target == "ACE"));
                    state = if whitespace { engine.combine(7, vec![state, derived]) } else { derived };
                }
                for child in tree.children(node).iter().filter(|_| !leaf) {
                    if budget.exhausted() {
                        return report;
                    }
                    let event = match tree.kind(*child) {
                        Ok(NodeKind::Element(_)) => Some(Event::Element(*child)),
                        Ok(NodeKind::Text(t)) if !t.trim_matches([' ', '\t', '\n', '\r']).is_empty() || tree.elements(node).next().is_none() => {
                            Some(Event::Text(t))
                        }
                        Ok(NodeKind::Instruction { target, value }) if target == "ACE" => {
                            if let Some(c) = value.as_deref().and_then(|v| u32::from_str_radix(v.trim(), 16).ok()).and_then(char::from_u32) {
                                let text = c.to_string();
                                state = engine.derive(state, &Event::Text(&text), &matches);
                            }
                            None
                        }
                        _ => None,
                    };
                    if let Some(e) = event {
                        state = engine.derive(state, &e, &matches);
                    }
                }
                if engine.nullable(state) {
                    good.insert(p);
                }
                if budget.exhausted() {
                    return report;
                }
            }
            if good.is_empty() {
                failed.insert(node);
                let before = report.issues.len();
                for a in &attrs {
                    let values = self.attribute_patterns(g, &candidates, &a.name, budget);
                    if budget.exhausted() {
                        return report;
                    }
                    let support = if values.is_empty() {
                        self.outside_profile_bounded(4, &a.name, budget)
                    } else if values.iter().any(|p| engine.valid_value(*p, &a.value)) {
                        Support::Supported
                    } else {
                        Support::Unsupported
                    };
                    if budget.exhausted() {
                        return report;
                    }
                    if support != Support::Supported {
                        if !Self::charge_location(tree, node, budget) {
                            return report;
                        }
                        report.issues.push(self.issue(
                            tree,
                            node,
                            support,
                            if values.is_empty() { IssueKind::Attribute } else { IssueKind::Value },
                            Some(a.name.qualified()),
                            if values.is_empty() {
                                "attribute is outside this profile at this location"
                            } else {
                                "attribute value does not match the selected profile"
                            },
                        ));
                    }
                }
                if report.issues.len() == before && !tree.elements(node).any(|n| failed.contains(&n)) {
                    if !Self::charge_location(tree, node, budget) {
                        return report;
                    }
                    report.issues.push(self.issue(
                        tree,
                        node,
                        Support::Unsupported,
                        IssueKind::Content,
                        None,
                        format!("content, order or required data does not match {}", g.file),
                    ));
                }
            }
            matches.insert(node, good);
        }
        report
    }
    pub fn attribute_support(&self, profile: &CompatibilityProfile, tree: &XmlTree, node: NodeId, name: &str, value: &str) -> Support {
        let Some(g) = self.grammar(profile, tree) else { return Support::Unknown };
        let budget = WorkBudget::new(&ValidationOptions::default());
        let expected = self.expected(g, tree, &budget);
        if budget.exhausted() {
            return Support::Unknown;
        }
        let Some(candidates) = expected.get(&node) else { return Support::Unknown };
        let Ok(q) = tree.resolve_attribute(node, name) else { return Support::Unsupported };
        let patterns = self.attribute_patterns(g, candidates, &q, &budget);
        if budget.exhausted() {
            return Support::Unknown;
        }
        if patterns.is_empty() {
            return self.outside_profile_bounded(4, &q, &budget);
        }
        let engine = Engine::new(self, g, &budget);
        let valid = patterns.iter().any(|p| engine.valid_value(*p, value));
        if budget.exhausted() {
            Support::Unknown
        } else if valid {
            Support::Supported
        } else {
            Support::Unsupported
        }
    }
    pub fn node_support(&self, profile: &CompatibilityProfile, tree: &XmlTree, node: NodeId) -> Support {
        let Some(g) = self.grammar(profile, tree) else { return Support::Unknown };
        let budget = WorkBudget::new(&ValidationOptions::default());
        let expected = self.expected(g, tree, &budget);
        if budget.exhausted() {
            return Support::Unknown;
        }
        if expected.get(&node).is_none_or(|p| p.is_empty()) {
            return tree.name(node).map_or(Support::Unknown, |n| self.outside_profile_bounded(3, n, &budget));
        }
        let mut nodes = BTreeSet::new();
        for child in tree.descendants(node) {
            if !budget.spend(1, "node support traversal") {
                return Support::Unknown;
            }
            nodes.insert(child);
        }
        let report = self.validate_with_budget(profile, tree, &budget);
        if budget.exhausted() {
            return Support::Unknown;
        }
        let issues: Vec<_> = report.issues.into_iter().filter(|i| nodes.contains(&i.node)).collect();
        if issues.iter().any(|i| i.support == Support::Unsupported) {
            Support::Unsupported
        } else if issues.is_empty() {
            Support::Supported
        } else {
            Support::Unknown
        }
    }
    fn attribute_patterns(&self, g: &Grammar, containers: &[u32], name: &Name, budget: &WorkBudget) -> Vec<u32> {
        let mut out = Vec::new();
        for p in containers {
            if budget.exhausted() {
                break;
            }
            self.walk_content(g, self.patterns[*p as usize][1], budget, &mut |p| {
                if self.patterns[p as usize][0] == 4 && self.pattern_name(p, name) {
                    out.push(self.patterns[p as usize][1]);
                }
            });
        }
        out
    }
    fn walk_content(&self, g: &Grammar, start: u32, budget: &WorkBudget, f: &mut impl FnMut(u32)) {
        let mut pending = vec![start];
        let mut seen = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !budget.spend(1, "grammar traversal") {
                return;
            }
            if !seen.insert(id) {
                continue;
            }
            let p = &self.patterns[id as usize];
            if !budget.spend(p.len(), "grammar traversal") {
                return;
            }
            match p[0] {
                3 | 4 => f(id),
                5..=7 => pending.extend(&p[1..]),
                8..=11 => pending.push(p[1]),
                12 => {
                    if let Some(id) = g.definitions.get(&p[1]) {
                        pending.push(*id)
                    }
                }
                _ => {}
            }
        }
    }
    fn expected(&self, g: &Grammar, tree: &XmlTree, budget: &WorkBudget) -> HashMap<NodeId, Vec<u32>> {
        let mut out = HashMap::new();
        let mut pending = vec![(tree.root(), vec![g.root])];
        while let Some((node, mut candidates)) = pending.pop() {
            if !budget.spend(1 + candidates.len(), "expected element traversal") {
                return out;
            }
            candidates.sort_unstable();
            candidates.dedup();
            let mut children = Vec::new();
            for p in &candidates {
                self.walk_content(g, self.patterns[*p as usize][1], budget, &mut |id| {
                    if self.patterns[id as usize][0] == 3 {
                        children.push(id)
                    }
                });
            }
            for child in tree.elements(node) {
                if !budget.spend(1 + children.len(), "expected element matching") {
                    return out;
                }
                let name = tree.name(child);
                let candidates = children.iter().copied().filter(|p| name.is_some_and(|n| self.pattern_name(*p, n))).collect();
                pending.push((child, candidates));
            }
            out.insert(node, candidates);
        }
        out
    }
}
fn bad_catalog() -> Error {
    Error::new(ErrorKind::Schema, "invalid or unsupported schema catalog")
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Expr {
    Fail,
    Empty,
    Text,
    Element(u32),
    Attribute(u32),
    Scalar(u32),
    Sequence(Vec<usize>),
    Interleave(Vec<usize>),
    Choice(Vec<usize>),
    Repeat(usize),
}
enum Event<'a> {
    Attribute(&'a Name, &'a str),
    Element(NodeId),
    Text(&'a str),
}
struct Engine<'a> {
    catalog: &'a SchemaCatalog,
    grammar: &'a Grammar,
    expressions: Vec<Expr>,
    interned: BTreeMap<Expr, usize>,
    compiled: BTreeMap<u32, usize>,
    budget: &'a WorkBudget,
}
impl<'a> Engine<'a> {
    fn new(catalog: &'a SchemaCatalog, grammar: &'a Grammar, budget: &'a WorkBudget) -> Self {
        Self {
            catalog,
            grammar,
            expressions: vec![Expr::Fail, Expr::Empty],
            interned: BTreeMap::from([(Expr::Fail, 0), (Expr::Empty, 1)]),
            compiled: BTreeMap::new(),
            budget,
        }
    }
    fn intern(&mut self, e: Expr) -> usize {
        let width = match &e {
            Expr::Sequence(cs) | Expr::Interleave(cs) | Expr::Choice(cs) => cs.len(),
            _ => 1,
        };
        if !self.budget.spend(1 + width, "expression allocation") {
            return 0;
        }
        if let Some(id) = self.interned.get(&e) {
            return *id;
        }
        let id = self.expressions.len();
        self.interned.insert(e.clone(), id);
        self.expressions.push(e);
        id
    }
    fn combine(&mut self, tag: u32, children: Vec<usize>) -> usize {
        if !self.budget.spend(1 + children.len(), "expression combination") {
            return 0;
        }
        let mut flat = Vec::new();
        for c in children {
            let width = match &self.expressions[c] {
                Expr::Sequence(cs) | Expr::Interleave(cs) | Expr::Choice(cs) => cs.len(),
                _ => 1,
            };
            if !self.budget.spend(width, "expression combination") {
                return 0;
            }
            match (&self.expressions[c], tag) {
                (Expr::Fail, 7) => continue,
                (Expr::Fail, _) => return 0,
                (Expr::Empty, 5 | 6) => continue,
                (Expr::Choice(ps), 7) | (Expr::Sequence(ps), 5) | (Expr::Interleave(ps), 6) => flat.extend(ps),
                _ => flat.push(c),
            }
        }
        if tag == 7 || tag == 6 {
            flat.sort_unstable();
            if tag == 7 {
                flat.dedup();
            }
        }
        match flat.len() {
            0 => {
                if tag == 7 {
                    0
                } else {
                    1
                }
            }
            1 => flat[0],
            _ => self.intern(match tag {
                5 => Expr::Sequence(flat),
                6 => Expr::Interleave(flat),
                _ => Expr::Choice(flat),
            }),
        }
    }
    fn compile(&mut self, id: u32, seen: &mut BTreeSet<u32>) -> usize {
        if !self.budget.enter("grammar compilation") {
            return 0;
        }
        let out = self.compile_inner(id, seen);
        self.budget.leave();
        out
    }
    fn compile_inner(&mut self, id: u32, seen: &mut BTreeSet<u32>) -> usize {
        if let Some(e) = self.compiled.get(&id) {
            return *e;
        }
        if !seen.insert(id) {
            return 0;
        }
        if !self.budget.spend(self.catalog.patterns[id as usize].len(), "grammar compilation") {
            return 0;
        }
        let p = self.catalog.patterns[id as usize].clone();
        let out = match p[0] {
            0 => 1,
            1 => 0,
            2 => self.intern(Expr::Text),
            3 => self.intern(Expr::Element(id)),
            4 => self.intern(Expr::Attribute(id)),
            5..=7 => {
                let cs = p[1..].iter().map(|p| self.compile(*p, seen)).collect();
                self.combine(p[0], cs)
            }
            8 => {
                let c = self.compile(p[1], seen);
                self.combine(7, vec![1, c])
            }
            9 => {
                let c = self.compile(p[1], seen);
                self.intern(Expr::Repeat(c))
            }
            10 => {
                let c = self.compile(p[1], seen);
                let r = self.intern(Expr::Repeat(c));
                self.combine(5, vec![c, r])
            }
            12 => self.grammar.definitions.get(&p[1]).copied().map_or(0, |id| self.compile(id, seen)),
            _ => self.intern(Expr::Scalar(id)),
        };
        seen.remove(&id);
        self.compiled.insert(id, out);
        out
    }
    fn nullable(&self, id: usize) -> bool {
        if !self.budget.enter("nullable matching") {
            return false;
        }
        let result = self.nullable_inner(id);
        self.budget.leave();
        result
    }
    fn nullable_inner(&self, id: usize) -> bool {
        let width = match &self.expressions[id] {
            Expr::Sequence(cs) | Expr::Interleave(cs) | Expr::Choice(cs) => cs.len(),
            _ => 1,
        };
        if !self.budget.spend(width, "nullable matching") {
            return false;
        }
        match &self.expressions[id] {
            Expr::Empty | Expr::Text | Expr::Repeat(_) => true,
            Expr::Scalar(p) => self.valid_value(*p, ""),
            Expr::Sequence(cs) | Expr::Interleave(cs) => cs.iter().all(|c| self.nullable(*c)),
            Expr::Choice(cs) => cs.iter().any(|c| self.nullable(*c)),
            _ => false,
        }
    }
    fn derive(&mut self, id: usize, event: &Event<'_>, matches: &HashMap<NodeId, BTreeSet<u32>>) -> usize {
        if !self.budget.enter("pattern matching") {
            return 0;
        }
        let result = self.derive_inner(id, event, matches);
        self.budget.leave();
        result
    }
    fn derive_inner(&mut self, id: usize, event: &Event<'_>, matches: &HashMap<NodeId, BTreeSet<u32>>) -> usize {
        let width = match &self.expressions[id] {
            Expr::Sequence(cs) | Expr::Interleave(cs) | Expr::Choice(cs) => cs.len(),
            _ => 1,
        };
        if !self.budget.spend(width, "pattern matching") {
            return 0;
        }
        match self.expressions[id].clone() {
            Expr::Fail | Expr::Empty => 0,
            Expr::Text => {
                if matches!(event, Event::Text(_)) {
                    id
                } else {
                    0
                }
            }
            Expr::Scalar(p) => {
                if let Event::Text(t) = event {
                    if self.valid_value(p, t) { 1 } else { 0 }
                } else {
                    0
                }
            }
            Expr::Element(p) => {
                if let Event::Element(n) = event {
                    if matches.get(n).is_some_and(|m| m.contains(&p)) { 1 } else { 0 }
                } else {
                    0
                }
            }
            Expr::Attribute(p) => {
                if let Event::Attribute(n, v) = event {
                    if self.catalog.pattern_name(p, n) && self.valid_value(self.catalog.patterns[p as usize][1], v) { 1 } else { 0 }
                } else {
                    0
                }
            }
            Expr::Choice(cs) => {
                let ds = cs.into_iter().map(|c| self.derive(c, event, matches)).collect();
                self.combine(7, ds)
            }
            Expr::Repeat(c) => {
                let d = self.derive(c, event, matches);
                self.combine(5, vec![d, id])
            }
            Expr::Sequence(cs) | Expr::Interleave(cs) => {
                let inter = matches!(&self.expressions[id], Expr::Interleave(_));
                let mut alternatives = Vec::new();
                for (i, &c) in cs.iter().enumerate() {
                    if !self.budget.spend(1, "pattern matching") {
                        return 0;
                    }
                    let d = self.derive(c, event, matches);
                    if d != 0 {
                        // Only a matching alternative copies the sequence.
                        // Charging its width for every rejected child would
                        // make a linear scan consume a quadratic work budget.
                        if !self.budget.spend(cs.len(), "matching sequence allocation") {
                            return 0;
                        }
                        let mut next = if !inter && !matches!(event, Event::Attribute(..)) { cs[i..].to_vec() } else { cs.clone() };
                        next[if !inter && !matches!(event, Event::Attribute(..)) { 0 } else { i }] = d;
                        alternatives.push(self.combine(if inter { 6 } else { 5 }, next));
                    }
                    if !inter && !matches!(event, Event::Attribute(..)) && !self.nullable(c) {
                        break;
                    }
                }
                self.combine(7, alternatives)
            }
        }
    }
    fn valid_value(&self, id: u32, text: &str) -> bool {
        if !self.budget.enter("value matching") {
            return false;
        }
        let result = self.valid_value_inner(id, text);
        self.budget.leave();
        result
    }
    fn valid_value_inner(&self, id: u32, text: &str) -> bool {
        if !self.budget.spend(text.len().saturating_add(self.catalog.patterns[id as usize].len()), "value matching") {
            return false;
        }
        let p = &self.catalog.patterns[id as usize];
        match p[0] {
            0 => text.is_empty(),
            1 => false,
            2 => true,
            7 => p[1..].iter().any(|p| self.valid_value(*p, text)),
            8 => text.is_empty() || self.valid_value(p[1], text),
            11 => {
                let tokens: Vec<_> = text.split([' ', '\t', '\n', '\r']).filter(|t| !t.is_empty()).collect();
                self.tokens(p[1], &tokens, 0).contains(&tokens.len())
            }
            12 => self.grammar.definitions.get(&p[1]).is_some_and(|p| self.valid_value(*p, text)),
            13 => {
                let expected = &self.catalog.strings[p[2] as usize];
                if p[1] == 1 {
                    text.split([' ', '\t', '\n', '\r']).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ") == *expected
                } else {
                    text == expected
                }
            }
            14 => self.scalar(p, text),
            _ => false,
        }
    }
    fn scalar(&self, p: &[u32], text: &str) -> bool {
        let ty = self.catalog.strings[p[1] as usize].as_str();
        let number = match ty {
            "string" => None,
            "boolean" => return bool::parse(text).is_ok(),
            "dateTime" => return crate::DateTime::parse(text).is_ok(),
            "int" => match i32::parse(text) {
                Ok(n) => Some(n as f64),
                Err(_) => return false,
            },
            "short" => match i16::parse(text) {
                Ok(n) => Some(n as f64),
                Err(_) => return false,
            },
            "double" => match f64::parse(text) {
                Ok(n) => Some(n),
                Err(_) => return false,
            },
            _ => return false,
        };
        for pair in p[2..].chunks_exact(2) {
            let name = self.catalog.strings[pair[0] as usize].as_str();
            let value = &self.catalog.strings[pair[1] as usize];
            let bound = value.parse::<f64>().ok();
            let valid = match name {
                "minInclusive" => number.zip(bound).is_some_and(|(n, b)| n >= b),
                "maxInclusive" => number.zip(bound).is_some_and(|(n, b)| n <= b),
                "pattern" if value == ".*\\.xml" => text.ends_with(".xml"),
                _ => false,
            };
            if !valid {
                return false;
            }
        }
        true
    }
    fn tokens(&self, id: u32, tokens: &[&str], at: usize) -> BTreeSet<usize> {
        if !self.budget.enter("list matching") {
            return BTreeSet::new();
        }
        let result = self.tokens_inner(id, tokens, at);
        self.budget.leave();
        result
    }
    fn tokens_inner(&self, id: u32, tokens: &[&str], at: usize) -> BTreeSet<usize> {
        let p = &self.catalog.patterns[id as usize];
        if !self.budget.spend(p.len(), "list matching") {
            return BTreeSet::new();
        }
        match p[0] {
            0 => BTreeSet::from([at]),
            5 => {
                let mut states = BTreeSet::from([at]);
                for p in &p[1..] {
                    if !self.budget.spend(states.len(), "list matching") {
                        return BTreeSet::new();
                    }
                    states = states.iter().flat_map(|at| self.tokens(*p, tokens, *at)).collect();
                }
                states
            }
            7 => p[1..].iter().flat_map(|p| self.tokens(*p, tokens, at)).collect(),
            8 => self.tokens(p[1], tokens, at).into_iter().chain([at]).collect(),
            9 | 10 => {
                let mut states = if p[0] == 9 { BTreeSet::from([at]) } else { self.tokens(p[1], tokens, at) };
                let mut pending: Vec<_> = states.iter().copied().collect();
                while let Some(at) = pending.pop() {
                    if self.budget.exhausted() {
                        return BTreeSet::new();
                    }
                    for end in self.tokens(p[1], tokens, at) {
                        if end > at && states.insert(end) {
                            pending.push(end)
                        }
                    }
                }
                states
            }
            12 => self.grammar.definitions.get(&p[1]).map_or_else(BTreeSet::new, |p| self.tokens(*p, tokens, at)),
            _ => {
                if tokens.get(at).is_some_and(|t| self.valid_value(id, t)) {
                    BTreeSet::from([at + 1])
                } else {
                    BTreeSet::new()
                }
            }
        }
    }
}
