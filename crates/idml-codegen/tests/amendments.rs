#![allow(clippy::unwrap_used, clippy::panic)]

use idml_codegen::{
    history,
    rnc::Ids,
    rng,
    schema::{self, Schema},
};

#[test]
fn effective_corrections_do_not_rewrite_upstream_history_or_hashes() {
    let path = idml_codegen::configured_schema_dir(&idml_codegen::idml_crate_dir()).unwrap();
    let raw = Schema::load_raw(&path).unwrap();
    let effective = Schema::load(&path).unwrap();
    assert_eq!(raw.sha256, effective.sha256);
    assert!(raw.amendment_nodes.is_empty());
    assert!(!effective.amendment_nodes.is_empty());
    let id = "CrossReferenceSource_Object/<Content>";
    assert!(!history::node_items(&raw).values().any(|item| item == id));
    assert!(history::node_items(&effective).values().any(|item| item == id));
    let snapshot = history::snapshot(&path, "21.5.0.64/package").unwrap();
    assert!(!snapshot.items.contains_key(id));
    let history = history::History::new(vec![snapshot]);
    assert!(history.tsv().contains("\tamendment\t"));
    assert!(!history.tsv().contains(&format!("{id}\t21.5")));
}

#[test]
fn standalone_validator_grammars_keep_every_pattern() {
    let path = idml_codegen::configured_schema_dir(&idml_codegen::idml_crate_dir()).unwrap();
    let schema = Schema::load(&path).unwrap();
    for entry in &schema.entries {
        let xml = rng::write(&schema, entry).unwrap();
        let parsed = rng::parse(&xml, &mut Ids::default()).unwrap();
        assert_eq!(parsed.defines.len(), schema.reachable_from(entry).len());
        for definition in parsed.defines {
            assert_eq!(definition.pattern.key(), schema.def(&definition.name).unwrap().pattern.key(), "{}: {}", entry.file, definition.name);
        }
    }
}

#[test]
fn amendments_require_explicit_valid_bounds_and_evidence() {
    for text in [
        "x\t12\t*\tA\tadd\ttext\tevidence",
        "x\t12.1\t12.0\tA\tadd\ttext\tevidence",
        "x\t12.0\t*\tA\tadd\ttext\t",
        "x\t12.0\t*\tA\tguess\ttext\tevidence",
    ] {
        assert!(schema::parse_amendments(text).is_err(), "{text}");
    }
    let row = "x\t12.0\t12.1\tA/@Count\tadd\tattribute Count {xsd:int}?\tevidence";
    assert_eq!(schema::parse_amendments(row).unwrap().len(), 1);
    assert!(schema::parse_amendments(&format!("{row}\n{row}")).is_err());
}

#[test]
fn expanded_attribute_names_are_not_silently_lowered_as_unqualified() {
    let dir = std::env::temp_dir().join(format!("idml-expanded-name-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Story.rng"),
        r#"<!-- Adobe InDesign Test 21.5.0.1 -->
<grammar xmlns="http://relaxng.org/ns/structure/1.0" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
<start><ref name="Story_File"/></start><define name="Story_File"><element name="idPkg:Story">
<attribute name="DOMVersion"><value type="string">21.5</value></attribute>
<optional><attribute><name ns="urn:plugin">Custom</name><text/></attribute></optional>
</element></define></grammar>"#,
    )
    .unwrap();
    let schema = Schema::load_raw(&dir).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    let error = idml_codegen::lower::lower(&schema).unwrap_err();
    assert!(error.contains("namespaced attribute"), "{error}");
}
