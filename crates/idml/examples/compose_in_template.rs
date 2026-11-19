//! Add new content to a caller-supplied native template using explicit geometry.
//! Run: compose_in_template TEMPLATE VERSION SPREAD_PART SPREAD_ID TX TY OUTPUT [--source]
//! --source explicitly permits source writing without exact schema acceptance;
//! it still requires the template version to match and never relabels parts.
use idml::{
    Authoring, DomVersion, Package, ReadOptions,
    compose::{Paragraph, StoryBuilder, TextFrameBuilder, TextRun},
    geometry::{Rect, Transform},
    profile::SchemaCatalog,
};
use std::io::Write;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 7 && !(args.len() == 8 && args[7] == "--source") {
        return Err("usage: compose_in_template TEMPLATE VERSION SPREAD_PART SPREAD_ID TX TY OUTPUT [--source]".into());
    }
    let mut package = Package::read(&std::fs::read(&args[0])?, &ReadOptions::source())?.value;
    let version: DomVersion = args[1].parse()?;
    if package.document().dom_version().and_then(|v| v.parse::<DomVersion>().ok()) != Some(version) {
        return Err("template does not match the explicit version; convert separately before composing".into());
    }
    let layer = package.document().attribute(package.document().root(), "ActiveLayer").ok_or("template has no explicit active layer")?.to_owned();
    let transform = Transform::translation(args[4].parse()?, args[5].parse()?)?;
    let edit = |package: &mut Package| -> Result<(), idml::Error> {
        let run = TextRun::new("New IDML story from the public producer API").format(|a| a.set_point_size(&18.0))?.with_break();
        package.add_story(
            "Stories/imported-story.xml",
            StoryBuilder::new("imported-story")?.paragraph(Paragraph::new().run(run)),
            &ReadOptions::source(),
        )?;
        let frame = TextFrameBuilder::rectangle("imported-frame", "imported-story", Rect::new(0.0, 0.0, 364.0, 164.0)?, transform)?
            .format(|a| a.set_item_layer(&layer))?;
        package.add_text_frame(&args[2], &args[3], frame, &ReadOptions::source())?;
        Ok(())
    };
    let bytes = if args.len() == 8 {
        edit(&mut package)?;
        package.write()?
    } else {
        let catalog = SchemaCatalog::builtin()?;
        let mut author = Authoring::from_package(catalog, catalog.effective_profile(version)?, package)?;
        author.plan_edit(edit)?.apply(&mut author)?;
        author.write()?
    };
    std::fs::OpenOptions::new().write(true).create_new(true).open(&args[6])?.write_all(&bytes)?;
    Ok(())
}
