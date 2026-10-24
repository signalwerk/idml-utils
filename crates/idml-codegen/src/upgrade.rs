//! Reading documents of older IDML versions into the current model.
//!
//! `schema-upgrades.tsv`, next to the `idml` crate's manifest, has a rule
//! for every schema item that an older snapshot has and the current one has
//! not, or has differently. Generation fails when such an item has no rule,
//! or a rule no item. The rules become edits that the reader makes to the
//! XML of an older document before reading it (`src/model/upgrades.rs`);
//! the reports show them next to the changes.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::history::{History, container, last_segment, parent};

/// A rule of `schema-upgrades.tsv`: `item`, `rule` and `note`, separated by
/// tabs.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub item: String,
    pub action: Action,
    /// Why; required for `drop`, which loses data. Shown in reports and
    /// with the reader's warnings.
    pub note: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// `drop`: the element, attribute or enumeration value is left out, with
    /// a warning.
    Drop,
    /// `wrap <A>/<B>`: the element is put into new elements, outermost first.
    Wrap(Vec<String>),
    /// `default "v"`: an attribute that became required gets `v` where it is
    /// missing.
    Default(String),
    /// `value "v"`: an enumeration value becomes `v`.
    Value(String),
    /// `keep`: every old value is valid in the current schema.
    Keep,
}

impl Action {
    pub fn describe(&self) -> String {
        match self {
            Action::Drop => "dropped".to_owned(),
            Action::Wrap(path) => format!("put into {}", angle(path)),
            Action::Default(v) => format!("set to {v:?} where missing"),
            Action::Value(v) => format!("read as {v:?}"),
            Action::Keep => "read as it is".to_owned(),
        }
    }
}

fn angle(path: &[String]) -> String {
    path.iter().map(|n| format!("<{n}>")).collect::<Vec<_>>().join("/")
}

pub fn parse(text: &str) -> Result<Vec<Rule>, String> {
    let mut rules: Vec<Rule> = Vec::new();
    for (n, text) in text.lines().enumerate() {
        let line = n + 1;
        let err = |m: &str| format!("schema-upgrades.tsv:{line}: {m}");
        if text.is_empty() || text.starts_with('#') || text == "item\trule\tnote" {
            continue;
        }
        let cols: Vec<&str> = text.split('\t').collect();
        let (item, rule, note) = match cols.as_slice() {
            [item, rule] => (*item, *rule, ""),
            [item, rule, note] => (*item, *rule, *note),
            _ => return Err(err("expected item, rule and note, separated by tabs")),
        };
        let (word, arg) = rule.split_once(' ').map_or((rule, ""), |(w, a)| (w, a.trim()));
        let action = match (word, arg) {
            ("drop", "") => Action::Drop,
            ("keep", "") => Action::Keep,
            ("wrap", path) if !path.is_empty() => {
                let names: Option<Vec<String>> = path.split('/').map(|s| s.strip_prefix('<')?.strip_suffix('>').map(str::to_owned)).collect();
                Action::Wrap(names.ok_or_else(|| err("expected wrap <A>/<B>"))?)
            }
            ("default", v) => Action::Default(quoted(v).ok_or_else(|| err("expected default \"value\""))?),
            ("value", v) => Action::Value(quoted(v).ok_or_else(|| err("expected value \"literal\""))?),
            _ => return Err(err(&format!("unknown rule {rule:?}; expected drop, wrap <A>/<B>, default \"v\", value \"v\" or keep"))),
        };
        if action == Action::Drop && note.trim().is_empty() {
            return Err(err("a drop loses data: say why in the note"));
        }
        if rules.iter().any(|r| r.item == item) {
            return Err(err(&format!("second rule for {item}")));
        }
        rules.push(Rule { item: item.to_owned(), action, note: note.trim().to_owned(), line });
    }
    Ok(rules)
}

/// `"text"` without escapes.
fn quoted(s: &str) -> Option<String> {
    let inner = s.strip_prefix('"')?.strip_suffix('"')?;
    (!inner.contains(['"', '\\'])).then(|| inner.to_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Older snapshots have the item, the current one has not.
    Gone,
    /// The current snapshot has it differently.
    Changed,
}

/// The items older snapshots have and the current one has not, or has
/// differently, with the snapshots that have them.
pub fn needs(h: &History) -> BTreeMap<String, (Need, BTreeSet<usize>)> {
    let Some(current) = h.versions.len().checked_sub(1) else { return BTreeMap::new() };
    let mut out: BTreeMap<String, (Need, BTreeSet<usize>)> = BTreeMap::new();
    for (id, states) in &h.items {
        for (v, state) in states.iter().enumerate().take(current) {
            let Some(old) = state else { continue };
            if !h.covers(v, id) {
                continue;
            }
            let need = match &states[current] {
                None => Need::Gone,
                Some(new) if new != old => Need::Changed,
                Some(_) => continue,
            };
            out.entry(id.clone()).or_insert((need, BTreeSet::new())).1.insert(v);
        }
    }
    out
}

/// An edit the reader makes to an older document.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Edit {
    Drop,
    Wrap(Vec<String>),
    DropAttribute(String),
    DropValue(String, String),
    MapValue(String, String, String),
    Default(String, String),
}

/// An edit, where it applies, and the rule it comes from.
#[derive(Debug, Clone, PartialEq)]
pub struct XmlRule {
    /// The snapshots whose documents it applies to.
    pub versions: BTreeSet<usize>,
    /// Element names the element's path ends with, outermost first.
    pub path: Vec<String>,
    pub edit: Edit,
    /// The rule's item.
    pub item: String,
    pub note: String,
}

/// The checked rules.
#[derive(Debug)]
pub struct Upgrades {
    pub rules: Vec<Rule>,
    /// Per rule, the snapshots whose documents it applies to.
    pub versions: Vec<BTreeSet<usize>>,
    pub edits: Vec<XmlRule>,
    /// For each item that needs a rule: the index of the rule covering it.
    pub covered: BTreeMap<String, usize>,
}

impl Upgrades {
    pub fn rule(&self, id: &str) -> Option<&Rule> {
        self.covered.get(id).and_then(|i| self.rules.get(*i))
    }
}

/// The rule covering an item: its own, a `drop` of an enclosing item, or a
/// `drop` of the definition the item refers to.
fn covering(h: &History, rules: &BTreeMap<&str, usize>, all: &[Rule], id: &str, need: Need) -> Option<usize> {
    if let Some(i) = rules.get(id) {
        return Some(*i);
    }
    if need != Need::Gone {
        return None;
    }
    let drop = |item: &str| rules.get(item).copied().filter(|i| all[*i].action == Action::Drop);
    let enclosing = id.char_indices().filter(|(_, c)| matches!(c, '/' | '=' | '|')).find_map(|(i, _)| drop(&id[..i]));
    enclosing.or_else(|| h.shapes.iter().filter_map(|s| s.targets.get(id)).find_map(|t| drop(t)))
}

/// Checks the rules against the history and turns them into edits.
pub fn check(h: &History, rules: Vec<Rule>) -> Result<Upgrades, String> {
    let index: BTreeMap<&str, usize> = rules.iter().enumerate().map(|(i, r)| (r.item.as_str(), i)).collect();
    let needs = needs(h);
    let mut covered = BTreeMap::new();
    let mut versions = vec![BTreeSet::new(); rules.len()];
    let mut missing = Vec::new();
    for (id, (need, at)) in &needs {
        match covering(h, &index, &rules, id, *need) {
            Some(i) => {
                covered.insert(id.clone(), i);
                versions[i].extend(at);
            }
            None => missing.push(id.as_str()),
        }
    }
    let mut errors = Vec::new();
    if !missing.is_empty() {
        errors.push(format!(
            "schema-upgrades.tsv has no rule for these items of older versions (suggested rules; a drop needs a note):\n{}",
            suggest(h, &needs, &missing)
        ));
    }
    let used: BTreeSet<usize> = covered.values().copied().collect();
    let mut edits = Vec::new();
    for (i, r) in rules.iter().enumerate() {
        let result = if used.contains(&i) { expand(h, r, &needs) } else { Err("not needed: no older version differs there".to_owned()) };
        match result {
            Ok(e) => edits.extend(e),
            Err(e) => errors.push(format!("schema-upgrades.tsv:{}: {}: {e}", r.line, r.item)),
        }
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    // One edit per path, kind and rule, with all its snapshots.
    let mut merged: BTreeMap<(Vec<String>, Edit, String), XmlRule> = BTreeMap::new();
    for e in edits {
        let key = (e.path.clone(), e.edit.clone(), e.item.clone());
        match merged.get_mut(&key) {
            Some(m) => m.versions.extend(e.versions),
            None => {
                merged.insert(key, e);
            }
        }
    }
    Ok(Upgrades { rules, versions, edits: merged.into_values().collect(), covered })
}

fn suggest(h: &History, needs: &BTreeMap<String, (Need, BTreeSet<usize>)>, missing: &[&str]) -> String {
    let current = h.versions.len() - 1;
    let mut dropped: BTreeSet<&str> = BTreeSet::new();
    let mut out = Vec::new();
    for id in missing {
        let enclosed = id.char_indices().any(|(i, c)| matches!(c, '/' | '=' | '|') && dropped.contains(&id[..i]));
        let refers = h.shapes.iter().filter_map(|s| s.targets.get(*id)).any(|t| dropped.contains(t.as_str()));
        if enclosed || refers {
            continue;
        }
        let Some((need, versions)) = needs.get(*id) else { continue };
        let labels: Vec<String> = versions.iter().map(|v| h.label(*v)).collect();
        let states = &h.items[*id];
        let rule = match need {
            Need::Changed => {
                let old = versions.iter().find_map(|v| states[*v].clone()).unwrap_or_default();
                let new = states[current].clone().unwrap_or_default();
                if old.starts_with("optional") && new.starts_with("required") { "default \"\"".to_owned() } else { "keep".to_owned() }
            }
            Need::Gone => match h.moved(id, current) {
                Some(now) if !last_segment(id).starts_with('@') => {
                    let mut path: Vec<&str> = now.split('/').collect();
                    path.pop();
                    format!("wrap {}", path.join("/"))
                }
                _ => {
                    dropped.insert(id);
                    "drop".to_owned()
                }
            },
        };
        out.push(format!("{id}\t{rule}\t(in {})", labels.join(", ")));
    }
    out.join("\n")
}

/// The element paths a container stands for in snapshot `v`: element names
/// from its definition's element, or for a content definition from each
/// element that includes it.
fn patterns(h: &History, v: usize, container: &str, seen: &mut BTreeSet<String>) -> Vec<Vec<String>> {
    let mut segments = container.split('/');
    let anchor = segments.next().unwrap_or_default();
    let rest: Vec<String> = segments.map(|s| s.trim_start_matches('<').trim_end_matches('>').to_owned()).collect();
    let shape = &h.shapes[v];
    let bases = match shape.elements.get(anchor) {
        Some(element) => vec![vec![element.clone()]],
        None if seen.insert(anchor.to_owned()) => {
            let includers: Vec<&String> = shape.includes.iter().filter(|(_, defs)| defs.contains(anchor)).map(|(k, _)| k).collect();
            includers.into_iter().flat_map(|k| patterns(h, v, k, seen)).collect()
        }
        None => Vec::new(),
    };
    bases
        .into_iter()
        .map(|mut b| {
            b.extend(rest.iter().cloned());
            b
        })
        .collect()
}

fn expand(h: &History, r: &Rule, needs: &BTreeMap<String, (Need, BTreeSet<usize>)>) -> Result<Vec<XmlRule>, String> {
    let current = h.versions.len() - 1;
    let (need, versions) = needs.get(&r.item).cloned().ok_or("no older version differs there")?;
    let last = last_segment(&r.item);
    let state = |id: &str, v: usize| h.items.get(id).and_then(|s| s.get(v).cloned().flatten());
    let mut out = Vec::new();
    let mut add = |v: usize, container: &str, edit: Edit| -> Result<(), String> {
        let paths = patterns(h, v, container, &mut BTreeSet::new());
        if paths.is_empty() {
            return Err(format!("no element contains {container} in {}", h.label(v)));
        }
        for path in paths {
            let versions = BTreeSet::from([v]);
            out.push(XmlRule { versions, path, edit: edit.clone(), item: r.item.clone(), note: r.note.clone() });
        }
        Ok(())
    };
    let parent = || parent(&r.item).ok_or("not an attribute or element of a definition");
    // An enumeration literal `E="v"`: edits of each attribute that may hold it.
    let literal = r.item.split_once("=\"").filter(|(e, _)| e.ends_with("_EnumValue") && !e.contains('/'));
    match (&r.action, need) {
        (Action::Keep, Need::Changed) => {}
        (Action::Default(value), Need::Changed) => {
            let name = last.strip_prefix('@').ok_or("default needs an attribute")?;
            if !state(&r.item, current).is_some_and(|s| s.starts_with("required")) {
                return Err("default is for attributes that became required".to_owned());
            }
            for v in versions.iter().filter(|v| state(&r.item, **v).is_some_and(|s| s.contains("optional"))) {
                add(*v, &parent()?, Edit::Default(name.to_owned(), value.clone()))?;
            }
        }
        (Action::Value(_) | Action::Drop, Need::Gone) if literal.is_some() => {
            let (e, lit) = literal.ok_or("not a literal")?;
            let lit = lit.trim_end_matches('"');
            if let Action::Value(new) = &r.action
                && state(&format!("{e}={new:?}"), current).is_none()
            {
                return Err(format!("{e} has no value {new:?} now"));
            }
            for v in &versions {
                for u in h.shapes[*v].enum_uses.get(e).into_iter().flatten() {
                    let Some(now) = state(u, current) else { continue }; // gone itself: its own rule
                    let attr = last_segment(u).strip_prefix('@').ok_or_else(|| format!("{u} holds {e} as text, which upgrades do not edit"))?;
                    if now.contains("list {") {
                        return Err(format!("{u} holds {e} in a list, which upgrades do not edit"));
                    }
                    let edit = match &r.action {
                        Action::Value(new) => Edit::MapValue(attr.to_owned(), lit.to_owned(), new.clone()),
                        _ if now.starts_with("required") => return Err(format!("{u} is required now: use value")),
                        _ => Edit::DropValue(attr.to_owned(), lit.to_owned()),
                    };
                    add(*v, &parent_of(u)?, edit)?;
                }
            }
        }
        (Action::Wrap(path), Need::Gone) => {
            if !last.starts_with('<') {
                return Err("wrap needs an element".to_owned());
            }
            let mut at = parent()?;
            for name in path {
                let child = h.child_elements(current, &at).into_iter().find(|(n, _)| *n == format!("<{name}>")).map(|(_, c)| c);
                at = child.ok_or_else(|| format!("<{name}> is not a child of {at} now"))?;
            }
            if !h.child_elements(current, &at).iter().any(|(n, _)| n == last) {
                return Err(format!("{last} is not a child of {} now", angle(path)));
            }
            for v in &versions {
                add(*v, &container(&r.item), Edit::Wrap(path.clone()))?;
            }
        }
        (Action::Drop, Need::Gone) if last.starts_with('@') => {
            for v in &versions {
                add(*v, &parent()?, Edit::DropAttribute(last.trim_start_matches('@').to_owned()))?;
            }
        }
        (Action::Drop, Need::Gone) if last.starts_with('<') => {
            for v in &versions {
                add(*v, &container(&r.item), Edit::Drop)?;
            }
        }
        (Action::Drop, Need::Gone) if !r.item.contains(['/', '=', '|']) => {
            // A definition: its element, wherever it is referred to.
            for v in &versions {
                if !h.shapes[*v].elements.contains_key(&r.item) {
                    return Err("only element definitions can be dropped as a whole".to_owned());
                }
                let refs: Vec<&String> =
                    h.shapes[*v].targets.iter().filter(|(id, t)| **t == r.item && state(id, *v).is_some()).map(|(id, _)| id).collect();
                for id in refs {
                    add(*v, &container(id), Edit::Drop)?;
                }
            }
        }
        (action, need) => {
            return Err(format!("{} does not fit an item that is {}", action.describe(), if need == Need::Gone { "gone" } else { "changed" }));
        }
    }
    Ok(out)
}

fn parent_of(id: &str) -> Result<String, String> {
    parent(id).ok_or_else(|| format!("{id} has no parent"))
}

/// `(7, 5)` for `7.5`.
fn dom(h: &History, v: usize) -> String {
    let mut n = h.versions[v].dom.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
    format!("({}, {})", n.next().unwrap_or(0), n.next().unwrap_or(0))
}

/// `src/model/upgrades.rs`: the edits, each for the `DOMVersion`s between
/// the snapshots around the ones that need it.
pub fn rust(upgrades: Option<(&History, &Upgrades)>, header: &str) -> String {
    let mut out = String::new();
    out.push_str(header);
    out.push_str(
        "//! The edits that upgrade documents of older `DOMVersion`s before they are\n\
         //! read, from `schema-upgrades.tsv`. Each applies to documents after\n\
         //! `after` (any older one if `None`) and before `before`, to elements whose\n\
         //! path ends with `path`.\n\n\
         use crate::upgrade::Edit::*;\nuse crate::upgrade::Rule;\n\npub(crate) const RULES: &[Rule] = &[\n",
    );
    let Some((h, upgrades)) = upgrades else {
        out.push_str("];\n");
        return out;
    };
    for e in &upgrades.edits {
        let indices: Vec<usize> = e.versions.iter().copied().collect();
        // Runs of consecutive snapshots.
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for i in indices {
            match runs.last_mut() {
                Some((_, end)) if *end + 1 == i => *end = i,
                _ => runs.push((i, i)),
            }
        }
        let edit = match &e.edit {
            Edit::Drop => "Drop".to_owned(),
            Edit::Wrap(names) => format!("Wrap(&{names:?})"),
            Edit::DropAttribute(a) => format!("DropAttribute({a:?})"),
            Edit::DropValue(a, v) => format!("DropValue({a:?}, {v:?})"),
            Edit::MapValue(a, v, w) => format!("MapValue({a:?}, {v:?}, {w:?})"),
            Edit::Default(a, v) => format!("Default({a:?}, {v:?})"),
        };
        let _ = writeln!(out, "    // {}", e.item);
        for (start, end) in runs {
            let after = start.checked_sub(1).map_or("None".to_owned(), |p| format!("Some({})", dom(h, p)));
            let _ =
                writeln!(out, "    Rule {{ after: {after}, before: {}, path: &{:?}, edit: {edit}, why: {:?} }},", dom(h, end + 1), e.path, e.note);
        }
    }
    out.push_str("];\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{Snapshot, Version, Walker};
    use crate::rnc::{self, Ids, Pattern};

    fn snapshot(dom: &str, text: &str) -> Snapshot {
        let f = rnc::parse(text, &mut Ids::default()).unwrap();
        let defs: BTreeMap<&str, &Pattern> = f.defines.iter().map(|d| (d.name.as_str(), &d.pattern)).collect();
        let (items, shape) = Walker::run_with_shape(&defs, &["A_Object"]);
        let items = items.into_iter().map(|i| (i.id, i.fingerprint)).collect();
        let version = Version {
            dir: dom.to_owned(),
            dom: dom.to_owned(),
            build: None,
            product: None,
            same_as: Vec::new(),
            single: false,
            sha256: String::new(),
        };
        Snapshot { version, items, shape }
    }

    /// In 8.0, `<B>` moved into `<W>`, `@Gone` is gone and `@X` became required.
    fn history() -> History {
        let old = "grammar { A_Object = element A { attribute Gone { text }?, attribute X { text }?, B_Object* }\n\
                   B_Object = element B { empty } }";
        let new = "grammar { A_Object = element A { attribute X { text }, W_Object* }\n\
                   W_Object = element W { B_Object* }\nB_Object = element B { empty } }";
        History::new(vec![snapshot("7.0", old), snapshot("8.0", new)])
    }

    #[test]
    fn missing_rules_are_suggested() {
        let e = check(&history(), Vec::new()).unwrap_err();
        assert!(e.contains("A_Object/<B>\twrap <W>\t(in 7.0)"), "{e}");
        assert!(e.contains("A_Object/@Gone\tdrop\t(in 7.0)"), "{e}");
        assert!(e.contains("A_Object/@X\tdefault \"\"\t(in 7.0)"), "{e}");
    }

    #[test]
    fn rules_become_edits() {
        let rules = "item\trule\tnote\nA_Object/<B>\twrap <W>\nA_Object/@Gone\tdrop\tnot used\nA_Object/@X\tdefault \"x\"\n";
        let u = check(&history(), parse(rules).unwrap()).unwrap();
        let edits: Vec<(Vec<String>, Edit)> = u.edits.into_iter().map(|e| (e.path, e.edit)).collect();
        let a = vec!["A".to_owned()];
        assert_eq!(
            edits,
            [
                (a.clone(), Edit::DropAttribute("Gone".to_owned())),
                (a.clone(), Edit::Default("X".to_owned(), "x".to_owned())),
                (vec!["A".to_owned(), "B".to_owned()], Edit::Wrap(vec!["W".to_owned()])),
            ]
        );
    }

    #[test]
    fn wrong_or_unneeded_rules_fail() {
        let base = "A_Object/@Gone\tdrop\tnot used\nA_Object/@X\tdefault \"x\"\n";
        let e = check(&history(), parse(&format!("{base}A_Object/<B>\twrap <Q>\n")).unwrap()).unwrap_err();
        assert!(e.contains("<Q> is not a child of A_Object now"), "{e}");
        let e = check(&history(), parse(&format!("{base}A_Object/<B>\twrap <W>\nB_Object\tkeep\n")).unwrap()).unwrap_err();
        assert!(e.contains("B_Object: not needed"), "{e}");
        assert!(parse("A_Object/@Gone\tdrop\n").unwrap_err().contains("say why"));
    }
}
