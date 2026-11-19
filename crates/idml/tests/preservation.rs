#![cfg(feature = "owned")]
#![allow(clippy::unwrap_used, clippy::panic)]

use idml::{ErrorKind, ReadOptions, XmlPart};

mod common;

fn document(xml: &str, options: &ReadOptions) -> Result<idml::Parsed<idml::OwnedPackage>, idml::Error> {
    let files = vec![
        ("mimetype".into(), idml::MIMETYPE.into()),
        ("META-INF/container.xml".into(), common::CONTAINER.into()),
        ("designmap.xml".into(), xml.into()),
    ];
    idml::OwnedPackage::read(&common::zip(&files), options)
}

fn written_document(package: &idml::OwnedPackage) -> String {
    common::unzip(&package.write().unwrap())["designmap.xml"].clone()
}
use idml::model::*;

const NS: &str = "xmlns:idPkg=\"http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging\"";

#[test]
fn unknown_data_and_lexical_forms_survive_known_edits() {
    let xml = format!(
        "<?xml version='1.0'?>\n<!-- keep -->\n<idPkg:Story {NS} DOMVersion='21.5'><Story Self='s' StoryTitle='can&apos;t &#xA; go' Plugin='yes'><PluginData><![CDATA[opaque <&>]]></PluginData><ParagraphStyleRange><CharacterStyleRange><Content xml:space='preserve'>before</Content></CharacterStyleRange></ParagraphStyleRange></Story></idPkg:Story>\n"
    );
    let mut parsed = XmlPart::<StoryFile>::read(&xml, &ReadOptions::lenient()).unwrap();
    assert_eq!(parsed.value.write().unwrap(), xml);
    assert_eq!(parsed.warnings.len(), 3);
    parsed.value.value.story[0].story_title = Some("it's \"edited\"".into());
    let written = parsed.value.write().unwrap();
    assert!(written.contains("StoryTitle=\"it's &quot;edited&quot;\""));
    assert!(written.contains("Plugin='yes'><PluginData><![CDATA[opaque <&>]]></PluginData>"));
    assert!(written.contains("<Content xml:space='preserve'>before</Content>"));
    assert!(written.starts_with("<?xml version='1.0'?>\n<!-- keep -->"));
    let root = roxmltree::Document::parse(&written).unwrap();
    assert_eq!(root.descendants().find(|n| n.attribute("Self") == Some("s")).unwrap().attribute("StoryTitle"), Some("it's \"edited\""));
}

#[test]
fn text_edits_keep_attributes_that_the_collapsed_string_does_not_model() {
    let xml = format!(
        "<idPkg:Story {NS} DOMVersion=\"21.5\"><Story Self=\"s\"><ParagraphStyleRange><CharacterStyleRange><Content xml:space='preserve'>before</Content></CharacterStyleRange></ParagraphStyleRange></Story></idPkg:Story>"
    );
    let mut p = XmlPart::<StoryFile>::read(&xml, &ReadOptions::lenient()).unwrap().value;
    let StoryChild::ParagraphStyleRange(paragraph) = &mut p.value.story[0].children[0] else { panic!() };
    let ParagraphStyleRangeChild::CharacterStyleRange(character) = &mut paragraph.children[0] else { panic!() };
    let CharacterStyleRangeChild::Content(text) = &mut character.children[0] else { panic!() };
    *text = "after & <".into();
    assert_eq!(p.write().unwrap(), xml.replace(">before</Content>", ">after &amp; &lt;</Content>"));
}

#[test]
fn old_versions_and_data_removed_from_the_projection_survive() {
    let xml = format!(
        "<idPkg:Preferences {NS} DOMVersion='7.5'><HTMLExportPreference NumberedListExportOption='StaticOrderedList' CSSExportOption='ExternalCSS'/></idPkg:Preferences>"
    );
    let mut p = XmlPart::<PreferencesFile>::read(&xml, &ReadOptions::default()).unwrap();
    assert!(!p.warnings.is_empty());
    assert_eq!(p.value.write().unwrap(), xml);
    p.value.value.html_export_preference.as_mut().unwrap().numbered_list_export_option = None;
    let written = p.value.write().unwrap();
    assert!(written.contains("CSSExportOption='ExternalCSS'"));
    assert!(written.contains("DOMVersion='7.5'"));
    assert!(!written.contains("NumberedListExportOption"));
}

#[test]
fn optional_unknown_choice_is_retained_and_required_invalid_data_still_fails() {
    // Destination is optional: an unknown future alternative is absent from
    // the projection, but kept in the source document.
    let xml = format!(
        "<Document {NS} DOMVersion='21.5' Self='d'><Hyperlink Self='h' Source='s'><Properties><Destination type='future'><PluginPayload/></Destination></Properties></Hyperlink></Document>"
    );
    let p = document(&xml, &ReadOptions::lenient()).unwrap();
    assert!(p.warnings.iter().any(|w| w.message.contains("typed projection")));
    assert_eq!(written_document(&p.value), xml);
    assert!(document(&xml.replace(" Self='h'", ""), &ReadOptions::lenient()).is_err());
}

#[test]
fn structural_changes_fail_instead_of_discarding_unknown_children() {
    let xml = format!("<idPkg:Story {NS} DOMVersion='21.5'><Story Self='s' Plugin='keep'/></idPkg:Story>");
    let mut p = XmlPart::<StoryFile>::read(&xml, &ReadOptions::lenient()).unwrap().value;
    p.value.story.clear();
    assert_eq!(p.write().unwrap_err().kind(), ErrorKind::Write);
    assert_eq!(p.original_xml(), xml);
}

#[test]
fn structural_replacements_cannot_erase_mixed_text_or_namespace_declarations() {
    for extra in ["xmlns:plugin='urn:plugin'", ""] {
        let content = if extra.is_empty() { "opaque text<Story Self='s'/>" } else { "<Story Self='s'/>" };
        let xml = format!("<idPkg:Story {NS} {extra} DOMVersion='21.5'>{content}</idPkg:Story>");
        let mut p = XmlPart::<StoryFile>::read(&xml, &ReadOptions::lenient()).unwrap().value;
        p.value.story.clear();
        assert_eq!(p.write().unwrap_err().kind(), ErrorKind::Write);
    }
}

#[test]
fn adding_fields_to_an_old_version_requires_conversion() {
    let xml = format!("<idPkg:Story {NS} DOMVersion='8.0'><Story Self='s'/></idPkg:Story>");
    let mut p = XmlPart::<StoryFile>::read(&xml, &ReadOptions::default()).unwrap().value;
    p.value.story[0].story_title = Some("new".into());
    assert_eq!(p.write().unwrap_err().kind(), ErrorKind::Write);
}

#[test]
fn hyperlink_list_is_typed_without_weakening_the_record_or_object_alternatives() {
    let xml = format!(
        "<Document {NS} DOMVersion='21.5' Self='d'><Hyperlink Self='h' Source='s'><Properties><Destination type='list'><ListItem type='string'>f.indd</ListItem><ListItem type='string'>volume</ListItem><ListItem type='long'>1</ListItem><ListItem type='long'>35841</ListItem><ListItem type='long'>3</ListItem></Destination></Properties></Hyperlink></Document>"
    );
    let p = document(&xml, &ReadOptions::default()).unwrap();
    assert!(p.warnings.is_empty());
    assert!(matches!(p.value.document.hyperlink[0].properties.as_ref().unwrap().destination, Some(Destination::List(_))));
    assert_eq!(written_document(&p.value), xml);
    assert!(document(&xml.replace(">35841<", ">bad<"), &ReadOptions::default()).is_err());
}
