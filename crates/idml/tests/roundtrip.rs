//! Round trips of schema-valid documents generated from the grammar.
//!
//! The documents come from `idml_codegen::instance`, which walks the RELAX NG
//! patterns and knows nothing of the generated model. Reading one and writing
//! it again must give the same XML, and together the documents (parts, and
//! designmaps read as packages) must exercise every attribute, element,
//! choice branch and literal of the schemas.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{CONTAINER, PARTS, canon, difference, schema, unzip, zip};
use idml::Package;
use idml_codegen::instance::{Generator, Lexical, Style};

/// Documents per part and style, before adding documents for what is still
/// missing.
const DOCUMENTS: u64 = 6;

#[test]
fn parts_are_complete() {
    let schema = schema();
    let mut files: Vec<&str> = schema.entries.iter().map(|e| e.file.as_str()).filter(|f| *f != "designmap.rnc").collect();
    files.sort();
    let mut known: Vec<&str> = PARTS.iter().map(|(f, _)| *f).collect();
    known.sort();
    assert_eq!(files, known);
}

#[test]
fn generated_documents_round_trip_exactly() {
    let schema = schema();
    let mut g = Generator::new(&schema, 1, Style::Full, Lexical::Canonical);
    for entry in &schema.entries {
        for i in 0..DOCUMENTS {
            for style in [Style::Full, Style::Random] {
                g.set_style(style, Lexical::Canonical);
                round_trip(&mut g, &schema, &entry.file, &format!("#{i} {style:?}"));
            }
        }
    }
    // Large enumerations used by few attributes need more documents: add
    // full documents of a grammar that contains what is still missing.
    g.set_style(Style::Full, Lexical::Canonical);
    let reach: Vec<_> = schema.entries.iter().map(|e| (e.file.clone(), schema.reachable_from(e))).collect();
    for round in 0..300 {
        let Some((def, _)) = g.uncovered().into_iter().next() else { break };
        // The smallest grammar that reaches it.
        let file = reach.iter().filter(|(_, r)| r.contains(&def)).min_by_key(|(_, r)| r.len()).unwrap().0.clone();
        round_trip(&mut g, &schema, &file, &format!("extra #{round}"));
    }
    let uncovered = g.uncovered();
    let shown: Vec<String> = uncovered.iter().take(40).map(|(d, p)| format!("{d}: {p}")).collect();
    assert!(uncovered.is_empty(), "{} schema items never exercised: {shown:#?}", uncovered.len());
}

/// Generates a document for the grammar in `file`, reads it, writes it and
/// compares. The designmap references the other parts, so it is read as a
/// package.
fn round_trip(g: &mut Generator, schema: &idml_codegen::schema::Schema, file: &str, what: &str) {
    let options = idml::ReadOptions::default();
    let entry = schema.entries.iter().find(|e| e.file == file).unwrap();
    if let Some((_, part)) = PARTS.iter().find(|(f, _)| *f == file) {
        let xml = g.document(entry).unwrap();
        let (written, _) = part(&xml, &options).unwrap_or_else(|e| panic!("{file} {what}: {e}"));
        if let Some(d) = difference(&canon(&xml).unwrap(), &canon(&written).unwrap()) {
            panic!("{file} {what} differs at {d}");
        }
        return;
    }
    let files = package_files(g, schema, entry);
    let package = Package::read(&zip(&files), &options).unwrap_or_else(|e| panic!("package {what}: {e}")).value;
    let written = unzip(&package.write().unwrap());
    for (path, original) in &files {
        if path == "mimetype" || path.starts_with("META-INF") {
            continue;
        }
        let out = written.get(path).unwrap_or_else(|| panic!("package {what}: {path} not written"));
        if let Some(d) = difference(&canon(original).unwrap(), &canon(out).unwrap()) {
            panic!("package {what} {path} differs at {d}");
        }
    }
}

/// A generated designmap and the parts it references, as package files.
fn package_files(g: &mut Generator, schema: &idml_codegen::schema::Schema, designmap: &idml_codegen::schema::Entry) -> Vec<(String, String)> {
    let xml = g.document(designmap).unwrap();
    // The parts get their own documents; here they only need to exist.
    let style = g.style();
    g.set_style(Style::Minimal, Lexical::Canonical);
    let mut files = vec![("mimetype".to_owned(), idml::MIMETYPE.to_owned()), ("META-INF/container.xml".to_owned(), CONTAINER.to_owned())];
    let doc = roxmltree::Document::parse(&xml).unwrap();
    for reference in doc.root_element().children().filter(|n| n.attribute("src").is_some()) {
        let root = format!("idPkg:{}", reference.tag_name().name());
        let entry = schema.entries.iter().find(|e| e.root.qualified() == root).unwrap();
        files.push((reference.attribute("src").unwrap().to_owned(), g.document(entry).unwrap()));
    }
    files.push(("designmap.xml".to_owned(), xml));
    g.set_style(style, Lexical::Canonical);
    files
}

#[test]
fn any_valid_form_is_read_and_written_canonically() {
    let schema = schema();
    let mut g = Generator::new(&schema, 2, Style::Random, Lexical::Any);
    let options = idml::ReadOptions::default();
    for (file, round_trip) in PARTS {
        let entry = schema.entries.iter().find(|e| e.file == *file).unwrap();
        for i in 0..DOCUMENTS {
            let xml = g.document(entry).unwrap();
            let (once, _) = round_trip(&xml, &options).unwrap_or_else(|e| panic!("{file} #{i}: {e}"));
            let (twice, _) = round_trip(&once, &options).unwrap();
            assert_eq!(once, twice, "{file} #{i}: writing is not stable");
        }
    }
}
