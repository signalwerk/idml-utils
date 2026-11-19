//! Audit real packages: cargo run --release -p idml --example corpus -- [OPTIONS] DIR …
//!
//! `--roundtrip` compares decompressed entry contents after writing. In source
//! mode, `--validate` resolves each designmap DOMVersion to an exact effective
//! profile and validates against it, and `--semantic` reports identity, reference,
//! thread and external-link observations. Each is a separate operation; a missing
//! profile or semantic observation is not a read failure or source loss.
use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Read},
    path::Path,
    process::ExitCode,
};

fn contents(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, Box<dyn std::error::Error>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut files = BTreeMap::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name()?.into_owned();
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        files.insert(name, data);
    }
    Ok(files)
}

fn visit(path: &Path, files: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if !entry.file_type()?.is_symlink() {
                visit(&entry.path(), files)?;
            }
        }
    } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("idml")) {
        files.push(path.to_owned());
    }
    Ok(())
}

enum AuditPackage {
    #[cfg(feature = "owned")]
    Owned(Box<idml::OwnedPackage>),
    Source(idml::Package),
}
impl AuditPackage {
    fn read(bytes: &[u8], options: &idml::ReadOptions, source: bool) -> Result<idml::Parsed<Self>, idml::Error> {
        if source {
            idml::Package::read(bytes, options).map(|p| idml::Parsed { value: Self::Source(p.value), warnings: p.warnings })
        } else {
            #[cfg(feature = "owned")]
            {
                idml::OwnedPackage::read(bytes, options).map(|p| idml::Parsed { value: Self::Owned(Box::new(p.value)), warnings: p.warnings })
            }
            #[cfg(not(feature = "owned"))]
            {
                Err(idml::Error::new(idml::ErrorKind::Package, "--owned requires cargo feature owned"))
            }
        }
    }
    fn source(&self) -> Option<&idml::Package> {
        match self {
            #[cfg(feature = "owned")]
            Self::Owned(_) => None,
            Self::Source(p) => Some(p),
        }
    }
    fn write(&self) -> Result<Vec<u8>, idml::Error> {
        match self {
            #[cfg(feature = "owned")]
            Self::Owned(p) => p.write(),
            Self::Source(p) => p.write(),
        }
    }
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn diagnostic(file: &Path, representation: &str, mode: &str, kind: &str, location: &idml::Location, message: &str) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        clean(&file.to_string_lossy()),
        representation,
        mode,
        kind,
        clean(&location.part),
        clean(&location.element),
        location.line,
        location.column,
        clean(message)
    )
}

/// An element path without sibling indices, for grouping similar diagnostics.
fn grouped(path: &str) -> String {
    path.split('/').map(|s| s.split('[').next().unwrap_or(s)).collect::<Vec<_>>().join("/")
}

/// Grouped counts per operation, printed to stderr after the per-file details.
#[derive(Default)]
struct Totals {
    groups: BTreeMap<(&'static str, String), usize>,
}
impl Totals {
    fn add(&mut self, operation: &'static str, key: impl Into<String>) {
        *self.groups.entry((operation, key.into())).or_default() += 1;
    }
    fn print(&self, operation: &'static str, limit: usize) {
        let mut rows: Vec<_> = self.groups.iter().filter(|((op, _), _)| *op == operation).collect();
        rows.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        for ((_, key), count) in rows.iter().take(limit) {
            eprintln!("{count}\t{operation}\t{key}");
        }
        if rows.len() > limit {
            eprintln!("…\t{operation}\t{} more groups in the TSV details", rows.len() - limit);
        }
    }
}

/// Resolve the designmap version to an exact effective profile and validate the
/// package against it. Unlabelled, unrecognized, ambiguous and unobserved
/// versions are reported without choosing a profile.
fn audit_validation(file: &Path, package: &idml::Package, catalog: &idml::profile::SchemaCatalog, totals: &mut Totals) {
    use idml::profile::{ProfileResolution, Support};
    let label = package.document().dom_version();
    let row = |kind: &str, location: &idml::Location, message: &str| println!("{}", diagnostic(file, "source", "validate", kind, location, message));
    let unresolved = |kind: &str, message: &str| println!("{}", diagnostic(file, "source", "profile", kind, &idml::Location::default(), message));
    let Some(label) = label else {
        unresolved("Unlabelled", "designmap has no DOMVersion");
        totals.add("profile", "unlabelled");
        return;
    };
    let version: idml::DomVersion = match label.parse() {
        Ok(version) => version,
        Err(_) => {
            unresolved("Unrecognized", &format!("DOMVersion {label}"));
            totals.add("profile", "unrecognized label");
            return;
        }
    };
    let profile = match catalog.resolve(version, true) {
        ProfileResolution::Exact(profile) => profile,
        ProfileResolution::Ambiguous(candidates) => {
            let ids: Vec<_> = candidates.iter().map(|p| p.id.as_str()).collect();
            unresolved("Ambiguous", &format!("DOMVersion {version}: {}", ids.join(", ")));
            totals.add("profile", format!("ambiguous {version}"));
            return;
        }
        ProfileResolution::Unknown { before, after } => {
            let bound = |v: Option<idml::DomVersion>| v.map_or_else(|| "none".to_owned(), |v| v.to_string());
            unresolved("Unobserved", &format!("DOMVersion {version}: no snapshot (nearest {} / {})", bound(before), bound(after)));
            totals.add("profile", format!("unobserved {version}"));
            return;
        }
    };
    totals.add("profile", format!("exact {}", profile.id));
    let report = package.validate(catalog, profile);
    let count = |support: Support| report.issues.iter().filter(|i| i.support == support).count();
    let (unsupported, unknown) = (count(Support::Unsupported), count(Support::Unknown));
    row(&format!("{:?}", report.support()), &idml::Location::default(), &format!("{}: {unsupported} unsupported, {unknown} unknown", profile.id));
    totals.add("validate", format!("{:?} {}", report.support(), profile.id));
    for issue in &report.issues {
        row(&format!("{:?}", issue.support), &issue.location, &issue.message);
        totals.add("issue", format!("{:?}\t{}\t{}", issue.support, grouped(&issue.location.element), issue.message));
    }
}

/// Opt-in semantic observations. Only non-resolved references and thread issues
/// get detail rows; unresolved references are not claims of document invalidity.
fn audit_semantics(file: &Path, package: &idml::Package, totals: &mut Totals) {
    use idml::semantic::ReferenceResolution;
    let graph = package.semantic_graph();
    let row = |kind: &str, location: &idml::Location, message: &str| println!("{}", diagnostic(file, "source", "semantic", kind, location, message));
    let location = |source: &idml::semantic::SourceNode| source.location(package).unwrap_or_default();
    for duplicate in graph.duplicates() {
        let first = duplicate.objects.first().and_then(|i| graph.object(*i));
        row(
            "duplicate",
            &first.map(|o| location(&o.source)).unwrap_or_default(),
            &format!("Self {} on {} elements", duplicate.self_id, duplicate.objects.len()),
        );
        totals.add("semantic", "duplicate identity");
    }
    for reference in graph.references() {
        let class = match &reference.resolution {
            ReferenceResolution::Resolved(_) => "resolved",
            ReferenceResolution::Unlinked => "nil",
            ReferenceResolution::Unresolved => "unresolved",
            ReferenceResolution::Ambiguous(_) => "ambiguous",
            ReferenceResolution::UnexpectedTarget(_) => "unexpected target",
        };
        totals.add("reference", format!("{}\t{class}", reference.attribute_name()));
        if !matches!(reference.resolution, ReferenceResolution::Resolved(_) | ReferenceResolution::Unlinked) {
            let target = match &reference.resolution {
                ReferenceResolution::UnexpectedTarget(i) => graph.object(*i).map(|o| format!(" ({})", o.element)).unwrap_or_default(),
                _ => String::new(),
            };
            row(class, &location(&reference.source), &format!("{}={}{target}", reference.attribute_name(), reference.target));
        }
    }
    let threads = graph.text_threads();
    totals.add("semantic", format!("files with {} threads", if threads.is_empty() { "no" } else { "some" }));
    for thread in &threads {
        totals.add("thread", format!("{} frame(s)", thread.frames.len().min(5)).replace("5 frame(s)", "5+ frame(s)"));
        for issue in &thread.issues {
            let name = format!("{issue:?}");
            let name = name.split([' ', '{']).next().unwrap_or("issue").to_owned();
            let frame = thread.frames.first().and_then(|i| graph.object(*i));
            row("thread", &frame.map(|o| location(&o.source)).unwrap_or_default(), &format!("{issue:?}"));
            totals.add("thread", format!("issue {name}"));
        }
    }
    for link in graph.external_links() {
        totals.add("link", format!("{}\t{}", link.scheme().unwrap_or("(none)"), link.stored_state.as_deref().unwrap_or("(absent)")));
    }
}

fn main() -> ExitCode {
    let mut files = Vec::new();
    let mut roundtrip = false;
    let mut validate = false;
    let mut semantic = false;
    let mut source = true;
    for dir in std::env::args_os().skip(1) {
        if dir == "--validate" {
            validate = true;
            continue;
        }
        if dir == "--semantic" {
            semantic = true;
            continue;
        }
        if dir == "--owned" {
            source = false;
            continue;
        }
        if dir == "--source" {
            source = true;
            continue;
        }
        if dir == "--roundtrip" {
            roundtrip = true;
            continue;
        }
        if let Err(e) = visit(Path::new(&dir), &mut files) {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    }
    files.sort();
    files.dedup();
    if files.is_empty() {
        eprintln!("usage: corpus DIR … (no IDML files found)");
        return ExitCode::FAILURE;
    }
    if !source && (validate || semantic) {
        eprintln!("--validate and --semantic audit the source representation; omit --owned");
        return ExitCode::FAILURE;
    }
    let catalog = match idml::profile::SchemaCatalog::builtin() {
        Ok(catalog) => catalog,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let mut totals = Totals::default();
    let representation = if source { "source" } else { "owned" };
    if source {
        eprintln!("Source mode: XML and package structure are parsed; schema acceptance is separate.");
    } else {
        eprintln!("Owned mode: projection uses the configured owned schema, not an exact profile selected from the document version.");
    }
    let options = if source { idml::ReadOptions::source() } else { idml::ReadOptions::projection() };
    let lenient_options = idml::ReadOptions { mode: idml::Mode::Lenient, ..options.clone() };
    println!("file\trepresentation\tmode\tkind\tpart\tpath\tline\tcolumn\tmessage");
    let mut strict = 0;
    let mut lenient = 0;
    let mut identical = 0;
    let mut warning_count = 0;
    let mut summaries = BTreeMap::<(String, String), usize>::new();
    for path in &files {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                continue;
            }
        };
        let (parsed, mode) = match AuditPackage::read(&bytes, &options, source) {
            Ok(p) => {
                strict += 1;
                (Some(p), "strict")
            }
            Err(e) => {
                let location = e.location().cloned().unwrap_or_default();
                println!("{}", diagnostic(path, representation, "strict", &format!("{:?}", e.kind()), &location, e.message()));
                *summaries.entry((grouped(&location.element), e.message().into())).or_default() += 1;
                match AuditPackage::read(&bytes, &lenient_options, source) {
                    Ok(p) => {
                        lenient += 1;
                        (Some(p), "lenient")
                    }
                    Err(e) => {
                        let l = e.location().cloned().unwrap_or_default();
                        println!("{}", diagnostic(path, representation, "lenient", &format!("{:?}", e.kind()), &l, e.message()));
                        (None, "lenient")
                    }
                }
            }
        };
        if let Some(p) = &parsed {
            warning_count += p.warnings.len();
            for warning in &p.warnings {
                println!("{}", diagnostic(path, representation, mode, "warning", &warning.location, &warning.message));
            }
            if let Some(package) = p.value.source() {
                if validate {
                    audit_validation(path, package, catalog, &mut totals);
                }
                if semantic {
                    audit_semantics(path, package, &mut totals);
                }
            }
        }
        if roundtrip && let Some(parsed) = parsed {
            match parsed.value.write() {
                Ok(written) => match (contents(&bytes), contents(&written)) {
                    (Ok(a), Ok(b)) if a == b => identical += 1,
                    (Ok(a), Ok(b)) => {
                        let changed: Vec<_> = a
                            .keys()
                            .chain(b.keys())
                            .filter(|p| a.get(*p) != b.get(*p))
                            .collect::<std::collections::BTreeSet<_>>()
                            .into_iter()
                            .cloned()
                            .collect();
                        println!("{}", diagnostic(path, representation, "roundtrip", "diff", &idml::Location::default(), &changed.join(", ")));
                    }
                    (Err(e), _) | (_, Err(e)) => eprintln!("{}: {e}", path.display()),
                },
                Err(e) => println!(
                    "{}",
                    diagnostic(
                        path,
                        representation,
                        "roundtrip",
                        &format!("{:?}", e.kind()),
                        &e.location().cloned().unwrap_or_default(),
                        e.message()
                    )
                ),
            }
        }
    }
    eprintln!("{} files: {strict} strict, {lenient} additional lenient, {} failed", files.len(), files.len() - strict - lenient);
    eprintln!("{warning_count} read warnings ({representation} representation)");
    if roundtrip {
        eprintln!("{identical}/{} readable packages keep every entry's contents byte for byte", strict + lenient);
    }
    let mut summaries: Vec<_> = summaries.into_iter().collect();
    summaries.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    for ((path, message), count) in summaries {
        eprintln!("{count}\t{path}\t{message}");
    }
    if validate {
        totals.print("profile", 50);
        totals.print("validate", 50);
        totals.print("issue", 40);
    }
    if semantic {
        totals.print("semantic", 20);
        totals.print("reference", 100);
        totals.print("thread", 30);
        totals.print("link", 30);
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture(files: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in files {
            writer.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn validation_and_semantic_operations_record_separate_outcomes() {
        let catalog = idml::profile::SchemaCatalog::builtin().unwrap();
        let mut totals = Totals::default();
        let has = |totals: &Totals, operation: &str, prefix: &str| totals.groups.keys().any(|(op, key)| *op == operation && key.starts_with(prefix));
        audit_validation(Path::new("a.idml"), &idml::Package::new("21.5".parse().unwrap(), "d").unwrap(), catalog, &mut totals);
        assert!(has(&totals, "profile", "exact 21.5/"));
        assert!(has(&totals, "validate", "Supported 21.5/"));
        // An unobserved version is reported, never validated against a neighbour.
        audit_validation(Path::new("b.idml"), &idml::Package::new("42.0".parse().unwrap(), "d").unwrap(), catalog, &mut totals);
        assert!(has(&totals, "profile", "unobserved 42.0"));
        assert!(!has(&totals, "validate", "Supported 42.0"));

        let mut package = idml::Package::new("21.5".parse().unwrap(), "d").unwrap();
        package.add_story("Stories/s.xml", idml::compose::StoryBuilder::new("s").unwrap(), &idml::ReadOptions::source()).unwrap();
        let root = package.document().root();
        package.document_mut().unwrap().set_attribute(root, "StoryList", "s missing").unwrap();
        audit_semantics(Path::new("c.idml"), &package, &mut totals);
        assert_eq!(totals.groups.get(&("reference", "StoryList\tresolved".into())), Some(&1));
        assert_eq!(totals.groups.get(&("reference", "StoryList\tunresolved".into())), Some(&1));
        assert!(has(&totals, "semantic", "files with no threads"));
    }

    #[test]
    fn source_audit_retains_and_locates_lenient_envelope_warnings() {
        let bytes = fixture(&[("designmap.xml", "<Document DOMVersion='21.5' Self='d'/>")]);
        assert!(AuditPackage::read(&bytes, &idml::ReadOptions::source(), true).is_err());
        let parsed = AuditPackage::read(&bytes, &idml::ReadOptions::lenient(), true).unwrap();
        assert_eq!(parsed.warnings.len(), 2);
        assert_eq!(parsed.warnings[0].location.part, "mimetype");
        assert_eq!(parsed.warnings[1].location.part, "META-INF/container.xml");
        let warning = &parsed.warnings[0];
        let row = diagnostic(Path::new("with\ttab.idml"), "source", "lenient", "warning", &warning.location, &warning.message);
        let columns = row.split('\t').collect::<Vec<_>>();
        assert_eq!(columns.len(), 9);
        assert_eq!(&columns[..5], &["with tab.idml", "source", "lenient", "warning", "mimetype"]);
        assert_eq!(columns[8], warning.message);
    }

    #[cfg(feature = "owned")]
    #[test]
    fn owned_audit_retains_projection_warnings() {
        let document = "<Document xmlns:idPkg='http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging' DOMVersion='21.5' Self='d'><Hyperlink Self='h' Source='s'><Properties><Destination type='future'><PluginPayload/></Destination></Properties></Hyperlink></Document>";
        let container = "<container xmlns='urn:oasis:names:tc:opendocument:xmlns:container'><rootfiles><rootfile full-path='designmap.xml'/></rootfiles></container>";
        let bytes = fixture(&[("mimetype", idml::MIMETYPE), ("META-INF/container.xml", container), ("designmap.xml", document)]);
        let parsed = AuditPackage::read(&bytes, &idml::ReadOptions::lenient(), false).unwrap();
        assert!(parsed.warnings.iter().any(|warning| warning.message.contains("typed projection") && warning.location.part == "designmap.xml"));
    }
}
