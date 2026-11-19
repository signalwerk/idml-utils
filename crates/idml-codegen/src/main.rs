//! `cargo run -p idml-codegen [-- --check] [-- SCHEMA_DIR]`
//!
//! Regenerates `crates/idml/src/model/`, `crates/idml/schema-map.tsv`,
//! `crates/idml/schema-history.tsv` and the GitHub Pages site in `docs/` from the
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
    let mut export = None;
    let mut catalog = None;
    let mut raw = false;
    let mut universal = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--raw" => raw = true,
            "--universal" => universal = true,
            "--export-catalog" => catalog = Some(PathBuf::from(args.next().ok_or("--export-catalog requires a file path")?)),
            "--export-schemas" => export = Some(PathBuf::from(args.next().ok_or("--export-schemas requires a directory")?)),
            "-h" | "--help" => {
                println!(
                    "usage: idml-codegen [--check] [SCHEMA_DIR]\n       idml-codegen --universal [--check]\n       idml-codegen --export-catalog FILE\n       idml-codegen --export-schemas DIR [--raw] [SCHEMA_DIR]"
                );
                return Ok(());
            }
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag}")),
            dir if schema_dir.is_none() => schema_dir = Some(PathBuf::from(dir)),
            _ => return Err("only one schema directory may be supplied".into()),
        }
    }
    let crate_dir = idml_codegen::idml_crate_dir();
    if universal {
        if schema_dir.is_some() || catalog.is_some() || export.is_some() || raw {
            return Err("--universal accepts only --check and uses the configured history".into());
        }
        let history = idml_codegen::configured_history_dir(&crate_dir)?.ok_or("universal generation requires a history directory")?;
        let files = idml_codegen::generate_universal(&history)?;
        let workspace = idml_codegen::workspace_dir();
        if check {
            let stale: Vec<_> =
                files.iter().filter(|(p, t)| std::fs::read_to_string(workspace.join(p)).ok().as_ref() != Some(t)).map(|(p, _)| p.as_str()).collect();
            if !stale.is_empty() {
                return Err(format!("universal artifacts out of date: {}", stale.join(", ")));
            }
            println!("universal artifacts are up to date");
        } else {
            idml_codegen::write_selected(&workspace, &files)?;
            println!("generated {} universal artifacts", files.len());
        }
        return Ok(());
    }
    if let Some(path) = catalog {
        if check || export.is_some() || raw || schema_dir.is_some() {
            return Err("--export-catalog uses configured history and cannot be combined with other modes or a schema directory".into());
        }
        let root = idml_codegen::configured_history_dir(&crate_dir)?.ok_or("catalog export requires a configured history directory")?;
        let bytes = idml_codegen::profiles::bytes(&root)?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        println!("exported profile catalog to {}", path.display());
        return Ok(());
    }
    let schema_dir = match schema_dir {
        Some(dir) => dir,
        None => idml_codegen::configured_schema_dir(&crate_dir)?,
    };
    let schema_dir = schema_dir.canonicalize().unwrap_or(schema_dir);
    if check && export.is_some() {
        return Err("--check and --export-schemas are mutually exclusive".into());
    }
    if let Some(dir) = export {
        let schema = if raw { idml_codegen::schema::Schema::load_raw(&schema_dir)? } else { idml_codegen::schema::Schema::load(&schema_dir)? };
        for entry in &schema.entries {
            let path = dir.join(&entry.file).with_extension("rng");
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, idml_codegen::rng::write(&schema, entry)?).map_err(|e| e.to_string())?;
        }
        println!("exported {} standalone grammars to {}", schema.entries.len(), dir.display());
        return Ok(());
    }
    if raw {
        return Err("--raw requires --export-schemas".into());
    }
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
