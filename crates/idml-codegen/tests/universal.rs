#![allow(clippy::unwrap_used)]
#[test]
fn universal_writes_leave_the_independent_projection_and_reports_intact() {
    let root = std::env::temp_dir().join(format!("idml-universal-write-{}", std::process::id()));
    std::fs::create_dir_all(root.join("crates/idml/src/model")).unwrap();
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("crates/idml/src/model/keep.rs"), "projection").unwrap();
    std::fs::write(root.join("docs/keep.html"), "report").unwrap();
    let files = std::collections::BTreeMap::from([("crates/idml/src/profile_data.rs".into(), "universal".into())]);
    idml_codegen::write_selected(&root, &files).unwrap();
    assert_eq!(std::fs::read_to_string(root.join("crates/idml/src/model/keep.rs")).unwrap(), "projection");
    assert_eq!(std::fs::read_to_string(root.join("docs/keep.html")).unwrap(), "report");
    std::fs::remove_dir_all(root).unwrap();
}
