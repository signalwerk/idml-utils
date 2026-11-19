//! A compact shared vocabulary and exact grammar catalog across all snapshots.
use crate::{
    history,
    rnc::{Ids, Kind, NameClass, Pattern},
    schema::{self, Files},
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt::Write as _, path::Path};

#[derive(Default)]
struct Pool {
    strings: Vec<String>,
    string_ids: BTreeMap<String, u32>,
    patterns: Vec<Vec<u32>>,
    pattern_ids: BTreeMap<Vec<u32>, u32>,
}
impl Pool {
    fn string(&mut self, s: &str) -> u32 {
        if let Some(id) = self.string_ids.get(s) {
            return *id;
        }
        let id = self.strings.len() as u32;
        self.strings.push(s.into());
        self.string_ids.insert(s.into(), id);
        id
    }
    fn pattern(&mut self, p: &Pattern) -> u32 {
        let mut words = match &p.kind {
            Kind::Empty => vec![0],
            Kind::NotAllowed => vec![1],
            Kind::Text => vec![2],
            Kind::Element(n, c) | Kind::Attribute(n, c) => {
                let tag = if matches!(p.kind, Kind::Element(..)) { 3 } else { 4 };
                let (ns, local, any) = match n {
                    NameClass::Name(q) => (self.string(&q.ns), self.string(&q.local), 0),
                    NameClass::Any => (self.string(""), self.string(""), 1),
                };
                vec![tag, self.pattern(c), ns, local, any]
            }
            Kind::Group(ps) | Kind::Interleave(ps) | Kind::Choice(ps) => {
                let tag = match p.kind {
                    Kind::Group(_) => 5,
                    Kind::Interleave(_) => 6,
                    _ => 7,
                };
                let mut w = vec![tag];
                w.extend(ps.iter().map(|p| self.pattern(p)));
                w
            }
            Kind::Optional(c) | Kind::ZeroOrMore(c) | Kind::OneOrMore(c) | Kind::List(c) => vec![
                match p.kind {
                    Kind::Optional(_) => 8,
                    Kind::ZeroOrMore(_) => 9,
                    Kind::OneOrMore(_) => 10,
                    _ => 11,
                },
                self.pattern(c),
            ],
            Kind::Ref(n) => vec![12, self.string(n)],
            Kind::Value { token, value } => vec![13, u32::from(*token), self.string(value)],
            Kind::Data { xsd, params } => {
                let mut w = vec![14, self.string(xsd)];
                for (k, v) in params {
                    w.extend([self.string(k), self.string(v)]);
                }
                w
            }
        };
        if let Some(id) = self.pattern_ids.get(&words) {
            return *id;
        }
        let id = self.patterns.len() as u32;
        self.patterns.push(words.clone());
        self.pattern_ids.insert(std::mem::take(&mut words), id);
        id
    }
}
fn u32out(out: &mut Vec<u8>, n: u32) {
    out.extend(n.to_le_bytes());
}
fn wordsout(out: &mut Vec<u8>, words: &[u32]) {
    u32out(out, words.len() as u32);
    for &w in words {
        u32out(out, w);
    }
}

/// Both raw and corrected grammars keep exact per-file definition scopes.
/// Pattern and string interning shares unchanged vocabulary between releases.
pub fn bytes(history_root: &Path) -> Result<Vec<u8>, String> {
    let snapshots = history::snapshots(history_root)?;
    let mut pool = Pool::default();
    let mut profiles = Vec::<Vec<u8>>::new();
    let mut identities = std::collections::BTreeSet::new();
    for snapshot in snapshots {
        let v = snapshot.version;
        let dir = history_root.join(&v.dir);
        for effective in [false, true] {
            let mut ids = Ids::default();
            let mut files = Files::read(&dir, &mut ids)?;
            if effective {
                files.apply_amendments(&mut ids)?;
            }
            let mut unsupported = Vec::new();
            for file in files.parsed.values() {
                for definition in &file.defines {
                    definition.pattern.walk(&mut |p| {
                        if let Kind::Data { xsd, params } = &p.kind {
                            if !matches!(xsd.as_str(), "string" | "boolean" | "double" | "int" | "short" | "dateTime") {
                                unsupported.push(xsd.clone());
                            }
                            for (name, value) in params {
                                if !matches!(name.as_str(), "minInclusive" | "maxInclusive") && !(name == "pattern" && value == ".*\\.xml") {
                                    unsupported.push(format!("{name}={value}"));
                                }
                            }
                        }
                    });
                }
            }
            if !unsupported.is_empty() {
                return Err(format!("profile validator needs support for schema facets/types: {unsupported:?}"));
            }
            let kind = if effective { "effective" } else { "raw" };
            let amendments = if effective { schema::amendments_sha256() } else { String::new() };
            let build = v.build.as_deref().unwrap_or("");
            let mut locale = std::fs::read_to_string(dir.parent().unwrap_or(&dir).join("manifest.json")).ok().and_then(|m| {
                m.lines().find_map(|l| l.trim().strip_prefix("\"indesignLocale\": \"").and_then(|s| s.split('"').next()).map(str::to_owned))
            });
            let mut plugins = Vec::new();
            let mut plugins_complete = false;
            let mut inventory_present = false;
            if let Ok(metadata) = std::fs::read_to_string(dir.parent().unwrap_or(&dir).join("profile.tsv")) {
                for line in metadata.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
                    match line.split('\t').collect::<Vec<_>>().as_slice() {
                        ["locale", value] => locale = Some((*value).into()),
                        ["plugin", name, version] => {
                            plugins.push(((*name).to_owned(), (*version).to_owned()));
                            inventory_present = true;
                        }
                        ["plugins_complete", value @ ("true" | "false")] => {
                            plugins_complete = *value == "true";
                            inventory_present = true;
                        }
                        _ => return Err(format!("{}: invalid profile metadata row {line:?}", dir.display())),
                    }
                }
            }
            plugins.sort();
            plugins.dedup();
            let mut digest = Sha256::new();
            let mut field = |value: &str| {
                digest.update((value.len() as u64).to_le_bytes());
                digest.update(value.as_bytes());
            };
            for value in
                [v.sha256.as_str(), amendments.as_str(), if v.single { "single" } else { "package" }, build, kind, locale.as_deref().unwrap_or("")]
            {
                field(value);
            }
            field(if inventory_present { "inventory-present" } else { "inventory-unknown" });
            field(if plugins_complete { "complete" } else { "incomplete" });
            for (name, version) in &plugins {
                field(name);
                field(version);
            }
            let identity = schema::hex(&digest.finalize());
            if !identities.insert(identity.clone()) {
                continue;
            }
            let id = format!("{}/{}/{kind}/{}", v.dom, if build.is_empty() { "unknown" } else { build }, &identity[..12]);
            let mut p = Vec::new();
            for s in [&id, &v.dom, build, &v.sha256, &amendments, &identity, &v.dir] {
                u32out(&mut p, pool.string(s));
            }
            u32out(&mut p, u32::from(v.single));
            u32out(&mut p, pool.string(locale.as_deref().unwrap_or("")));
            u32out(&mut p, u32::from(inventory_present));
            u32out(&mut p, u32::from(plugins_complete));
            u32out(&mut p, plugins.len() as u32);
            for (name, version) in &plugins {
                u32out(&mut p, pool.string(name));
                u32out(&mut p, pool.string(version));
            }
            let grammars = files.grammars()?;
            u32out(&mut p, grammars.len() as u32);
            for grammar in grammars {
                u32out(&mut p, pool.string(grammar.file));
                let root = grammar.defs.get(grammar.start).ok_or("undefined start")?.1;
                u32out(&mut p, pool.pattern(&root.pattern));
                u32out(&mut p, grammar.defs.len() as u32);
                for (name, (_, def)) in grammar.defs {
                    u32out(&mut p, pool.string(name));
                    u32out(&mut p, pool.pattern(&def.pattern));
                }
            }
            profiles.push(p);
        }
    }
    let mut out = b"IDMLPROF2".to_vec();
    u32out(&mut out, pool.strings.len() as u32);
    for s in pool.strings {
        u32out(&mut out, s.len() as u32);
        out.extend(s.as_bytes());
    }
    u32out(&mut out, pool.patterns.len() as u32);
    for p in pool.patterns {
        wordsout(&mut out, &p);
    }
    u32out(&mut out, profiles.len() as u32);
    for p in profiles {
        out.extend(p);
    }
    Ok(miniz_oxide::deflate::compress_to_vec(&out, 9))
}

pub fn rust(root: &Path) -> Result<String, String> {
    let bytes = bytes(root)?;
    let mut out = String::from(crate::emit::HEADER);
    out.push_str("// Exact raw and effective schema profiles, compressed; generated from all snapshots.\npub(crate) const DATA: &[u8] = b\"\\\n");
    for chunk in bytes.chunks(32) {
        for b in chunk {
            let _ = write!(out, "\\x{b:02x}");
        }
        out.push_str("\\\n");
    }
    out.push_str("\";\n");
    Ok(out)
}
