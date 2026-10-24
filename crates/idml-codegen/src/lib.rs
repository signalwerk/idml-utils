//! Generates the `idml` crate's document model from InDesign's RELAX NG
//! package schemas.
//!
//! The pipeline is [`schema::Schema::load`] (parse and merge the `.rnc`
//! files), [`lower::lower`] (map every reachable pattern to the [`model`],
//! failing on anything it cannot represent or leaves unrepresented),
//! [`history`] (date every item from older schema snapshots), [`upgrade`]
//! (check the rules for reading older versions), then [`emit`] (Rust
//! source), [`map`] (the schema map) and [`report`] (HTML reports of the
//! changes between versions).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub mod emit;
pub mod history;
pub mod instance;
pub mod lower;
pub mod map;
pub mod model;
pub mod names;
pub mod report;
pub mod rnc;
pub mod rng;
pub mod schema;
pub mod upgrade;

/// The workspace directory.
pub fn workspace_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The `idml` crate directory.
pub fn idml_crate_dir() -> PathBuf {
    workspace_dir().join("crates/idml")
}
/// The schema directory named by `[package.metadata.idml] schema` in the
/// `idml` crate's manifest, resolved against the crate directory.
pub fn configured_schema_dir(crate_dir: &Path) -> Result<PathBuf, String> {
    let manifest = fs::read_to_string(crate_dir.join("Cargo.toml")).map_err(|e| e.to_string())?;
    let value = manifest
        .lines()
        .find_map(|l| l.trim().strip_prefix("schema").map(str::trim).and_then(|l| l.strip_prefix('=')))
        .map(|v| v.trim().trim_matches('"'))
        .ok_or("no `schema = \"…\"` in [package.metadata.idml]")?;
    Ok(crate_dir.join(value))
}

/// The directory of schema snapshots named by `[package.metadata.idml]
/// history`, if any.
pub fn configured_history_dir(crate_dir: &Path) -> Result<Option<PathBuf>, String> {
    configured(crate_dir, "history")
}

/// The upgrade rules file named by `[package.metadata.idml] upgrades`, if any.
pub fn configured_upgrades_file(crate_dir: &Path) -> Result<Option<PathBuf>, String> {
    configured(crate_dir, "upgrades")
}

fn configured(crate_dir: &Path, key: &str) -> Result<Option<PathBuf>, String> {
    let manifest = fs::read_to_string(crate_dir.join("Cargo.toml")).map_err(|e| e.to_string())?;
    let value = manifest
        .lines()
        .find_map(|l| l.trim().strip_prefix(key).map(str::trim).and_then(|l| l.strip_prefix('=')))
        .map(|v| v.trim().trim_matches('"'));
    Ok(value.map(|v| crate_dir.join(v)))
}

/// Directories holding only generated files: files in them that are not
/// generated any more are stale.
const GENERATED_DIRS: [&str; 2] = ["crates/idml/src/model", "docs/schema-changes"];

/// The generated files, by path relative to the workspace. With `history`,
/// the snapshots below it date every item, the `upgrades` rules are checked
/// against them, and the change reports are made.
pub fn generate(schema_dir: &Path, history: Option<&Path>, upgrades: Option<&Path>) -> Result<BTreeMap<String, String>, String> {
    let schema = schema::Schema::load(schema_dir)?;
    let mut model = lower::lower(&schema)?;
    let mut files = BTreeMap::new();
    let mut upgrades_rs = upgrade::rust(None, emit::HEADER);
    if let Some(root) = history {
        let history = history::History::new(history::snapshots(root)?);
        history::annotate(&mut model, &schema, &history)?;
        let rules = match upgrades {
            Some(file) => upgrade::parse(&fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?)?,
            None => Vec::new(),
        };
        let upgrades = upgrade::check(&history, rules)?;
        upgrades_rs = upgrade::rust(Some((&history, &upgrades)), emit::HEADER);
        files.insert("crates/idml/schema-history.tsv".to_owned(), history.tsv());
        for (name, text) in report::html(&history, &model, &upgrades) {
            files.insert(format!("docs/schema-changes/{name}"), text);
        }
    }
    files.insert("crates/idml/src/model/upgrades.rs".to_owned(), upgrades_rs);
    for (name, text) in emit::rust(&model)? {
        files.insert(format!("crates/idml/src/model/{name}"), text);
    }
    files.insert("crates/idml/schema-map.tsv".to_owned(), map::tsv(&model, &schema));
    Ok(files)
}

/// The files in the generated directories that are not generated.
fn extra(root: &Path, files: &BTreeMap<String, String>) -> Vec<String> {
    let mut extra = Vec::new();
    for dir in GENERATED_DIRS {
        let Ok(entries) = fs::read_dir(root.join(dir)) else { continue };
        for entry in entries.flatten() {
            let rel = format!("{dir}/{}", entry.file_name().to_string_lossy());
            if !files.contains_key(&rel) {
                extra.push(rel);
            }
        }
    }
    extra
}

/// Writes the generated files below the workspace `root` and removes stale
/// ones from the generated directories.
pub fn write(root: &Path, files: &BTreeMap<String, String>) -> Result<(), String> {
    for rel in extra(root, files) {
        fs::remove_file(root.join(&rel)).map_err(|e| format!("{rel}: {e}"))?;
    }
    for (rel, text) in files {
        let path = root.join(rel);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        fs::write(&path, text).map_err(|e| format!("{rel}: {e}"))?;
    }
    Ok(())
}

/// The generated files that differ from what is below the workspace `root`,
/// including stale files in the generated directories.
pub fn stale(root: &Path, files: &BTreeMap<String, String>) -> Vec<String> {
    let mut stale: Vec<String> =
        files.iter().filter(|(rel, text)| fs::read_to_string(root.join(rel)).ok().as_ref() != Some(*text)).map(|(rel, _)| rel.clone()).collect();
    stale.extend(extra(root, files));
    stale
}
