//! Package-level behaviour: building, reading, writing and diagnostics.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use idml::model::*;
use idml::{ErrorKind, Mode, Package, Part, PartFile, PartPath, ReadOptions};

fn zip(files: &[(&str, &str)]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, text) in files {
        w.start_file(*name, stored).unwrap();
        w.write_all(text.as_bytes()).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn unzip(bytes: &[u8]) -> BTreeMap<String, String> {
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

const CONTAINER: &str = r#"<?xml version="1.0" encoding="UTF-8"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#;

fn story(text: &str) -> StoryFile {
    let mut csr = CharacterStyleRange { applied_character_style: Some("CharacterStyle/$ID/[No character style]".into()), ..Default::default() };
    csr.children.push(CharacterStyleRangeChild::Content(text.into()));
    csr.children.push(CharacterStyleRangeChild::Br);
    let mut psr = ParagraphStyleRange::default();
    psr.children.push(ParagraphStyleRangeChild::CharacterStyleRange(Box::new(csr)));
    let mut story = Story::new("u1");
    story.children.push(StoryChild::ParagraphStyleRange(Box::new(psr)));
    StoryFile { story: vec![story] }
}

#[test]
fn a_package_built_in_code_round_trips() {
    let mut document = Document::new("d");
    document.story_list = Some(vec![idml::Token::new("u1").unwrap()]);
    document.story.push(Part::new(PartPath::new("Stories/Story_u1.xml").unwrap(), story("Tab\there, <&> and \u{7} indent")));
    document.graphic = Some(GraphicFile::default());
    let package = Package::new(document);

    let bytes = package.write().unwrap();
    let files = unzip(&bytes);
    assert_eq!(files.keys().next().map(String::as_str), Some("META-INF/container.xml"));
    assert!(bytes.get(30..38) == Some(b"mimetype"), "mimetype comes first");
    assert!(files["designmap.xml"].contains("<?aid style=\"50\""));
    assert!(files["designmap.xml"].contains(r#"<idPkg:Story src="Stories/Story_u1.xml"/>"#));
    assert!(files["Stories/Story_u1.xml"].contains("<Content>Tab\there, &lt;&amp;&gt; and <?ACE 7?> indent</Content>"));

    let read = Package::read(&bytes, &ReadOptions::default()).unwrap();
    assert!(read.warnings.is_empty(), "{:?}", read.warnings);
    assert_eq!(read.value, package);
}

#[test]
fn strict_reading_reports_where_the_schema_is_violated() {
    let xml = "<idPkg:Story xmlns:idPkg=\"http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging\" DOMVersion=\"21.5\">\n\
               <Story Self=\"u1\" Unknown=\"x\"><ParagraphStyleRange/></Story></idPkg:Story>";
    let e = StoryFile::from_xml(xml, &ReadOptions::default()).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Schema);
    assert!(e.message().contains("Unknown"), "{e}");
    let at = e.location().unwrap();
    assert_eq!((at.line, at.element.as_str()), (2, "idPkg:Story/Story"));

    let lenient = StoryFile::from_xml(xml, &ReadOptions::lenient()).unwrap();
    assert_eq!(lenient.warnings.len(), 1);
    assert_eq!(lenient.value.story[0].self_, "u1");
}

#[test]
fn lenient_reading_accepts_newer_versions_and_skips_unknown_content() {
    let xml = "<idPkg:Story xmlns:idPkg=\"http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging\" DOMVersion=\"22.0\">\
               <Story Self=\"u1\" FillTint=\"not a number\"><FutureElement/><StoryPreference OpticalMarginAlignment=\"true\"/></Story>\
               </idPkg:Story>";
    assert!(StoryFile::from_xml(xml, &ReadOptions::default()).is_err());
    let read = StoryFile::from_xml(xml, &ReadOptions { mode: Mode::Lenient, ..ReadOptions::default() }).unwrap();
    assert_eq!(read.warnings.len(), 3, "{:#?}", read.warnings);
    let story = &read.value.story[0];
    assert_eq!(story.fill_tint, None);
    assert_eq!(story.story_preference.as_ref().and_then(|p| p.optical_margin_alignment), Some(true));
    assert!(read.value.to_xml().unwrap().contains("DOMVersion=\"21.5\""));
}

#[test]
fn older_versions_are_upgraded_while_reading() {
    let xml = "<idPkg:Preferences xmlns:idPkg=\"http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging\" DOMVersion=\"7.5\">\n\
               <HTMLExportPreference NumberedListExportOption=\"StaticOrderedList\" CSSExportOption=\"ExternalCSS\"/></idPkg:Preferences>";
    let read = PreferencesFile::from_xml(xml, &ReadOptions::default()).unwrap();
    let html = read.value.html_export_preference.as_ref().unwrap();
    assert_eq!(html.numbered_list_export_option, Some(NumberedListExportOption::OrderedList));
    assert_eq!(html.css_export_option, None);
    let [warning] = read.warnings.as_slice() else { panic!("{:?}", read.warnings) };
    assert_eq!(
        warning.to_string(),
        "<xml>:2:68 (idPkg:Preferences/HTMLExportPreference): attribute CSSExportOption=\"ExternalCSS\" left out: not an option since 8.0"
    );
    assert!(read.value.to_xml().unwrap().contains("DOMVersion=\"21.5\""));
}

#[test]
fn values_outside_the_schema_cannot_be_constructed() {
    assert!(Double0To100::new(100.5).is_err());
    assert!(Double0To100::new(f64::NAN).is_err());
    assert_eq!(Double0To100::new(42.0).map(Double0To100::get), Ok(42.0));
    assert!("Sideways".parse::<AnchorPoint>().is_err());
    assert_eq!("TopLeftAnchor".parse::<AnchorPoint>(), Ok(AnchorPoint::TopLeftAnchor));
    assert_eq!(AnchorPoint::ALL.len(), 9);
    assert!(PartPath::new("../evil.xml").is_err());
    assert!(idml::Token::new("two words").is_err());
}

#[test]
fn writing_rejects_what_xml_cannot_hold_and_duplicate_paths() {
    let mut story = Story::new("u1");
    story.story_title = Some("\u{FFFE}".into());
    let e = StoryFile { story: vec![story] }.to_xml().unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Package);

    let mut document = Document::new("d");
    let path = PartPath::new("Stories/Story_u1.xml").unwrap();
    document.story.push(Part::new(path.clone(), StoryFile::default()));
    document.story.push(Part::new(path, StoryFile::default()));
    assert_eq!(Package::new(document).write().unwrap_err().kind(), ErrorKind::Package);
}

#[test]
fn nesting_is_limited_before_parsing() {
    let nested = |n: usize| {
        let mut xml =
            String::from(r#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="21.5"><Spread Self="s">"#);
        for i in 0..n {
            xml.push_str(&format!(r#"<Group Self="g{i}">"#));
        }
        xml.push_str(&"</Group>".repeat(n));
        xml + "</Spread></idPkg:Spread>"
    };
    let limit = ReadOptions::default().max_depth;
    // At the limit, in a thread with the default 2 MiB test stack, even
    // unoptimized.
    let at_limit = nested(limit - 2);
    let read = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || SpreadFile::from_xml(&at_limit, &ReadOptions::default()).map(|r| r.value))
        .unwrap()
        .join()
        .unwrap();
    assert!(read.is_ok());
    let e = SpreadFile::from_xml(&nested(100_000), &ReadOptions::default()).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Limit);
}

#[test]
fn unreferenced_files_are_kept() {
    let designmap = r#"<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="21.5" Self="d"/>"#;
    let bytes = zip(&[("mimetype", idml::MIMETYPE), ("META-INF/container.xml", CONTAINER), ("designmap.xml", designmap), ("Extra/notes.txt", "hi")]);
    let read = Package::read(&bytes, &ReadOptions::default()).unwrap();
    assert_eq!(read.warnings.len(), 1);
    assert_eq!(read.value.other_files.get("Extra/notes.txt").map(Vec::as_slice), Some(&b"hi"[..]));
    assert_eq!(unzip(&read.value.write().unwrap())["Extra/notes.txt"], "hi");
}

#[test]
fn lenient_warnings_are_bounded() {
    let attrs: String = (0..5000).map(|i| format!(" X{i}=\"1\"")).collect();
    let xml = format!(
        "<idPkg:Story xmlns:idPkg=\"http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging\" DOMVersion=\"21.5\"><Story Self=\"u1\"{attrs}/></idPkg:Story>"
    );
    let read = StoryFile::from_xml(&xml, &ReadOptions::lenient()).unwrap();
    assert_eq!(read.warnings.len(), 1001);
    assert_eq!(read.warnings.last().map(|w| w.message.as_str()), Some("4000 more warnings not listed"));
}
