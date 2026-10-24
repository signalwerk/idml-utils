//! `cargo run -p idml-codegen [-- --check] [-- SCHEMA_DIR]`
//!
//! Regenerates `crates/idml/src/model/`, `crates/idml/schema-map.tsv`,
//! `crates/idml/schema-history.tsv` and `docs/schema-changes/` from the
//! package schemas configured in `crates/idml/Cargo.toml`, or from
//! `SCHEMA_DIR`. With `--check`, only reports whether they are up to date.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("idml-codegen: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut check = false;
    let mut schema_dir = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--check" => check = true,
            "-h" | "--help" => {
                println!("usage: idml-codegen [--check] [SCHEMA_DIR]");
                return Ok(());
            }
            dir => schema_dir = Some(PathBuf::from(dir)),
        }
    }
    let crate_dir = idml_codegen::idml_crate_dir();
    let schema_dir = match schema_dir {
        Some(dir) => dir,
        None => idml_codegen::configured_schema_dir(&crate_dir)?,
    };
    let schema_dir = schema_dir.canonicalize().unwrap_or(schema_dir);
    let history = idml_codegen::configured_history_dir(&crate_dir)?;
    let upgrades = idml_codegen::configured_upgrades_file(&crate_dir)?;
    let files = idml_codegen::generate(&schema_dir, history.as_deref(), upgrades.as_deref())?;
    if check {
        let stale = idml_codegen::stale(&idml_codegen::workspace_dir(), &files);
        if !stale.is_empty() {
            return Err(format!("out of date: {}; run `cargo run -p idml-codegen`", stale.join(", ")));
        }
        println!("up to date with {}", schema_dir.display());
    } else {
        idml_codegen::write(&idml_codegen::workspace_dir(), &files)?;
        println!("generated {} files from {}", files.len(), schema_dir.display());
    }
    Ok(())
}
