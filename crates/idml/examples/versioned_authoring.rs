//! Start at 8.0 and explicitly adopt the ColorGroup vocabulary in 10.0.
use idml::profile::SchemaCatalog;
use idml::{Authoring, LossPolicy, ReadOptions, view::Builder, views};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let catalog = SchemaCatalog::builtin()?;
    let mut document = Authoring::for_version(catalog, "8.0".parse()?, "document")?;
    let draft = document.plan_edit(|package| {
        let mut group = Builder::<views::ColorGroup>::new();
        group.set_self_(&"colors".to_owned())?;
        group.set_name(&"Colors".to_owned())?;
        group.set_is_root_color_group(&false)?;
        let graphic = Builder::<views::GraphicFile>::new();
        package.add_part("Resources/Graphic.xml", &graphic.build(), &ReadOptions::default())?;
        let tree = package.document_mut()?;
        let root = tree.root();
        let index = tree.elements(root).count();
        tree.insert_element(root, index, &group.build())?;
        Ok(())
    })?;
    println!("Draft valid at 8.0: {}", draft.can_apply());
    for candidate in draft.compatible_upgrades()? {
        println!("Compatible observed snapshot: {} ({})", candidate.snapshot.dom_version, candidate.id);
    }
    // The caller chooses 10.0. Neither editing nor writing chooses the newest.
    let plan = draft.for_target(catalog.effective_profile("10.0".parse()?)?, LossPolicy::Forbid)?;
    plan.apply(&mut document)?;
    let bytes = document.write()?;
    println!("Wrote {} bytes at {}", bytes.len(), document.profile().snapshot.dom_version);
    Ok(())
}
