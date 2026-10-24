//! Test helpers: the configured schema and structural XML comparison.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use idml::PartFile;
use idml::model::*;
use idml_codegen::schema::Schema;

pub fn schema() -> Schema {
    let dir = idml_codegen::configured_schema_dir(&idml_codegen::idml_crate_dir()).unwrap();
    Schema::load(&dir).unwrap()
}

/// Reads a part document with the generated type for its schema file and
/// writes it again; also returns the warnings.
pub type RoundTrip = fn(&str, &idml::ReadOptions) -> Result<(String, Vec<idml::Warning>), String>;

fn round_trip<T: PartFile>(xml: &str, options: &idml::ReadOptions) -> Result<(String, Vec<idml::Warning>), String> {
    let parsed = T::from_xml(xml, options).map_err(|e| e.to_string())?;
    Ok((parsed.value.to_xml().map_err(|e| e.to_string())?, parsed.warnings))
}

/// The reader for a part schema file, by its path without extension
/// (older snapshots have `.rng` files).
pub fn part(file: &str) -> Option<RoundTrip> {
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    PARTS.iter().find(|(f, _)| f.strip_suffix(".rnc") == Some(stem)).map(|(_, r)| *r)
}

/// The part types by schema file. A new part schema fails `parts_are_complete`.
pub const PARTS: &[(&str, RoundTrip)] = &[
    ("MasterSpreads/MasterSpread.rnc", round_trip::<MasterSpreadFile>),
    ("Resources/Fonts.rnc", round_trip::<FontsFile>),
    ("Resources/Graphic.rnc", round_trip::<GraphicFile>),
    ("Resources/Preferences.rnc", round_trip::<PreferencesFile>),
    ("Resources/Styles.rnc", round_trip::<StylesFile>),
    ("Spreads/Spread.rnc", round_trip::<SpreadFile>),
    ("Stories/Story.rnc", round_trip::<StoryFile>),
    ("XML/BackingStory.rnc", round_trip::<BackingStoryFile>),
    ("XML/Mapping.rnc", round_trip::<MappingFile>),
    ("XML/Tags.rnc", round_trip::<TagsFile>),
];

/// An element as XML infoset data: expanded name, attributes, element
/// children in order, and text (with `<?ACE n?>` as its character) when it
/// has no element children.
#[derive(Debug, PartialEq)]
pub struct Canon {
    name: String,
    attrs: BTreeMap<String, String>,
    children: Vec<Canon>,
    text: String,
}

pub fn canon(xml: &str) -> Result<Canon, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
    Ok(canon_node(doc.root_element()))
}

fn canon_node(n: roxmltree::Node) -> Canon {
    let name = match n.tag_name().namespace() {
        Some(ns) => format!("{{{ns}}}{}", n.tag_name().name()),
        None => n.tag_name().name().to_owned(),
    };
    let attrs = n.attributes().map(|a| (a.name().to_owned(), a.value().to_owned())).collect();
    let children: Vec<Canon> = n.children().filter(|c| c.is_element()).map(canon_node).collect();
    let mut text = String::new();
    if children.is_empty() {
        for c in n.children() {
            if c.is_text() {
                text.push_str(c.text().unwrap_or_default());
            } else if let Some(pi) = c.pi().filter(|pi| pi.target == "ACE") {
                let code = u32::from_str_radix(pi.value.unwrap_or_default().trim(), 16).unwrap();
                text.push(char::from_u32(code).unwrap());
            }
        }
    }
    Canon { name, attrs, children, text }
}

/// The path to the first difference, or `None`.
pub fn difference(a: &Canon, b: &Canon) -> Option<String> {
    if a.name != b.name {
        return Some(format!("<{}> vs <{}>", a.name, b.name));
    }
    if a.attrs != b.attrs {
        for (k, v) in &a.attrs {
            if b.attrs.get(k) != Some(v) {
                return Some(format!("{}/@{k}: {v:?} vs {:?}", a.name, b.attrs.get(k)));
            }
        }
        let extra: Vec<&String> = b.attrs.keys().filter(|k| !a.attrs.contains_key(*k)).collect();
        return Some(format!("{}: extra attributes {extra:?}", a.name));
    }
    if a.text != b.text {
        return Some(format!("{}: text {:?} vs {:?}", a.name, a.text, b.text));
    }
    for (i, (x, y)) in a.children.iter().zip(&b.children).enumerate() {
        if let Some(d) = difference(x, y) {
            return Some(format!("{}/[{i}]{d}", a.name));
        }
    }
    if a.children.len() != b.children.len() {
        let names = |c: &Canon| c.children.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        return Some(format!("{}: children {:?} vs {:?}", a.name, names(a), names(b)));
    }
    None
}

pub const CONTAINER: &str = r#"<?xml version="1.0" encoding="UTF-8"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#;

pub fn zip(files: &[(String, String)]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, text) in files {
        w.start_file(name.as_str(), stored).unwrap();
        w.write_all(text.as_bytes()).unwrap();
    }
    w.finish().unwrap().into_inner()
}

pub fn unzip(bytes: &[u8]) -> BTreeMap<String, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).unwrap();
        let mut text = String::new();
        f.read_to_string(&mut text).unwrap();
        out.insert(f.name().unwrap().into_owned(), text);
    }
    out
}
