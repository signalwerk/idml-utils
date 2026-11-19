//! List the exact built-in schema/compatibility profiles.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("profile\tDOMVersion\tbuild\tform\tcorrections\tlocale\tplugins_complete");
    for p in idml::profile::SchemaCatalog::builtin()?.profiles() {
        println!(
            "{}\t{}\t{}\t{:?}\t{}\t{}\t{}",
            p.id,
            p.snapshot.dom_version,
            p.snapshot.build.as_deref().unwrap_or("unknown"),
            p.snapshot.form,
            p.amendments_sha256.is_some(),
            p.snapshot.locale.as_deref().unwrap_or("unknown"),
            p.snapshot.plugins_complete
        );
    }
    Ok(())
}
