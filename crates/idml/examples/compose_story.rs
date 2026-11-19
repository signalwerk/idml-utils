//! Produce a minimal source package at an explicitly chosen exact profile.
//! This demonstrates story authoring; it does not claim a complete page layout.
//! Run: cargo run -p idml --example compose_story -- 8.0 target/story.idml
use idml::{
    Authoring, ReadOptions,
    compose::{Paragraph, StoryBuilder, TextRun},
    profile::SchemaCatalog,
};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let version = args.next().ok_or("usage: compose_story VERSION OUTPUT.idml")?;
    let output = args.next().ok_or("usage: compose_story VERSION OUTPUT.idml")?;
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let catalog = SchemaCatalog::builtin()?;
    let mut author = Authoring::for_version(catalog, version.parse()?, "document")?;
    author
        .plan_edit(|package| {
            let run = TextRun::new("Hello from idml-utils").format(|a| a.set_point_size(&18.0))?.with_break();
            package.add_story("Stories/body.xml", StoryBuilder::new("body")?.paragraph(Paragraph::new().run(run)), &ReadOptions::source())?;
            Ok(())
        })?
        .apply(&mut author)?;
    std::fs::OpenOptions::new().write(true).create_new(true).open(output)?.write_all(&author.write()?)?;
    Ok(())
}
