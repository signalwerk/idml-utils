#![allow(clippy::unwrap_used, clippy::panic)]
use idml::profile::{ProfileResolution, SchemaCatalog};
use idml::{ReadOptions, XmlTree};
use idml_codegen::{
    instance::{Generator, Lexical, Style},
    schema::Schema,
};
#[test]
fn every_snapshot_validates_its_own_minimal_full_and_random_documents() {
    let catalog = SchemaCatalog::builtin().unwrap();
    let root = idml_codegen::configured_history_dir(&idml_codegen::idml_crate_dir()).unwrap().unwrap();
    for profile in catalog.profiles() {
        let path = root.join(&profile.source);
        let schemas = if profile.amendments_sha256.is_some() { Schema::load_effective_parts(&path) } else { Schema::load_parts(&path) }.unwrap();
        for schema in schemas {
            for (seed, style, lexical) in [(1, Style::Minimal, Lexical::Canonical), (2, Style::Full, Lexical::Any), (3, Style::Random, Lexical::Any)]
            {
                let mut g = Generator::new(&schema, seed, style, lexical);
                for entry in &schema.entries {
                    let xml = g.document(entry).unwrap();
                    let tree = XmlTree::parse(&xml, &ReadOptions::source()).unwrap();
                    let report = catalog.validate(profile, &tree);
                    assert!(report.is_supported(), "{} {} seed {seed}: {:?}", profile.id, entry.file, report);
                }
            }
        }
    }
}
#[test]
fn current_effective_grammar_validates_full_and_random_instances() {
    let catalog = SchemaCatalog::builtin().unwrap();
    let profile = match catalog.resolve("21.5".parse().unwrap(), true) {
        ProfileResolution::Exact(p) => p,
        _ => panic!(),
    };
    let dir = idml_codegen::configured_schema_dir(&idml_codegen::idml_crate_dir()).unwrap();
    let schema = Schema::load(&dir).unwrap();
    for (seed, style) in [(1, Style::Full), (2, Style::Random), (3, Style::Random)] {
        let mut g = Generator::new(&schema, seed, style, Lexical::Any);
        for entry in &schema.entries {
            let xml = g.document(entry).unwrap();
            let tree = XmlTree::parse(&xml, &ReadOptions::source()).unwrap();
            let report = catalog.validate(profile, &tree);
            assert!(report.is_supported(), "{} {seed}: {:?}", entry.file, report);
        }
    }
}

/// RELAX NG weak matching (section 6.2.7): indented empty elements, as InDesign
/// writes them, match like self-closing ones. Whitespace never satisfies
/// required data, and ACE characters are not whitespace.
#[test]
fn whitespace_only_content_matches_like_empty_content() {
    let catalog = SchemaCatalog::builtin().unwrap();
    let profile = match catalog.resolve("15.0".parse().unwrap(), true) {
        ProfileResolution::Exact(p) => p,
        other => panic!("{other:?}"),
    };
    let validate = |body: &str| {
        let xml = format!(
            "<idPkg:Preferences xmlns:idPkg='http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging' DOMVersion='15.0'><XMLImportPreference><Properties>{body}</Properties></XMLImportPreference></idPkg:Preferences>"
        );
        catalog.validate(profile, &XmlTree::parse(&xml, &ReadOptions::source()).unwrap())
    };
    for body in
        ["<TransformParameters/>", "<TransformParameters>\n\t\t</TransformParameters>", "<TransformParameters> <!-- c --> \r\n</TransformParameters>"]
    {
        assert!(validate(body).is_supported(), "{body:?}: {:?}", validate(body));
    }
    for body in [
        "<TransformParameters>x</TransformParameters>",
        "<TransformParameters>\u{a0}</TransformParameters>",
        "<TransformParameters><?ACE 7?></TransformParameters>",
        "<TransformFilename type='enumeration'>\n</TransformFilename>",
    ] {
        assert!(!validate(body).is_supported(), "{body:?}");
    }
}
