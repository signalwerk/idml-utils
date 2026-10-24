//! When each schema item entered IDML, by comparing schema snapshots.
//!
//! Every schema item has an ID from where the schema text defines it:
//!
//! | ID | Item |
//! |---|---|
//! | `TextFrame_Object` | definition |
//! | `TextFrame_Object/@FillColor` | attribute |
//! | `TextFrame_Object/<Properties>` | child element, inline or by reference |
//! | `TextFrame_Object/<Properties>/<Label>` | element nested in an inline element |
//! | `AnchorPoint_EnumValue="TopLeftAnchor"` | enumeration literal |
//! | `…/<AppliedFont>\|type=object xsd:string` | choice branch |
//! | `…/<Contents>/text()` | text content |
//!
//! A snapshot lists the items of each part grammar with a fingerprint of
//! what can change about them (an attribute's type, a child's occurrence).
//! Snapshots are compared in `DOMVersion` order. A single-file schema (the
//! only form of 6.0) has no package parts, so it does not count for the
//! designmap or the part roots.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use crate::lower::describe;
use crate::rnc::{Ids, Kind, NameClass, Pattern};
use crate::schema::Files;

/// The items of one schema snapshot.
#[derive(Debug)]
pub struct Snapshot {
    pub version: Version,
    pub items: BTreeMap<String, String>,
    pub shape: Shape,
}

/// How the items of a snapshot fit together: what move hints and upgrades
/// need beyond the item IDs.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Shape {
    /// The definition each reference item refers to:
    /// `Story_Object/<TextFrame>` → `TextFrame_Object`.
    pub targets: BTreeMap<String, String>,
    /// The element name of each element definition.
    pub elements: BTreeMap<String, String>,
    /// The content definitions each container (a definition or an inline
    /// element, without choice branches) includes.
    pub includes: BTreeMap<String, BTreeSet<String>>,
    /// The attribute and text items whose value may be a literal of each
    /// enumeration.
    pub enum_uses: BTreeMap<String, BTreeSet<String>>,
}

impl Shape {
    fn merge(&mut self, other: Shape) {
        self.targets.extend(other.targets);
        self.elements.extend(other.elements);
        for (k, v) in other.includes {
            self.includes.entry(k).or_default().extend(v);
        }
        for (k, v) in other.enum_uses {
            self.enum_uses.entry(k).or_default().extend(v);
        }
    }
}

/// An item ID without its choice branches: the container an item is in.
pub fn container(id: &str) -> String {
    id.split('/').map(|s| s.split('|').next().unwrap_or(s)).collect::<Vec<_>>().join("/")
}

/// The last segment of an item ID (`<TextFrame>`, `@Name`), without a branch.
pub fn last_segment(id: &str) -> &str {
    let last = id.rsplit('/').next().unwrap_or(id);
    last.split('|').next().unwrap_or(last)
}

/// The container of an item: its ID without the last segment.
pub fn parent(id: &str) -> Option<String> {
    let i = id.rfind('/')?;
    Some(container(&id[..i]))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    /// The schema directory, relative to the history root (`7.0.3.535/package`).
    pub dir: String,
    pub dom: String,
    /// The InDesign build, when the files name it.
    pub build: Option<String>,
    /// The product, such as `CC 2014`, when the files name it.
    pub product: Option<String>,
    /// Other builds of the same `DOMVersion` with exactly the same items.
    pub same_as: Vec<String>,
    /// A single-file schema rather than package parts.
    pub single: bool,
    pub sha256: String,
}

/// Every snapshot below `root`: `<name>/package`, or `<name>/single` when
/// there is no package form.
pub fn snapshots(root: &Path) -> Result<Vec<Snapshot>, String> {
    let mut out = Vec::new();
    let mut names: Vec<_> = fs::read_dir(root).map_err(|e| format!("{}: {e}", root.display()))?.flatten().map(|e| e.path()).collect();
    names.sort();
    for dir in names.iter().filter(|p| p.is_dir()) {
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        for form in ["package", "single"] {
            if dir.join(form).is_dir() {
                out.push(snapshot(&dir.join(form), &format!("{name}/{form}"))?);
                break;
            }
        }
    }
    out.sort_by_key(|s| order(&s.version));
    Ok(out)
}

fn numbers(s: &str) -> Vec<u64> {
    s.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

fn order(v: &Version) -> (Vec<u64>, Vec<u64>) {
    (numbers(&v.dom), v.build.as_deref().map(numbers).unwrap_or_default())
}

pub fn snapshot(dir: &Path, name: &str) -> Result<Snapshot, String> {
    let files = Files::read(dir, &mut Ids::default())?;
    let mut items: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut shape = Shape::default();
    let mut doms = BTreeSet::new();
    for g in files.grammars()? {
        let defs: BTreeMap<&str, &Pattern> = g.defs.iter().map(|(n, (_, d))| (*n, &d.pattern)).collect();
        let (found, found_shape) = Walker::run_with_shape(&defs, &[g.start]);
        for item in found {
            items.entry(item.id).or_default().insert(item.fingerprint);
        }
        shape.merge(found_shape);
        if let Some(Kind::Element(_, content)) = defs.get(g.start).map(|p| &p.kind) {
            content.walk(&mut |p| {
                if let Kind::Attribute(NameClass::Name(n), v) = &p.kind
                    && n.local == "DOMVersion"
                    && let Kind::Value { value, .. } = &v.kind
                {
                    doms.insert(value.clone());
                }
            });
        }
    }
    let dom = match doms.len() {
        1 => doms.into_iter().next().unwrap_or_default(),
        _ => return Err(format!("{}: expected one DOMVersion, found {doms:?}", dir.display())),
    };
    let version = Version {
        dir: name.to_owned(),
        dom,
        build: files.indesign_version.clone(),
        product: files.product.clone().or_else(|| manifest_product(dir)),
        same_as: Vec::new(),
        single: name.ends_with("/single"),
        sha256: files.sha256,
    };
    // An item the grammars define differently keeps all fingerprints.
    let items = items.into_iter().map(|(id, fps)| (id, fps.into_iter().collect::<Vec<_>>().join(" / "))).collect();
    Ok(Snapshot { version, items, shape })
}

/// The `indesignVersion` of the snapshot's `manifest.json`, for schemas
/// whose files do not name the product.
fn manifest_product(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.parent()?.join("manifest.json")).ok()?;
    let value = text.lines().find_map(|l| l.trim().strip_prefix("\"indesignVersion\": \""))?;
    value.strip_suffix("\",").or(value.strip_suffix('"')).map(str::to_owned)
}

/// An item of a grammar.
#[derive(Debug)]
pub struct Item {
    /// The pattern node it comes from.
    pub node: u32,
    pub id: String,
    pub fingerprint: String,
}

/// Walks the definitions reachable from `starts`.
pub struct Walker<'p, 'd> {
    defs: &'d BTreeMap<&'d str, &'p Pattern>,
    out: Vec<Item>,
    shape: Shape,
}

impl<'p, 'd> Walker<'p, 'd> {
    pub fn run(defs: &'d BTreeMap<&'d str, &'p Pattern>, starts: &[&str]) -> Vec<Item> {
        Self::run_with_shape(defs, starts).0
    }

    pub fn run_with_shape(defs: &'d BTreeMap<&'d str, &'p Pattern>, starts: &[&str]) -> (Vec<Item>, Shape) {
        let mut w = Walker { defs, out: Vec::new(), shape: Shape::default() };
        let mut seen = BTreeSet::new();
        let mut stack: Vec<String> = starts.iter().map(|s| (*s).to_owned()).collect();
        while let Some(name) = stack.pop() {
            let Some(p) = defs.get(name.as_str()) else { continue };
            if !seen.insert(name.clone()) {
                continue;
            }
            p.walk(&mut |q| {
                if let Kind::Ref(r) = &q.kind {
                    stack.push(r.clone());
                }
            });
            w.def(&name, p);
        }
        (w.out, w.shape)
    }

    fn push(&mut self, node: u32, id: String, fingerprint: String) {
        self.out.push(Item { node, id, fingerprint });
    }

    fn def(&mut self, name: &str, p: &Pattern) {
        match &p.kind {
            Kind::Element(n, content) => {
                self.shape.elements.insert(name.to_owned(), name_of(n));
                self.push(p.id, name.to_owned(), format!("element {}", name_of(n)));
                self.walk(name, "", content, "1", false);
            }
            Kind::Attribute(..) => {} // a literal attribute such as `object_type`: part of its users
            Kind::Choice(ps) if name.ends_with("_EnumValue") => {
                self.push(p.id, name.to_owned(), "enumeration".to_owned());
                for v in ps {
                    if let Kind::Value { value, .. } = &v.kind {
                        self.push(v.id, format!("{name}={value:?}"), String::new());
                    }
                }
            }
            Kind::Value { value, .. } if name.ends_with("_EnumValue") => {
                self.push(p.id, name.to_owned(), "enumeration".to_owned());
                self.push(p.id, format!("{name}={value:?}"), String::new());
            }
            _ if self.is_value(p) => self.push(p.id, name.to_owned(), format!("value {}", describe(p))),
            _ => {
                self.push(p.id, name.to_owned(), "content".to_owned());
                self.walk(name, "", p, "1", false);
            }
        }
    }

    fn element_def(&self, r: &str) -> Option<String> {
        match self.defs.get(r).map(|p| &p.kind) {
            Some(Kind::Element(n, _)) => Some(name_of(n)),
            _ => None,
        }
    }

    /// Records the enumerations a value pattern may take literals of.
    fn uses(&mut self, id: &str, p: &Pattern) {
        let mut enums = BTreeSet::new();
        self.enums_in(p, &mut enums, &mut BTreeSet::new());
        for e in enums {
            self.shape.enum_uses.entry(e).or_default().insert(id.to_owned());
        }
    }

    fn enums_in(&self, p: &Pattern, out: &mut BTreeSet<String>, seen: &mut BTreeSet<String>) {
        match &p.kind {
            Kind::Ref(r) if r.ends_with("_EnumValue") => {
                out.insert(r.clone());
            }
            Kind::Ref(r) => {
                if let Some(d) = self.defs.get(r.as_str()).filter(|d| self.is_value(d))
                    && seen.insert(r.clone())
                {
                    self.enums_in(d, out, seen);
                }
            }
            Kind::Choice(ps) | Kind::Group(ps) | Kind::Interleave(ps) => ps.iter().for_each(|q| self.enums_in(q, out, seen)),
            Kind::Optional(q) | Kind::ZeroOrMore(q) | Kind::OneOrMore(q) | Kind::List(q) => self.enums_in(q, out, seen),
            _ => {}
        }
    }

    fn is_value(&self, p: &Pattern) -> bool {
        match &p.kind {
            Kind::Text | Kind::Data { .. } | Kind::Value { .. } | Kind::List(_) => true,
            Kind::Choice(ps) => ps.iter().all(|q| self.is_value(q)),
            Kind::Ref(r) => r.ends_with("_EnumValue") || self.defs.get(r.as_str()).is_some_and(|d| self.is_value(d)),
            _ => false,
        }
    }

    /// `branch` suppresses text items: inside a choice branch the branch
    /// item stands for its value.
    fn walk(&mut self, def: &str, path: &str, p: &Pattern, occurs: &str, branch: bool) {
        match &p.kind {
            Kind::Element(n, content) => {
                let here = format!("{path}/<{}>", name_of(n));
                // An element's own content is no longer the branch's value.
                self.push(p.id, format!("{def}{here}"), occurs.to_owned());
                self.walk(def, &here, content, "1", false);
            }
            Kind::Attribute(n, v) => {
                let required = if occurs == "?" { "optional" } else { "required" };
                let value = if name_of(n) == "DOMVersion" { "the DOMVersion".to_owned() } else { describe(v) };
                let id = format!("{def}{path}/@{}", name_of(n));
                self.uses(&id, v);
                self.push(p.id, id, format!("{required} {value}"));
            }
            Kind::Ref(r) => {
                if let Some(element) = self.element_def(r) {
                    let id = format!("{def}{path}/<{element}>");
                    self.shape.targets.insert(id.clone(), r.clone());
                    self.push(p.id, id, occurs.to_owned());
                } else if self.is_value(p) {
                    if !branch {
                        let id = format!("{def}{path}/text()");
                        self.uses(&id, p);
                        self.push(p.id, id, r.clone());
                    }
                } else if self.defs.contains_key(r.as_str()) {
                    self.shape.includes.entry(container(&format!("{def}{path}"))).or_default().insert(r.clone());
                }
            }
            Kind::Choice(ps) if !self.is_value(p) => {
                let mut keys: Vec<String> = Vec::new();
                for b in ps {
                    let mut key = self.branch_key(b);
                    if keys.contains(&key) {
                        key = format!("{key} #{}", keys.len() + 1);
                    }
                    let here = format!("{path}|{key}");
                    let id = format!("{def}{here}");
                    self.uses(&id, b);
                    self.push(b.id, id, String::new());
                    keys.push(key);
                    self.walk(def, &here, b, "1", true);
                }
            }
            Kind::Group(ps) | Kind::Interleave(ps) => {
                for q in ps {
                    self.walk(def, path, q, "1", branch);
                }
            }
            Kind::Optional(q) => self.walk(def, path, q, "?", branch),
            Kind::ZeroOrMore(q) => self.walk(def, path, q, "*", branch),
            Kind::OneOrMore(q) => self.walk(def, path, q, "+", branch),
            Kind::Text | Kind::Data { .. } | Kind::List(_) | Kind::Value { .. } | Kind::Choice(_) if !branch => {
                let id = format!("{def}{path}/text()");
                self.uses(&id, p);
                self.push(p.id, id, describe(p));
            }
            _ => {}
        }
    }

    /// A choice branch by its `type` tag (`type=object xsd:string`), or its
    /// pattern.
    fn branch_key(&self, b: &Pattern) -> String {
        let mut items = Vec::new();
        flatten(b, &mut items);
        let mut tag = None;
        let mut value = None;
        for q in &items {
            let attr = match &q.kind {
                Kind::Ref(r) => self.defs.get(r.as_str()).copied(),
                _ => Some(*q),
            };
            match attr.map(|a| &a.kind) {
                Some(Kind::Attribute(n, v)) if name_of(n) == "type" => {
                    if let Kind::Value { value, .. } = &v.kind {
                        tag = Some(value.clone());
                    }
                }
                _ if value.is_none() && self.is_value(q) => value = Some(describe(q)),
                _ => {}
            }
        }
        match (tag, value) {
            (Some(t), Some(v)) => format!("type={t} {v}"),
            (Some(t), None) => format!("type={t}"),
            _ => describe(b),
        }
    }
}

fn flatten<'p>(p: &'p Pattern, out: &mut Vec<&'p Pattern>) {
    if let Kind::Group(ps) = &p.kind {
        for q in ps {
            flatten(q, out);
        }
    } else {
        out.push(p);
    }
}

fn name_of(n: &NameClass) -> String {
    match n {
        NameClass::Name(q) => q.qualified(),
        NameClass::Any => "*".to_owned(),
    }
}

// -- comparison --------------------------------------------------------------------

/// The snapshots and, per item, its fingerprint in each (absent: `None`).
#[derive(Debug)]
pub struct History {
    pub versions: Vec<Version>,
    pub items: BTreeMap<String, Vec<Option<String>>>,
    pub shapes: Vec<Shape>,
    graphs: Vec<Graph>,
}

/// The children of each container: `<Name>` or `@Name`, and for elements the
/// container they are.
type Graph = BTreeMap<String, Vec<(String, Option<String>)>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Changed,
    Removed,
    Restored,
}

impl History {
    pub fn new(snapshots: Vec<Snapshot>) -> History {
        // Builds of one DOMVersion that agree item for item count once.
        let mut merged: Vec<Snapshot> = Vec::new();
        for s in snapshots {
            match merged.last_mut() {
                Some(last) if last.version.dom == s.version.dom && last.items == s.items => {
                    let mut builds = std::mem::take(&mut last.version.same_as);
                    builds.extend(last.version.build.take());
                    *last = s;
                    last.version.same_as = builds;
                }
                _ => merged.push(s),
            }
        }
        let snapshots = merged;
        let n = snapshots.len();
        let mut items: BTreeMap<String, Vec<Option<String>>> = BTreeMap::new();
        let mut versions = Vec::new();
        let mut shapes = Vec::new();
        let mut graphs = Vec::new();
        for (i, s) in snapshots.into_iter().enumerate() {
            let mut graph = Graph::new();
            for id in s.items.keys() {
                let (Some(parent), last) = (parent(id), last_segment(id)) else { continue };
                let child = match last.chars().next() {
                    Some('<') => Some(s.shape.targets.get(id).cloned().unwrap_or_else(|| container(id))),
                    Some('@') => None,
                    _ => continue,
                };
                graph.entry(parent).or_default().push((last.to_owned(), child));
            }
            for (id, fp) in s.items {
                items.entry(id).or_insert_with(|| vec![None; n])[i] = Some(fp);
            }
            versions.push(s.version);
            shapes.push(s.shape);
            graphs.push(graph);
        }
        History { versions, items, shapes, graphs }
    }

    /// The child elements of a container in snapshot `at`, through included
    /// content definitions: `<Name>` and the child's container.
    pub fn child_elements(&self, at: usize, container: &str) -> Vec<(String, String)> {
        let (Some(graph), Some(shape)) = (self.graphs.get(at), self.shapes.get(at)) else { return Vec::new() };
        let mut out = Vec::new();
        let mut stack = vec![container.to_owned()];
        let mut seen = BTreeSet::new();
        while let Some(c) = stack.pop() {
            if !seen.insert(c.clone()) {
                continue;
            }
            stack.extend(shape.includes.get(&c).into_iter().flatten().cloned());
            for (name, child) in graph.get(&c).into_iter().flatten() {
                if let Some(child) = child {
                    out.push((name.clone(), child.clone()));
                }
            }
        }
        out
    }

    /// Where an element or attribute that snapshot `at` no longer has in its
    /// parent is still allowed there, nested deeper: `<CharacterStyleRange>/<TextFrame>`
    /// for a `<TextFrame>` that was a direct child of `<Story>`. An attribute
    /// counts as moved when it became an element (`<Properties>/<Name>`).
    /// Only elements without an identity (`@Self`) count as nesting: inside
    /// another object it would belong to that object.
    pub fn moved(&self, id: &str, at: usize) -> Option<String> {
        let (graph, shape) = (self.graphs.get(at)?, self.shapes.get(at)?);
        let start = parent(id)?;
        if !graph.contains_key(&start) && !shape.includes.contains_key(&start) {
            return None;
        }
        let last = last_segment(id);
        let (wanted, shortest, longest) = match last.strip_prefix('@') {
            Some(name) => (format!("<{name}>"), 1, 2),
            None => (last.to_owned(), 2, 3),
        };
        // An element must stay an element of the same definition.
        let base = |def: &str| def.trim_end_matches(|c: char| c.is_ascii_digit()).to_owned();
        let target = (0..at).rev().find_map(|i| self.shapes[i].targets.get(id)).map(|t| base(t));
        let mut queue = std::collections::VecDeque::from([(start, Vec::<String>::new())]);
        let mut seen = BTreeSet::new();
        while let Some((at_container, path)) = queue.pop_front() {
            if !seen.insert(at_container.clone()) {
                continue;
            }
            for inc in shape.includes.get(&at_container).into_iter().flatten() {
                queue.push_front((inc.clone(), path.clone()));
            }
            for (name, child) in graph.get(&at_container).into_iter().flatten() {
                let Some(child) = child else { continue };
                let mut here = path.clone();
                here.push(name.clone());
                let same = *name == wanted && (target.is_none() || target.as_deref() == Some(base(child).as_str()));
                if same && here.len() >= shortest {
                    return Some(here.join("/"));
                }
                let object = graph.get(child).is_some_and(|c| c.iter().any(|(n, _)| n == "@Self"));
                if here.len() < longest && !object {
                    queue.push_back((child.clone(), here));
                }
            }
        }
        None
    }

    /// The single-file form has no package parts, so it says nothing about
    /// the designmap and the part roots; it also reaches the `anything`
    /// wildcard, for embedded XMP. Everything else it describes exactly as
    /// the package form does (checked with build 21.5.1.73, generated in both
    /// forms by `scripts/generate-idml-schema`).
    pub fn covers(&self, i: usize, id: &str) -> bool {
        let anchor = id.split(['/', '=', '|']).next().unwrap_or(id);
        !(self.versions[i].single && (anchor == "Document_Object" || anchor.ends_with("_File") || anchor == "anything"))
    }

    /// How a version is named: `7.0`, or with the build where several
    /// snapshots share a `DOMVersion`.
    pub fn label(&self, i: usize) -> String {
        let v = &self.versions[i];
        let shared = self.versions.iter().filter(|w| w.dom == v.dom).count() > 1;
        match (&v.build, shared) {
            (Some(b), true) => format!("{} ({b})", v.dom),
            _ => v.dom.clone(),
        }
    }

    /// When something happened, between the snapshot `at` and the covering
    /// snapshot before it: `7.5` when no major version lies between them,
    /// `> 11.0 <= 15.0` when versions are missing (12.0 to 14.0), and
    /// `<= 7.0` when no earlier snapshot covers the item.
    pub fn span(&self, at: usize, previous: Option<usize>) -> String {
        match previous {
            // 6.0 is the first IDML version.
            None if self.versions[at].dom == "6.0" => self.label(at),
            None => format!("<= {}", self.label(at)),
            Some(p) if self.gap(p, at) => format!("> {} <= {}", self.label(p), self.label(at)),
            Some(_) => self.label(at),
        }
    }

    /// Whether a major version lies between two snapshots: 8.0 and 10.0
    /// miss 9.0; 7.0, 7.5 and 8.0 miss nothing.
    pub fn gap(&self, a: usize, b: usize) -> bool {
        let major = |i: usize| numbers(&self.versions[i].dom).first().copied().unwrap_or(0);
        major(b) > major(a) + 1
    }

    /// The first snapshot with the item, and the covering snapshot before it.
    pub fn since(&self, id: &str) -> Option<(usize, Option<usize>)> {
        let states = self.items.get(id)?;
        let mut previous = None;
        for (i, state) in states.iter().enumerate() {
            if !self.covers(i, id) {
                continue;
            }
            if state.is_some() {
                return Some((i, previous));
            }
            previous = Some(i);
        }
        None
    }

    /// What happened to the item after it appeared: in which snapshot, the
    /// covering snapshot before it, and what.
    pub fn events(&self, id: &str) -> Vec<(usize, Option<usize>, Event)> {
        let Some(states) = self.items.get(id) else { return Vec::new() };
        let mut out = Vec::new();
        let mut last: Option<(usize, &Option<String>)> = None;
        for (i, state) in states.iter().enumerate() {
            if !self.covers(i, id) {
                continue;
            }
            let previous = last.map(|(p, _)| p);
            match (last.map(|(_, s)| s), state) {
                (Some(Some(a)), Some(b)) if a != b => out.push((i, previous, Event::Changed)),
                (Some(Some(_)), None) => out.push((i, previous, Event::Removed)),
                (Some(None), Some(_)) if states[..i].iter().any(Option::is_some) => out.push((i, previous, Event::Restored)),
                _ => {}
            }
            last = Some((i, state));
        }
        out
    }

    /// `Since IDML <= 7.0; changed in > 11.0 <= 15.0.` for the items behind
    /// one Rust item: the earliest appearance, and every later change.
    pub fn describe(&self, ids: &[&str]) -> Option<String> {
        let (first, previous) = ids.iter().filter_map(|id| self.since(id)).min_by_key(|(f, _)| *f)?;
        let mut text = format!("Since IDML {}", self.span(first, previous));
        let mut changed = BTreeSet::new();
        let mut gaps = BTreeSet::new();
        for id in ids {
            for (i, previous, e) in self.events(id) {
                match e {
                    Event::Changed => changed.insert((i, previous)),
                    Event::Removed => gaps.insert(i),
                    Event::Restored => false,
                };
            }
        }
        if !changed.is_empty() {
            let v: Vec<String> = changed.iter().map(|(i, p)| self.span(*i, *p)).collect();
            let _ = write!(text, "; changed in {}", v.join(", "));
        }
        if !gaps.is_empty() && ids.iter().all(|id| self.items.get(*id).and_then(|s| s.last()).is_some_and(Option::is_some)) {
            let v: Vec<String> = gaps.iter().map(|i| self.label(*i)).collect();
            let _ = write!(text, "; not in {}", v.join(", "));
        }
        text.push('.');
        Some(text)
    }

    /// The history file: every item of every snapshot.
    pub fn tsv(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# IDML schema history: when each schema item appeared, changed and was removed.");
        let _ = writeln!(out, "# @generated by idml-codegen from these snapshots, oldest first. Do not edit.");
        for (i, v) in self.versions.iter().enumerate() {
            let form = if v.single { "single-file schema: no designmap or part roots" } else { "package schemas" };
            let same = if v.same_as.is_empty() { String::new() } else { format!(" (the same items as {})", v.same_as.join(", ")) };
            let build = v.build.as_deref().unwrap_or("unknown");
            let _ = writeln!(out, "# {}  DOMVersion {}, build {build}{same}, {form}, schemas/{}", v.sha256, self.label(i), v.dir);
        }
        let undated = self.items.keys().filter(|id| self.since(id).is_none()).count();
        let _ = writeln!(out, "# Not listed: {undated} items only of the single-file form (its designmap, part roots and XMP wildcard).");
        let _ = writeln!(
            out,
            "# Versions: 7.5 = in 7.5, not in the snapshot before; > 11.0 <= 15.0 = after 11.0, by 15.0 (no snapshot in between); <= 7.0 = in 7.0, nothing earlier says."
        );
        let _ = writeln!(
            out,
            "# moved in 7.0 to <A>/<B>: no longer allowed in its parent, but still nested in it in elements without an identity (no @Self)."
        );
        let _ = writeln!(out, "item\tsince\thistory\tdefinition");
        for (id, states) in &self.items {
            let Some((first, previous)) = self.since(id) else { continue };
            let since = self.span(first, previous);
            let events: Vec<String> = self
                .events(id)
                .into_iter()
                .map(|(i, previous, e)| {
                    let span = self.span(i, previous);
                    match e {
                        Event::Changed => format!("changed in {span}"),
                        Event::Removed => match self.moved(id, i) {
                            Some(now) => format!("moved in {span} to {now}"),
                            None => format!("removed in {span}"),
                        },
                        Event::Restored => format!("restored in {span}"),
                    }
                })
                .collect();
            let last = states.iter().rev().flatten().next().map_or("", String::as_str);
            let clean = |s: &str| s.replace(['\t', '\n'], " ");
            let _ = writeln!(out, "{}\t{since}\t{}\t{}", clean(id), events.join(", "), clean(last));
        }
        out
    }
}

/// Fills in each trace's items and history. Fails when a schema item behind
/// the model has no ID, or when the newest snapshot is not the target.
pub fn annotate(model: &mut crate::model::Model, schema: &crate::schema::Schema, history: &History) -> Result<(), String> {
    match history.versions.last() {
        Some(v) if v.sha256 == schema.sha256 => {}
        _ => return Err("the newest schema snapshot must be the schemas the model is generated from".to_owned()),
    }
    let items = node_items(schema);
    let mut missing = BTreeSet::new();
    for t in &mut model.types {
        let name = t.name.clone();
        crate::lower::for_each_trace(t, &mut |trace| {
            let mut ids: Vec<String> = trace.nodes.iter().filter_map(|n| items.get(n).cloned()).collect();
            ids.sort();
            ids.dedup();
            if trace.nodes.iter().any(|n| !items.contains_key(n)) {
                missing.insert(name.clone());
            }
            let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            trace.since = history.describe(&refs);
            if trace.since.is_none() && !ids.is_empty() {
                missing.insert(name.clone());
            }
            trace.items = ids;
        });
    }
    if missing.is_empty() { Ok(()) } else { Err(format!("model items without a schema history: {missing:?}")) }
}

/// The item of each pattern node of the target schema.
pub fn node_items(schema: &crate::schema::Schema) -> HashMap<u32, String> {
    let defs: BTreeMap<&str, &Pattern> = schema.defs.iter().map(|(n, d)| (n.as_str(), &d.pattern)).collect();
    let starts: Vec<&str> = schema.entries.iter().map(|e| e.start.as_str()).collect();
    Walker::run(&defs, &starts).into_iter().map(|i| (i.node, i.id)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rnc;

    fn snapshot(dom: &str, single: bool, text: &str) -> Snapshot {
        let f = rnc::parse(text, &mut Ids::default()).unwrap();
        let defs: BTreeMap<&str, &Pattern> = f.defines.iter().map(|d| (d.name.as_str(), &d.pattern)).collect();
        let (items, shape) = Walker::run_with_shape(&defs, &["A_Object"]);
        let items = items.into_iter().map(|i| (i.id, i.fingerprint)).collect();
        let version =
            Version { dir: dom.to_owned(), dom: dom.to_owned(), build: None, product: None, same_as: Vec::new(), single, sha256: String::new() };
        Snapshot { version, items, shape }
    }

    #[test]
    fn items_appear_change_and_disappear() {
        let old = "grammar { A_Object = element A { attribute X { xsd:string }?, attribute Gone { text }?, element P { B_EnumValue }? }\n\
                   B_EnumValue = string \"one\" }";
        let new = "grammar { A_Object = element A { attribute X { xsd:double }?, attribute Y { text }?, element P { B_EnumValue }? }\n\
                   B_EnumValue = string \"one\" | string \"two\" }";
        let h = History::new(vec![snapshot("7.0", false, old), snapshot("8.0", false, new)]);
        assert_eq!(h.describe(&["A_Object/@X"]).as_deref(), Some("Since IDML <= 7.0; changed in 8.0."));
        assert_eq!(h.describe(&["A_Object/@Y"]).as_deref(), Some("Since IDML 8.0."));
        assert_eq!(h.describe(&["B_EnumValue=\"two\""]).as_deref(), Some("Since IDML 8.0."));
        assert_eq!(h.describe(&["A_Object/<P>/text()"]).as_deref(), Some("Since IDML <= 7.0."));
        assert_eq!(h.events("A_Object/@Gone"), [(1, Some(0), Event::Removed)]);

        // Versions missing in between: 12.0 to 14.0.
        let h = History::new(vec![snapshot("11.0", false, old), snapshot("15.0", false, new)]);
        assert_eq!(h.describe(&["A_Object/@X"]).as_deref(), Some("Since IDML <= 11.0; changed in > 11.0 <= 15.0."));
        assert_eq!(h.describe(&["A_Object/@Y"]).as_deref(), Some("Since IDML > 11.0 <= 15.0."));
    }

    #[test]
    fn single_file_schemas_do_not_count_for_package_items() {
        let a = "grammar { A_Object = element A { attribute X { text }? } }";
        let h = History::new(vec![snapshot("6.0", true, a), snapshot("7.0", false, a)]);
        assert_eq!(h.describe(&["A_Object/@X"]).as_deref(), Some("Since IDML 6.0."));
        let mut h = h;
        h.items.insert("Story_File/@Y".to_owned(), vec![None, Some(String::new())]);
        assert_eq!(h.describe(&["Story_File/@Y"]).as_deref(), Some("Since IDML <= 7.0."));
    }
}
