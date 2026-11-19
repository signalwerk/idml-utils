//! Loads a directory of InDesign package schemas.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::rnc::{self, Ids, Kind, NameClass, Pattern, QName};
use crate::rng;

/// The package schemas of one InDesign version.
#[derive(Debug)]
pub struct Schema {
    pub dir: PathBuf,
    /// The InDesign build that generated the schemas, e.g. `21.5.0.64`.
    pub indesign_version: String,
    /// SHA-256 over all schema files (see [`Schema::files`]).
    pub sha256: String,
    /// Every schema file, by path relative to `dir`, with its SHA-256.
    pub files: BTreeMap<String, String>,
    /// Comment lines without `#` that the parser tolerated, by file.
    pub stray_lines: BTreeMap<String, Vec<usize>>,
    /// The package parts: one per file with a `start` pattern.
    pub entries: Vec<Entry>,
    /// All definitions by name. A name defined in several files must have
    /// the same pattern in each.
    pub defs: BTreeMap<String, Def>,
    /// Names of the definitions reachable from an entry's start pattern.
    pub reachable: BTreeSet<String>,
    /// Added pattern nodes and their amendment IDs (upstream hashes stay unchanged).
    pub amendment_nodes: BTreeMap<u32, String>,
}

/// A package part's grammar.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Path of the schema file relative to the schema directory.
    pub file: String,
    /// The definition that `start` refers to.
    pub start: String,
    /// The root element of the part.
    pub root: QName,
}

#[derive(Debug, Clone)]
pub struct Def {
    pub pattern: Pattern,
    pub doc: Vec<String>,
    /// The files whose text defines it.
    pub defined_in: BTreeSet<String>,
}

/// The parsed `.rnc` or `.rng` files of one schema directory.
#[derive(Debug)]
pub struct Files {
    pub dir: PathBuf,
    pub parsed: BTreeMap<String, rnc::File>,
    /// SHA-256 of each file, by path relative to `dir`.
    pub hashes: BTreeMap<String, String>,
    /// SHA-256 over all files (path and SHA-256 of each).
    pub sha256: String,
    /// The InDesign build named by the files' header comments, if they name one.
    pub indesign_version: Option<String>,
    /// The product the headers name, such as `CC 2014`.
    pub product: Option<String>,
    pub stray_lines: BTreeMap<String, Vec<usize>>,
    pub amendment_nodes: BTreeMap<u32, String>,
}

/// One part grammar: a file with a `start`, and the definitions in scope.
#[derive(Debug)]
pub struct Grammar<'f> {
    pub file: &'f str,
    pub start: &'f str,
    /// Definitions by name, with the file whose text defines them.
    pub defs: BTreeMap<&'f str, (&'f str, &'f rnc::Define)>,
}

impl Files {
    pub fn read(dir: &Path, ids: &mut Ids) -> Result<Files, String> {
        let mut paths = Vec::new();
        collect_schemas(dir, &mut paths)?;
        paths.sort();
        if paths.is_empty() {
            return Err(format!("{}: no .rnc or .rng files", dir.display()));
        }
        let mut parsed = BTreeMap::new();
        let mut hashes = BTreeMap::new();
        let mut stray_lines = BTreeMap::new();
        let mut versions = BTreeSet::new();
        let mut product = None;
        for path in &paths {
            let rel = relative(dir, path)?;
            let bytes = fs::read(path).map_err(|e| format!("{rel}: {e}"))?;
            hashes.insert(rel.clone(), hex(&Sha256::digest(&bytes)));
            let text = String::from_utf8(bytes).map_err(|e| format!("{rel}: {e}"))?;
            let file = if rel.ends_with(".rng") { rng::parse(&text, ids) } else { rnc::parse(&text, ids) };
            let file = file.map_err(|e| format!("{}/{rel}: {e}", dir.display()))?;
            if let Some(v) = file.header.as_deref().and_then(|h| h.strip_prefix("Adobe InDesign ")) {
                let (name, build) = v.rsplit_once(' ').unwrap_or(("", v));
                versions.insert(build.to_owned());
                product = Some(name.to_owned()).filter(|n| !n.is_empty());
            }
            if !file.stray_lines.is_empty() {
                stray_lines.insert(rel.clone(), file.stray_lines.clone());
            }
            parsed.insert(rel, file);
        }
        let indesign_version = match versions.len() {
            0 => None,
            1 => versions.into_iter().next(),
            _ => return Err(format!("{}: headers name different builds {versions:?}", dir.display())),
        };
        let sha256 = {
            let mut h = Sha256::new();
            for (path, hash) in &hashes {
                h.update(format!("{path}\0{hash}\n"));
            }
            hex(&h.finalize())
        };
        Ok(Files { dir: dir.to_owned(), parsed, hashes, sha256, indesign_version, product, stray_lines, amendment_nodes: BTreeMap::new() })
    }

    pub fn apply_amendments(&mut self, ids: &mut Ids) -> Result<(), String> {
        self.amendment_nodes = amend(&mut self.parsed, ids)?;
        Ok(())
    }

    /// The part grammars: every file with a `start` and what it includes.
    pub fn grammars(&self) -> Result<Vec<Grammar<'_>>, String> {
        let mut out = Vec::new();
        for (rel, file) in &self.parsed {
            let Some(start) = &file.start else { continue };
            let Kind::Ref(start) = &start.kind else {
                return Err(format!("{rel}: start must refer to a definition"));
            };
            let mut scope = vec![(rel.as_str(), file)];
            for include in &file.includes {
                let inc = resolve(rel, include);
                let (inc, f) = self.parsed.get_key_value(&inc).ok_or_else(|| format!("{rel}: missing include {include}"))?;
                if f.start.is_some() || !f.includes.is_empty() {
                    return Err(format!("{inc}: included files must not have start or includes"));
                }
                scope.push((inc.as_str(), f));
            }
            let mut defs = BTreeMap::new();
            for (source, f) in scope {
                for d in &f.defines {
                    if defs.insert(d.name.as_str(), (source, d)).is_some() {
                        return Err(format!("{rel}: {} is defined twice in one grammar", d.name));
                    }
                }
            }
            out.push(Grammar { file: rel, start, defs });
        }
        Ok(out)
    }
}

impl Schema {
    /// Loads the schemas the model is generated from. Unlike [`Files`], this
    /// requires one global meaning per definition name.
    pub fn load(dir: &Path) -> Result<Schema, String> {
        Self::load_profile(dir, true)
    }

    /// Unmodified upstream grammar, for comparison and external validation.
    pub fn load_raw(dir: &Path) -> Result<Schema, String> {
        Self::load_profile(dir, false)
    }

    fn load_profile(dir: &Path, effective: bool) -> Result<Schema, String> {
        let mut ids = Ids::default();
        let mut files = Files::read(dir, &mut ids)?;
        if effective {
            files.amendment_nodes = amend(&mut files.parsed, &mut ids)?;
        }
        let indesign_version = files.indesign_version.clone().ok_or("expected a `# Adobe InDesign <version>` header")?;
        Schema::build(dir, &files, &files.grammars()?, indesign_version)
    }

    /// One schema per part grammar, for older snapshots, which define some
    /// names differently in different parts.
    pub fn load_parts(dir: &Path) -> Result<Vec<Schema>, String> {
        Self::load_parts_profile(dir, false)
    }
    pub fn load_effective_parts(dir: &Path) -> Result<Vec<Schema>, String> {
        Self::load_parts_profile(dir, true)
    }
    fn load_parts_profile(dir: &Path, effective: bool) -> Result<Vec<Schema>, String> {
        let mut ids = Ids::default();
        let mut files = Files::read(dir, &mut ids)?;
        if effective {
            files.apply_amendments(&mut ids)?;
        }
        let version = files.indesign_version.clone().unwrap_or_default();
        files.grammars()?.iter().map(|g| Schema::build(dir, &files, std::slice::from_ref(g), version.clone())).collect()
    }

    fn build(dir: &Path, files: &Files, grammars: &[Grammar], indesign_version: String) -> Result<Schema, String> {
        let mut defs: BTreeMap<String, Def> = BTreeMap::new();
        let mut entries = Vec::new();
        for g in grammars {
            for (name, (source, d)) in &g.defs {
                match defs.get_mut(*name) {
                    Some(def) if def.pattern.key() != d.pattern.key() => {
                        return Err(format!(
                            "{name} differs between {} and {}; the generator needs one global definition per name",
                            def.defined_in.iter().next().map_or("", String::as_str),
                            g.file
                        ));
                    }
                    Some(def) => {
                        def.defined_in.insert((*source).to_owned());
                    }
                    None => {
                        let def = Def { pattern: d.pattern.clone(), doc: d.doc.clone(), defined_in: BTreeSet::from([(*source).to_owned()]) };
                        defs.insert((*name).to_owned(), def);
                    }
                }
            }
            let def = defs.get(g.start).ok_or_else(|| format!("{}: undefined start {}", g.file, g.start))?;
            let Kind::Element(NameClass::Name(root), _) = &def.pattern.kind else {
                return Err(format!("{}: start {} is not a named element", g.file, g.start));
            };
            entries.push(Entry { file: g.file.to_owned(), start: g.start.to_owned(), root: root.clone() });
        }

        let mut reachable = BTreeSet::new();
        let mut stack: Vec<String> = entries.iter().map(|e| e.start.clone()).collect();
        while let Some(name) = stack.pop() {
            let def = defs.get(&name).ok_or_else(|| format!("undefined reference {name}"))?;
            if reachable.insert(name) {
                def.pattern.walk(&mut |p| {
                    if let Kind::Ref(r) = &p.kind {
                        stack.push(r.clone());
                    }
                });
            }
        }
        Ok(Schema {
            dir: dir.to_owned(),
            indesign_version,
            sha256: files.sha256.clone(),
            files: files.hashes.clone(),
            stray_lines: files.stray_lines.clone(),
            entries,
            defs,
            reachable,
            amendment_nodes: files.amendment_nodes.clone(),
        })
    }

    pub fn def(&self, name: &str) -> Result<&Def, String> {
        self.defs.get(name).ok_or_else(|| format!("undefined reference {name}"))
    }

    /// The definitions reachable from one part's start pattern.
    pub fn reachable_from(&self, entry: &Entry) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![entry.start.clone()];
        while let Some(name) = stack.pop() {
            let Some(def) = self.defs.get(&name) else { continue };
            if seen.insert(name) {
                def.pattern.walk(&mut |p| {
                    if let Kind::Ref(r) = &p.kind {
                        stack.push(r.clone());
                    }
                });
            }
        }
        seen
    }
}

/// An evidence-backed correction, separate from the upstream schema history.
#[derive(Debug, Clone)]
pub struct Amendment {
    pub id: String,
    pub min: String,
    pub max: String,
    pub target: String,
    pub action: String,
    pub pattern: String,
    pub evidence: String,
}

impl Amendment {
    /// Inclusive application bounds, separate from the dates of the evidence.
    pub fn applies_to(&self, version: &str) -> bool {
        let (Some(v), Some(min)) = (dom(version), dom(&self.min)) else { return false };
        v >= min && (self.max == "*" || dom(&self.max).is_some_and(|max| v <= max))
    }
}

pub fn amendments_sha256() -> String {
    hex(&Sha256::digest(include_bytes!("../../idml/schema-amendments.tsv")))
}

pub fn amendments() -> Result<Vec<Amendment>, String> {
    parse_amendments(include_str!("../../idml/schema-amendments.tsv"))
}

pub fn parse_amendments(text: &str) -> Result<Vec<Amendment>, String> {
    let mut out: Vec<Amendment> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let error = |why| format!("schema-amendments.tsv:{}: {why}", i + 1);
        let [id, min, max, target, action, pattern, evidence] = line.split('\t').collect::<Vec<_>>()[..] else {
            return Err(error("expected id, min DOM, max DOM, target, action, pattern, evidence"));
        };
        if id.is_empty() || evidence.trim().is_empty() || out.iter().any(|a| a.id == id) {
            return Err(error("each amendment needs a unique ID and evidence"));
        }
        if dom(min).is_none() || (max != "*" && dom(max).is_none_or(|m| Some(m) < dom(min))) {
            return Err(error("invalid inclusive DOMVersion bounds"));
        }
        if !matches!(action, "add" | "choice" | "replace" | "optional") {
            return Err(error("unknown amendment action"));
        }
        out.push(Amendment {
            id: id.into(),
            min: min.into(),
            max: max.into(),
            target: target.into(),
            action: action.into(),
            pattern: pattern.into(),
            evidence: evidence.into(),
        });
    }
    Ok(out)
}

fn dom(text: &str) -> Option<(u16, u16)> {
    let (a, b) = text.split_once('.')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// Corrections affect the effective model/oracle, never raw snapshots.
fn amend(parsed: &mut BTreeMap<String, rnc::File>, ids: &mut Ids) -> Result<BTreeMap<u32, String>, String> {
    let mut version = None;
    for file in parsed.values() {
        for d in &file.defines {
            d.pattern.walk(&mut |p| {
                if let Kind::Attribute(NameClass::Name(n), v) = &p.kind
                    && n.local == "DOMVersion"
                    && let Kind::Value { value, .. } = &v.kind
                {
                    version = Some(value.clone());
                }
            });
        }
    }
    let defined: BTreeSet<String> = parsed.values().flat_map(|f| f.defines.iter().map(|d| d.name.clone())).collect();
    let mut nodes = BTreeMap::new();
    for a in amendments()? {
        if version.as_deref().is_none_or(|v| !a.applies_to(v)) {
            continue;
        }
        let snippet = rnc::parse(&format!("amendment = {}", a.pattern), ids).map_err(|e| format!("amendment {}: {e}", a.id))?;
        let Some(pattern) = snippet.defines.into_iter().next().map(|d| d.pattern) else {
            return Err(format!("amendment {}: no pattern", a.id));
        };
        let mut refs = Vec::new();
        pattern.walk(&mut |p| {
            if let Kind::Ref(r) = &p.kind {
                refs.push(r.clone());
            }
        });
        let mut path = a.target.split('/');
        let definition = path.next().unwrap_or_default();
        if !defined.contains(definition) {
            continue;
        }
        if refs.iter().any(|r| !defined.contains(r)) {
            return Err(format!("amendment {}: undefined reference in {}", a.id, a.pattern));
        }
        let path: Vec<&str> = path.collect();
        let mut matches = 0;
        for d in parsed.values_mut().flat_map(|f| f.defines.iter_mut()).filter(|d| d.name == definition) {
            let Kind::Element(_, content) = &mut d.pattern.kind else {
                return Err(format!("amendment {}: not an element definition", a.id));
            };
            apply_at(content, &path, &a, &pattern, ids, &mut nodes, &mut matches);
        }
        if matches == 0 {
            return Err(format!("amendment {}: target {} does not exist", a.id, a.target));
        }
    }
    Ok(nodes)
}

fn apply_at(p: &mut Pattern, path: &[&str], a: &Amendment, extra: &Pattern, ids: &mut Ids, nodes: &mut BTreeMap<u32, String>, matches: &mut usize) {
    if path.is_empty() {
        let extra = renumbered(extra, ids);
        extra.walk(&mut |q| {
            nodes.insert(q.id, a.id.clone());
        });
        match a.action.as_str() {
            "add" => add_content(p, extra, ids),
            "choice" if matches!(p.kind, Kind::Choice(_)) => {
                if let Kind::Choice(branches) = &mut p.kind {
                    branches.push(extra);
                }
            }
            "choice" => {
                let old = std::mem::replace(p, Pattern { id: ids.next(), kind: Kind::Empty });
                *p = Pattern { id: ids.next(), kind: Kind::Choice(vec![old, extra]) };
            }
            _ => *p = extra,
        }
        *matches += 1;
        return;
    }
    if a.action == "optional"
        && path.len() == 1
        && matches!(&p.kind, Kind::Attribute(NameClass::Name(n), _) if path[0].strip_prefix('@') == Some(n.qualified().as_str()))
    {
        let old = std::mem::replace(p, Pattern { id: ids.next(), kind: Kind::Empty });
        old.walk(&mut |q| {
            nodes.insert(q.id, a.id.clone());
        });
        *p = Pattern { id: ids.next(), kind: Kind::Optional(Box::new(old)) };
        *matches += 1;
        return;
    }
    match &mut p.kind {
        Kind::Element(NameClass::Name(n), c) if path[0] == n.qualified() => apply_at(c, &path[1..], a, extra, ids, nodes, matches),
        Kind::Attribute(NameClass::Name(n), c) if path[0].strip_prefix('@') == Some(n.qualified().as_str()) => {
            apply_at(c, &path[1..], a, extra, ids, nodes, matches)
        }
        Kind::Group(ps) | Kind::Interleave(ps) | Kind::Choice(ps) => {
            for c in ps {
                apply_at(c, path, a, extra, ids, nodes, matches);
            }
        }
        Kind::Optional(c) | Kind::ZeroOrMore(c) | Kind::OneOrMore(c) => apply_at(c, path, a, extra, ids, nodes, matches),
        _ => {}
    }
}

/// Adds `extra` to an element's content: into its interleave of child
/// elements, if it has one, else after the content.
fn add_content(content: &mut Pattern, extra: Pattern, ids: &mut Ids) {
    let attribute = match &extra.kind {
        Kind::Attribute(..) => true,
        Kind::Optional(p) => matches!(p.kind, Kind::Attribute(..)),
        _ => false,
    };
    if attribute {
        let old = std::mem::replace(content, Pattern { id: ids.next(), kind: Kind::Empty });
        *content = Pattern { id: ids.next(), kind: Kind::Group(vec![extra, old]) };
        return;
    }
    match &mut content.kind {
        Kind::Interleave(items) => items.push(extra),
        Kind::Group(items) => match items.iter_mut().rev().find(|p| matches!(p.kind, Kind::Interleave(_))) {
            Some(Pattern { kind: Kind::Interleave(inner), .. }) => inner.push(extra),
            _ => items.push(extra),
        },
        Kind::Empty => *content = extra,
        _ => {
            let old = std::mem::replace(content, Pattern { id: ids.next(), kind: Kind::Empty });
            *content = Pattern { id: ids.next(), kind: Kind::Group(vec![old, extra]) };
        }
    }
}

/// A copy of `p` with new node ids, so each file's copy has its own.
fn renumbered(p: &Pattern, ids: &mut Ids) -> Pattern {
    let kind = match &p.kind {
        Kind::Element(n, c) => Kind::Element(n.clone(), Box::new(renumbered(c, ids))),
        Kind::Attribute(n, c) => Kind::Attribute(n.clone(), Box::new(renumbered(c, ids))),
        Kind::Group(ps) => Kind::Group(ps.iter().map(|c| renumbered(c, ids)).collect()),
        Kind::Interleave(ps) => Kind::Interleave(ps.iter().map(|c| renumbered(c, ids)).collect()),
        Kind::Choice(ps) => Kind::Choice(ps.iter().map(|c| renumbered(c, ids)).collect()),
        Kind::Optional(c) => Kind::Optional(Box::new(renumbered(c, ids))),
        Kind::ZeroOrMore(c) => Kind::ZeroOrMore(Box::new(renumbered(c, ids))),
        Kind::OneOrMore(c) => Kind::OneOrMore(Box::new(renumbered(c, ids))),
        Kind::List(c) => Kind::List(Box::new(renumbered(c, ids))),
        other => other.clone(),
    };
    Pattern { id: ids.next(), kind }
}

fn collect_schemas(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            collect_schemas(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rnc" || e == "rng") {
            out.push(path);
        }
    }
    Ok(())
}

fn relative(dir: &Path, path: &Path) -> Result<String, String> {
    let rel = path.strip_prefix(dir).map_err(|e| e.to_string())?;
    let parts: Vec<_> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    Ok(parts.join("/"))
}

/// Resolves an include path relative to the including file.
fn resolve(from: &str, include: &str) -> String {
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for seg in include.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    #[test]
    fn resolves_includes() {
        assert_eq!(resolve("Stories/Story.rnc", "../datatype.rnc"), "datatype.rnc");
        assert_eq!(resolve("designmap.rnc", "datatype.rnc"), "datatype.rnc");
    }

    #[test]
    fn amendments_obey_bounds_and_keep_attribute_additions_outside_child_content() {
        let a = parse_amendments("x\t12.0\t12.1\tA\tadd\tattribute Count {xsd:int}?\tevidence").unwrap().remove(0);
        for v in ["12.0", "12.1"] {
            assert!(a.applies_to(v));
        }
        for v in ["11.0", "12.2", "13.0", "x"] {
            assert!(!a.applies_to(v));
        }
        let mut ids = Ids::default();
        let mut p = rnc::parse("A = (element Child {empty}* & element Other {empty}*)", &mut ids).unwrap().defines.remove(0).pattern;
        let extra = rnc::parse(&format!("X = {}", a.pattern), &mut ids).unwrap().defines.remove(0).pattern;
        add_content(&mut p, extra, &mut ids);
        let Kind::Group(items) = p.kind else {
            panic!("attribute must be outside the child interleave");
        };
        assert!(matches!(items[0].kind, Kind::Optional(_)));
        assert!(matches!(items[1].kind, Kind::Interleave(_)));
    }
}
