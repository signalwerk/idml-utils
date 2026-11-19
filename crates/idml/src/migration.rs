//! Directed, reviewed migrations between explicit endpoints. Identity retargets
//! are accepted only when the exact destination grammar validates the content.
use crate::{
    DomVersion, Element, Error, ErrorKind, NodeId, XmlTree,
    package::{ChangeKind, ConversionChange},
    profile::{CompatibilityProfile, SchemaCatalog, SchemaForm, ValidationOptions},
};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::OnceLock,
};

#[derive(Debug, Clone)]
pub enum MigrationAction {
    Wrap(Vec<String>),
    MapValue { name: String, from: String, to: String },
    Default { name: String, value: String },
    RemoveElement,
    RemoveAttribute(String),
}
#[derive(Debug, Clone)]
pub struct MigrationRule {
    pub id: String,
    pub from: DomVersion,
    pub to: DomVersion,
    /// Expanded element names in a path suffix: `Story/TextFrame`; names in
    /// other namespaces use Clark notation (`{namespace}local`).
    pub path: Vec<String>,
    pub action: MigrationAction,
    pub lossy: bool,
    pub reason: String,
    pub evidence: String,
}
#[derive(Debug)]
pub struct MigrationRegistry {
    rules: Vec<MigrationRule>,
}
static BUILTIN: OnceLock<Result<MigrationRegistry, Error>> = OnceLock::new();
impl MigrationRegistry {
    pub fn builtin() -> Result<&'static Self, Error> {
        BUILTIN.get_or_init(|| Self::new(crate::migration_data::rules())).as_ref().map_err(Clone::clone)
    }
    pub fn new(rules: Vec<MigrationRule>) -> Result<Self, Error> {
        let mut ids = BTreeSet::new();
        for r in &rules {
            let bad_action = match &r.action {
                MigrationAction::Wrap(names) => names.is_empty() || names.iter().any(String::is_empty),
                MigrationAction::RemoveElement | MigrationAction::RemoveAttribute(_) | MigrationAction::MapValue { .. } => !r.lossy,
                MigrationAction::Default { name, .. } => name == "DOMVersion",
            };
            if r.id.is_empty()
                || !ids.insert(&r.id)
                || r.from == r.to
                || r.path.is_empty()
                || r.path.iter().any(String::is_empty)
                || r.reason.is_empty()
                || r.evidence.is_empty()
                || bad_action
            {
                return Err(Error::new(ErrorKind::Schema, "invalid migration: endpoints, unique ID, path, impact and evidence are required"));
            }
        }
        Ok(Self { rules })
    }
    pub fn rules(&self) -> &[MigrationRule] {
        &self.rules
    }
}

fn name(tree: &XmlTree, id: NodeId) -> String {
    tree.name(id).map_or_else(String::new, |n| if n.namespace.is_empty() { n.local.clone() } else { format!("{{{}}}{}", n.namespace, n.local) })
}
fn applies(tree: &XmlTree, id: NodeId, rule: &MigrationRule) -> bool {
    let mut at = Some(id);
    for expected in rule.path.iter().rev() {
        let Some(id) = at else { return false };
        if name(tree, id) != *expected {
            return false;
        }
        at = tree.parent(id);
    }
    true
}
fn rewrite(tree: &mut XmlTree, path: &str, rules: &[&MigrationRule]) -> Result<Vec<ConversionChange>, Error> {
    let nodes: Vec<_> = tree.descendants(tree.root()).filter(|id| tree.name(*id).is_some()).collect();
    let mut changes = Vec::new();
    for id in nodes {
        for r in rules.iter().filter(|r| applies(tree, id, r)).copied().collect::<Vec<_>>() {
            if tree.kind(id).is_err() {
                break;
            }
            let original = tree.fragment(id)?;
            let mut location = tree.location(id);
            location.part = path.into();
            let (kind, attribute, replacement) = match &r.action {
                MigrationAction::Wrap(wrappers) => {
                    let mut node = id;
                    for wrapper in wrappers.iter().rev() {
                        let parent = tree.parent(node).ok_or_else(|| Error::new(ErrorKind::Write, "cannot wrap the part root"))?;
                        let index = tree.elements(parent).position(|n| n == node).ok_or_else(|| Error::new(ErrorKind::Write, "missing child"))?;
                        let wrapper = tree.insert_element(parent, index, &Element::new(wrapper))?;
                        tree.move_node(node, wrapper, 0)?;
                        node = wrapper;
                    }
                    (ChangeKind::Wrap, None, Some(tree.fragment(node)?))
                }
                MigrationAction::MapValue { name, from, to } => {
                    if tree.attribute(id, name) != Some(from) {
                        continue;
                    }
                    tree.set_attribute(id, name, to)?;
                    (ChangeKind::MapValue, Some(name.clone()), Some(tree.fragment(id)?))
                }
                MigrationAction::Default { name, value } => {
                    if tree.attribute(id, name).is_some() {
                        continue;
                    }
                    tree.set_attribute(id, name, value)?;
                    (ChangeKind::Default, Some(name.clone()), Some(tree.fragment(id)?))
                }
                MigrationAction::RemoveElement => {
                    tree.remove_node(id)?;
                    (ChangeKind::RemoveElement, None, None)
                }
                MigrationAction::RemoveAttribute(name) => {
                    if tree.attribute(id, name).is_none() {
                        continue;
                    }
                    tree.remove_attribute(id, name)?;
                    (ChangeKind::RemoveAttribute, Some(name.clone()), Some(tree.fragment(id)?))
                }
            };
            changes.push(ConversionChange {
                part: path.into(),
                node: id,
                location,
                kind,
                attribute,
                lossy: r.lossy,
                reason: format!("{}: {} ({} → {}; {})", r.id, r.reason, r.from, r.to, r.evidence),
                migration_rule: Some(r.id.clone()),
                evidence: Some(r.evidence.clone()),
                original,
                replacement,
            });
        }
    }
    Ok(changes)
}

pub(crate) fn retarget(
    tree: &XmlTree,
    path: &str,
    catalog: &SchemaCatalog,
    target: &CompatibilityProfile,
    registry: &MigrationRegistry,
    options: &ValidationOptions,
) -> Result<(XmlTree, Vec<ConversionChange>), Error> {
    let mut fallback = tree.clone();
    fallback.reversion(&target.snapshot.dom_version.to_string())?;
    let fallback_validation = catalog.validate_with_options(target, &fallback, options);
    if fallback_validation.is_supported() || fallback_validation.issues.iter().any(|i| i.kind == crate::profile::IssueKind::Limit) {
        return Ok((fallback, Vec::new()));
    }
    let Some(source) = tree.dom_version().and_then(|v| v.parse::<DomVersion>().ok()) else {
        return Ok((fallback, Vec::new()));
    };
    // Prefer routes without semantic/data loss. The search is deliberately
    // bounded; a missing route leaves a blocked proposal, never a guessed edit.
    let mut queue = VecDeque::from([(source, tree.clone(), Vec::<ConversionChange>::new())]);
    let mut visited = BTreeSet::new();
    let mut best = None;
    for _ in 0..64 {
        let Some((version, tree, changes)) = queue.pop_front() else { break };
        if !visited.insert((version, tree.to_xml()?)) {
            continue;
        }
        // Bridge to another reviewed edge only when its source snapshot
        // independently accepts this content. No numeric version interval or
        // monotonic capability assumption is used.
        let starts: BTreeSet<_> = registry.rules.iter().map(|r| r.from).filter(|v| *v != version).collect();
        for start in starts {
            let mut bridge = tree.clone();
            bridge.reversion(&start.to_string())?;
            if catalog.profiles().iter().any(|p| {
                p.snapshot.dom_version == start
                    && p.snapshot.form == SchemaForm::Package
                    && p.amendments_sha256.is_some() == target.amendments_sha256.is_some()
                    && catalog.validate_with_options(p, &bridge, options).is_supported()
            }) {
                queue.push_back((start, bridge, changes.clone()));
            }
        }
        let endpoints: BTreeSet<_> = registry.rules.iter().filter(|r| r.from == version).map(|r| r.to).collect();
        for endpoint in endpoints {
            let rules: Vec<_> = registry.rules.iter().filter(|r| r.from == version && r.to == endpoint).collect();
            let mut candidate = tree.clone();
            let mut edits = changes.clone();
            edits.extend(rewrite(&mut candidate, path, &rules)?);
            candidate.reversion(&endpoint.to_string())?;
            let accepted = catalog.profiles().iter().any(|p| {
                p.snapshot.dom_version == endpoint
                    && p.snapshot.form == SchemaForm::Package
                    && p.amendments_sha256.is_some() == target.amendments_sha256.is_some()
                    && catalog.validate_with_options(p, &candidate, options).is_supported()
            });
            if !accepted {
                continue;
            }
            let mut final_tree = candidate.clone();
            final_tree.reversion(&target.snapshot.dom_version.to_string())?;
            if catalog.validate_with_options(target, &final_tree, options).is_supported() {
                if !edits.iter().any(|e| e.lossy) {
                    return Ok((final_tree, edits));
                }
                if best.is_none() {
                    best = Some((final_tree, edits.clone()));
                }
            }
            queue.push_back((endpoint, candidate, edits));
        }
    }
    Ok(best.unwrap_or((fallback, Vec::new())))
}
