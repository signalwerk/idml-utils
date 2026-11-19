//! Inspect identities, relationships, text threads and external links without
//! changing a package or selecting a schema.
//! Run: `cargo run -p idml --example semantic_inspection -- document.idml`

use idml::semantic::ReferenceResolution;
use idml::{Package, ReadOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os().nth(1).ok_or("usage: semantic_inspection document.idml")?;
    let bytes = std::fs::read(path)?;
    let parsed = Package::read(&bytes, &ReadOptions::source())?;
    for warning in &parsed.warnings {
        eprintln!("{}: {}", warning.location, warning.message);
    }
    let package = parsed.value;
    let document = package.source_document();
    let flows = document.text_flows();
    let graph = flows.graph();
    println!(
        "{} stories; {} text containers in {} threads; {} external links",
        document.stories().count(),
        document.text_containers().count(),
        flows.threads().len(),
        graph.external_links().len()
    );
    for duplicate in graph.duplicates() {
        println!("duplicate identity {:?}: {} occurrences", duplicate.self_id, duplicate.objects.len());
    }
    for reference in graph.references() {
        if matches!(reference.resolution, ReferenceResolution::Resolved(_) | ReferenceResolution::Unlinked) {
            continue;
        }
        if let Some(location) = reference.source.location(&package) {
            println!("{} @{}={:?}: {:?}", location, reference.attribute_name(), reference.target, reference.resolution);
        }
    }
    for thread in flows.threads().iter().filter(|t| t.frames.len() > 1 || !t.issues.is_empty()) {
        let frames: Vec<_> = flows.containers(thread).iter().map(|c| format!("{:?}", c.self_id())).collect();
        let story = flows.story(thread).map(|s| format!("{:?}", s.self_id()));
        println!("thread {} story {}: {:?}", frames.join(" -> "), story.as_deref().unwrap_or("(unresolved)"), thread.issues);
    }
    for link in graph.external_links() {
        // Stored URIs are not resolved; availability depends on the opening environment.
        println!("link {} ({})", link.uri, link.stored_state.as_deref().unwrap_or("no StoredState"));
    }
    Ok(())
}
