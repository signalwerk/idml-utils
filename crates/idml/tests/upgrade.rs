//! Documents of older IDML versions are read into the current model.
//!
//! For every older snapshot in `schemas/`, documents generated from its
//! grammars (by `idml_codegen::instance`, which knows nothing of the model)
//! must be read strictly, with warnings only where `schema-upgrades.tsv`
//! drops data, and together they must exercise every item of those
//! grammars. 6.0 exists only as a single-file schema; its parts are the 7.0
//! part roots around the 6.0 definitions.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use common::{CONTAINER, part, zip};
use idml::Package;
use idml_codegen::instance::{Generator, Lexical, Style};
use idml_codegen::rnc::{Kind, NameClass, Pattern};
use idml_codegen::schema::{Def, Schema};

/// Documents per part and style, before adding documents for what is still
/// missing.
const DOCUMENTS: u64 = 2;

/// The `<build>/package` folders of the older snapshots, and the 6.0 one.
fn older() -> (Vec<PathBuf>, PathBuf) {
    let crate_dir = idml_codegen::idml_crate_dir();
    let current = idml_codegen::configured_schema_dir(&crate_dir).unwrap().canonicalize().unwrap();
    let root = idml_codegen::configured_history_dir(&crate_dir).unwrap().unwrap();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().path().join("package")).filter(|p| p.is_dir()).collect();
    dirs.retain(|d| d.canonicalize().unwrap() != current);
    dirs.sort();
    (dirs, root.join("6.0/single"))
}

#[test]
fn older_parts_are_read() {
    let (dirs, single) = older();
    let seven = dirs.iter().find(|d| d.parent().unwrap().file_name().unwrap().to_string_lossy().starts_with("7.0.")).unwrap();
    let six = Schema::load_parts(&single).unwrap().remove(0);
    let mut versions: Vec<(String, Vec<Schema>)> =
        vec![("6.0".to_owned(), Schema::load_parts(seven).unwrap().iter().map(|part| with_root_of(&six, part, "6.0")).collect())];
    for dir in &dirs {
        versions.push((dir.parent().unwrap().file_name().unwrap().to_string_lossy().into_owned(), Schema::load_parts(dir).unwrap()));
    }
    // One thread per version.
    std::thread::scope(|s| {
        for (version, schemas) in &versions {
            s.spawn(move || {
                for schema in schemas.iter().filter(|s| part(&s.entries[0].file).is_some()) {
                    read_all(schema, version);
                }
            });
        }
    });
}

/// Reads generated documents of a part grammar until they exercised all of it.
fn read_all(schema: &Schema, version: &str) {
    let entry = &schema.entries[0];
    let read = part(&entry.file).unwrap();
    let mut g = Generator::new(schema, 7, Style::Full, Lexical::Canonical);
    let check = |g: &mut Generator, what: &str| {
        let xml = g.document(entry).unwrap();
        let (_, warnings) = read(&xml, &idml::ReadOptions::default()).unwrap_or_else(|e| panic!("{version} {} {what}: {e}", entry.file));
        for w in warnings {
            assert!(w.message.contains(" left out: "), "{version} {} {what}: unexpected warning {w}", entry.file);
        }
    };
    for i in 0..DOCUMENTS {
        for style in [Style::Full, Style::Random] {
            g.set_style(style, Lexical::Canonical);
            check(&mut g, &format!("#{i} {style:?}"));
        }
    }
    g.set_style(Style::Full, Lexical::Canonical);
    for round in 0..200 {
        if g.uncovered().is_empty() {
            break;
        }
        check(&mut g, &format!("extra #{round}"));
    }
    let uncovered = g.uncovered();
    let shown: Vec<String> = uncovered.iter().take(20).map(|(d, p)| format!("{d}: {p}")).collect();
    assert!(uncovered.is_empty(), "{version} {}: {} items never exercised: {shown:#?}", entry.file, uncovered.len());
}

#[test]
fn older_packages_are_read() {
    let (dirs, _) = older();
    for dir in &dirs {
        let version = dir.parent().unwrap().file_name().unwrap().to_string_lossy().into_owned();
        let schemas = Schema::load_parts(dir).unwrap();
        let designmap = schemas.iter().find(|s| s.entries[0].file.starts_with("designmap.")).unwrap();
        let mut g = Generator::new(designmap, 3, Style::Full, Lexical::Canonical);
        let xml = g.document(&designmap.entries[0]).unwrap();
        let mut files = vec![("mimetype".to_owned(), idml::MIMETYPE.to_owned()), ("META-INF/container.xml".to_owned(), CONTAINER.to_owned())];
        let doc = roxmltree::Document::parse(&xml).unwrap();
        for reference in doc.root_element().children().filter(|n| n.attribute("src").is_some()) {
            let root = format!("idPkg:{}", reference.tag_name().name());
            let schema = schemas.iter().find(|s| s.entries[0].root.qualified() == root).unwrap();
            let mut parts = Generator::new(schema, 4, Style::Minimal, Lexical::Canonical);
            files.push((reference.attribute("src").unwrap().to_owned(), parts.document(&schema.entries[0]).unwrap()));
        }
        files.push(("designmap.xml".to_owned(), xml));
        let package = Package::read(&zip(&files), &idml::ReadOptions::default()).unwrap_or_else(|e| panic!("{version} package: {e}"));
        if let Err(e) = package.value.write() {
            panic!("{version} package: writing: {e}");
        }
    }
}

/// The part grammar `part` with the definitions of `older` where it has
/// them, and `DOMVersion` `dom`.
fn with_root_of(older: &Schema, part: &Schema, dom: &str) -> Schema {
    let start = part.entries[0].start.clone();
    let mut defs = older.defs.clone();
    let mut stack = vec![start.clone()];
    let mut taken = BTreeSet::new();
    while let Some(name) = stack.pop() {
        if (name != start && older.defs.contains_key(&name)) || !taken.insert(name.clone()) {
            continue;
        }
        let Some(def) = part.defs.get(&name) else { continue };
        let mut def: Def = def.clone();
        set_dom(&mut def.pattern, dom);
        def.pattern.walk(&mut |p| {
            if let Kind::Ref(r) = &p.kind {
                stack.push(r.clone());
            }
        });
        defs.insert(name, def);
    }
    let mut schema = Schema {
        dir: Path::new(&part.dir).to_owned(),
        indesign_version: older.indesign_version.clone(),
        sha256: String::new(),
        files: Default::default(),
        stray_lines: Default::default(),
        entries: vec![part.entries[0].clone()],
        defs,
        reachable: BTreeSet::new(),
    };
    schema.reachable = schema.reachable_from(&schema.entries[0]);
    schema
}

fn set_dom(p: &mut Pattern, dom: &str) {
    match &mut p.kind {
        Kind::Attribute(NameClass::Name(q), v) if q.local == "DOMVersion" => {
            if let Kind::Value { value, .. } = &mut v.kind {
                *value = dom.to_owned();
            }
        }
        Kind::Element(_, q) | Kind::Optional(q) | Kind::ZeroOrMore(q) | Kind::OneOrMore(q) => set_dom(q, dom),
        Kind::Group(ps) | Kind::Interleave(ps) | Kind::Choice(ps) => ps.iter_mut().for_each(|q| set_dom(q, dom)),
        _ => {}
    }
}
