//! The generated model, schema map, history and reports must be what the generator produces
//! from the configured schemas. Lowering fails if any reachable schema
//! pattern has no representation, so this also checks completeness.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[test]
fn generated_files_are_up_to_date() {
    let crate_dir = idml_codegen::idml_crate_dir();
    let schema_dir = idml_codegen::configured_schema_dir(&crate_dir).unwrap();
    let history = idml_codegen::configured_history_dir(&crate_dir).unwrap();
    let upgrades = idml_codegen::configured_upgrades_file(&crate_dir).unwrap();
    let files = idml_codegen::generate(&schema_dir, history.as_deref(), upgrades.as_deref()).unwrap();
    let stale = idml_codegen::stale(&idml_codegen::workspace_dir(), &files);
    assert!(stale.is_empty(), "out of date: {stale:?}; run `cargo run -p idml-codegen`");
}
