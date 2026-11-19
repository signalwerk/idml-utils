//! Preserve a source package, then edit its first ordinary story text node.
//! Run: cargo run -p idml --example edit_source_text -- input.idml noop.idml edited.idml
//! Outputs must not already exist. This demonstrates source editing; it does not
//! validate the edit against an exact profile or claim visual equivalence.
use idml::document::StoryEvent;
use idml::{Field, Package, ReadOptions};
use std::io::Write;

fn write_new(path: &std::ffi::OsStr, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let input = args.next().ok_or("usage: edit_source_text INPUT NOOP EDITED")?;
    let noop = args.next().ok_or("missing no-op output")?;
    let edited = args.next().ok_or("missing edited output")?;
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let parsed = Package::read(&std::fs::read(input)?, &ReadOptions::source())?;
    for warning in &parsed.warnings {
        eprintln!("{}: {}", warning.location, warning.message);
    }
    let mut package = parsed.value;
    let selected = package.source_document().stories().flat_map(|story| story.content()).find_map(|event| {
        let StoryEvent::Text { source, value: Field::Known(text) } = &event else { return None };
        if !text.chars().any(char::is_alphanumeric) {
            return None;
        }
        Some((source.part().to_owned(), text.chars().count(), event.replace_text(format!("{text} [idml-utils native probe]"))))
    });
    let (part, original_len, replacement) = selected.ok_or("no ordinary story text found")?;
    let replacement = replacement?;
    write_new(&noop, &package.write()?)?;
    package.replace_text(&[replacement])?;
    write_new(&edited, &package.write()?)?;
    println!("edited one Content element in {part}; original length {original_len} characters");
    Ok(())
}
