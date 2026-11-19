#![allow(clippy::unwrap_used, clippy::panic)]
mod common;
use idml::profile::{ProfileResolution, SchemaCatalog, SchemaForm, Support};
use idml::{Element, Field, LossPolicy, Package, ReadOptions, XmlTree};
fn profile(version: &str, effective: bool) -> &'static idml::profile::CompatibilityProfile {
    let c = SchemaCatalog::builtin().unwrap();
    match c.resolve(version.parse().unwrap(), effective) {
        ProfileResolution::Exact(p) => p,
        _ => panic!("missing exact profile"),
    }
}
fn package(designmap: &str, parts: &[(&str, &str)]) -> Package {
    let mut files = vec![
        ("mimetype".into(), idml::MIMETYPE.into()),
        ("META-INF/container.xml".into(), common::CONTAINER.into()),
        ("designmap.xml".into(), designmap.into()),
    ];
    files.extend(parts.iter().map(|(p, x)| (p.to_string(), x.to_string())));
    Package::read(&common::zip(&files), &ReadOptions::source()).unwrap().value
}
const NS: &str = "xmlns:idPkg='http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging'";
#[test]
fn node_identity_and_namespace_context_survive_structural_edits() {
    let xml = "<?xml version='1.0'?><r xmlns:p='urn:one'><a><p:item Self='s' custom='&apos;' /></a><b xmlns:p='urn:two'><p:other /></b></r>";
    let mut t = XmlTree::parse(xml, &ReadOptions::source()).unwrap();
    let item = t.descendants(t.root()).find(|n| t.attribute(*n, "Self") == Some("s")).unwrap();
    let b = t.elements(t.root()).nth(1).unwrap();
    t.move_node(item, b, 0).unwrap();
    assert_eq!(t.attribute(item, "custom"), Some("'"));
    assert_eq!(t.name(item).unwrap().namespace, "urn:one");
    let out = t.to_xml().unwrap();
    assert!(out.contains("custom='&apos;'"));
    assert!(out.contains("xmlns:p=\"urn:one\""));
    let parsed = XmlTree::parse(&out, &ReadOptions::source()).unwrap();
    let n = parsed.descendants(parsed.root()).find(|n| parsed.attribute(*n, "Self") == Some("s")).unwrap();
    assert_eq!(parsed.name(n).unwrap().namespace, "urn:one");
    let before = t.to_xml().unwrap();
    assert!(t.move_node(item, b, 999).is_err());
    assert_eq!(t.to_xml().unwrap(), before);
    assert!(t.declare_namespace(b, "", "urn:bad").is_err());
    t.remove_node(item).unwrap();
    assert!(t.kind(item).is_err());
}
#[test]
fn unknown_values_have_a_distinct_state_and_views_edit_the_source() {
    let xml = format!(
        "<idPkg:Story {NS} DOMVersion='21.5'><Story Self='s' TrackChanges='future' Plugin='keep'><PluginData/><ParagraphStyleRange><CharacterStyleRange><Content xml:space='preserve'>before</Content></CharacterStyleRange></ParagraphStyleRange></Story></idPkg:Story>"
    );
    let mut t = XmlTree::parse(&xml, &ReadOptions::source()).unwrap();
    let id = t.views::<idml::views::Story>().next().unwrap().node_id();
    assert!(matches!(t.view::<idml::views::Story>(id).unwrap().track_changes(), Field::Unrecognized { .. }));
    t.view_mut::<idml::views::Story>(id).unwrap().set_track_changes(&true).unwrap();
    let c = t.views::<idml::views::Content>().next().unwrap().node_id();
    t.view_mut::<idml::views::Content>(c).unwrap().set_text("after & <").unwrap();
    let out = t.to_xml().unwrap();
    assert!(out.contains("Plugin='keep'><PluginData/>"));
    assert!(out.contains("<Content xml:space='preserve'>after &amp; &lt;</Content>"));
    assert!(out.contains("TrackChanges=\"true\""));
    assert!(t.set_attribute(t.root(), "DOMVersion", "8.0").is_err());
}
#[test]
fn profiles_do_not_infer_support_in_missing_releases() {
    let c = SchemaCatalog::builtin().unwrap();
    assert!(matches!(c.resolve("12.0".parse().unwrap(), true), ProfileResolution::Unknown { .. }));
    assert!(c.profiles().iter().any(|p| p.snapshot.form == SchemaForm::Consolidated));
    let xml = "<Document DOMVersion='21.5' Self='d'><PluginObject value='raw'/></Document>";
    let t = XmlTree::parse(xml, &ReadOptions::source()).unwrap();
    let r = c.validate(profile("21.5", true), &t);
    assert_eq!(r.support(), Support::Unknown);
    assert_eq!(t.to_xml().unwrap(), xml);
    let mut forged = profile("21.5", true).clone();
    forged.snapshot.dom_version = "8.0".parse().unwrap();
    assert_eq!(c.validate(&forged, &t).support(), Support::Unknown);
}
#[test]
fn exact_raw_and_effective_profiles_differ_at_the_documented_gap() {
    let xml = format!(
        "<Document {NS} DOMVersion='21.5' Self='d'><Hyperlink Self='h' Source='s'><Properties><Destination type='list'><ListItem type='string'>a</ListItem><ListItem type='string'>b</ListItem><ListItem type='long'>1</ListItem><ListItem type='long'>2</ListItem><ListItem type='long'>3</ListItem></Destination></Properties></Hyperlink></Document>"
    );
    let t = XmlTree::parse(&xml, &ReadOptions::source()).unwrap();
    let c = SchemaCatalog::builtin().unwrap();
    assert!(c.validate(profile("21.5", true), &t).is_supported(), "{:?}", c.validate(profile("21.5", true), &t));
    assert!(!c.validate(profile("21.5", false), &t).is_supported());
}
#[test]
fn conversion_is_transactional_validated_and_forbids_loss_by_default() {
    let xml = format!("<Document {NS} DOMVersion='8.0' Self='d'><PluginObject secret='keep'/></Document>");
    let mut p = package(&xml, &[]);
    let before = p.write().unwrap();
    let c = SchemaCatalog::builtin().unwrap();
    let plan = p.plan_conversion(c, profile("21.5", true), LossPolicy::Forbid).unwrap();
    assert!(!plan.can_apply());
    assert_eq!(p.write().unwrap(), before);
    assert!(plan.apply(&mut p).is_err());
    assert_eq!(p.write().unwrap(), before);
    let plan = p.plan_conversion(c, profile("21.5", true), LossPolicy::Allow).unwrap();
    assert!(plan.can_apply(), "{:?}", plan.validation());
    assert!(plan.changes().iter().any(|c| c.lossy && c.original.contains("secret='keep'")));
    plan.apply(&mut p).unwrap();
    assert!(p.validate(c, profile("21.5", true)).is_supported());
    assert_eq!(p.document().dom_version(), Some("21.5"));
    let plan = p.plan_conversion(c, profile("21.5", true), LossPolicy::Forbid).unwrap();
    let root = p.document().root();
    p.document_mut().unwrap().set_attribute(root, "Self", "changed").unwrap();
    assert!(plan.apply(&mut p).is_err());
    assert_eq!(p.document().attribute(root, "Self"), Some("changed"));
}
#[test]
fn conversion_preserves_stable_nodes_during_reviewed_wraps() {
    let designmap = format!("<Document {NS} DOMVersion='6.0' Self='d'><idPkg:Story src='Stories/s.xml'/></Document>");
    let story = format!("<idPkg:Story {NS} DOMVersion='6.0'><Story Self='s'><TextFrame Self='t'/></Story></idPkg:Story>");
    let mut p = package(&designmap, &[("Stories/s.xml", &story)]);
    let id = p
        .part("Stories/s.xml")
        .unwrap()
        .descendants(p.part("Stories/s.xml").unwrap().root())
        .find(|n| p.part("Stories/s.xml").unwrap().attribute(*n, "Self") == Some("t"))
        .unwrap();
    let c = SchemaCatalog::builtin().unwrap();
    let plan = p.plan_conversion(c, profile("21.5", true), LossPolicy::Forbid).unwrap();
    assert!(plan.can_apply(), "{:?}", plan.validation());
    assert!(plan.changes().iter().any(|c| c.kind == idml::package::ChangeKind::Wrap));
    plan.apply(&mut p).unwrap();
    assert_eq!(p.part("Stories/s.xml").unwrap().attribute(id, "Self"), Some("t"));
}
#[test]
fn iterative_source_path_handles_deep_groups_on_a_small_stack() {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| {
            let xml = format!("<r>{}<leaf value='keep'/>{}</r>", "<g>".repeat(800), "</g>".repeat(800));
            let mut t = XmlTree::parse(&xml, &ReadOptions::source()).unwrap();
            let id = t.descendants(t.root()).find(|n| t.attribute(*n, "value").is_some()).unwrap();
            t.set_attribute(id, "value", "edited").unwrap();
            let copy = t.clone();
            assert!(copy.to_xml().unwrap().contains("value=\"edited\""));
            drop(copy);
            drop(t);
        })
        .unwrap()
        .join()
        .unwrap();
}
#[test]
fn direct_element_builder_preserves_unknown_data_without_a_projection() {
    let mut doc = Element::new("Document");
    doc.set_attr("Self", "d");
    let mut unknown = Element::new("PluginObject");
    unknown.set_attr("Secret", "x");
    doc.children.push(unknown);
    let p = Package::from_elements(&doc, &Default::default(), "21.5".parse().unwrap(), &ReadOptions::source()).unwrap();
    assert!(p.document().to_xml().unwrap().contains("PluginObject Secret=\"x\""));
    let mut b = idml::view::Builder::<idml::views::Story>::new();
    assert!(b.set_attribute("TrackChanges", "invalid").is_err());
    b.set_track_changes(&true).unwrap();
    assert_eq!(b.build().attr("TrackChanges"), Some("true"));
}
#[test]
fn streaming_parser_validates_structure_namespaces_and_xml_characters() {
    for xml in [
        "<r><a></r></a>",
        "<r/><x/>",
        "text<r/>",
        "<r x='1' x='2'/>",
        "<r xmlns:a='u' xmlns:b='u' a:x='1' b:x='2'/>",
        "<p:r/>",
        "<!DOCTYPE r><r/>",
        "<r a='<'/>",
        "<r>]]></r>",
        "<r>&unknown;</r>",
        "<r>&#0;</r>",
        "<r/><?XML bad?>",
        " <r><?xml version='1.0'?></r>",
    ] {
        assert!(XmlTree::parse(xml, &ReadOptions::source()).is_err(), "accepted {xml}");
    }
    let xml = "<r xmlns:p='urn:a&amp;b' a='x&#xA;y\nz'><p:c xml:space='preserve'><![CDATA[a\rb]]>&amp;&#xD;</p:c></r>";
    let tree = XmlTree::parse(xml, &ReadOptions::source()).unwrap();
    assert_eq!(tree.attribute(tree.root(), "a"), Some("x\ny z"));
    let c = tree.elements(tree.root()).next().unwrap();
    assert_eq!(tree.name(c).unwrap().namespace, "urn:a&b");
    assert_eq!(tree.attribute(c, "xml:space"), Some("preserve"));
    assert_eq!(tree.text(c).unwrap(), "a\nb&\r");
    assert_eq!(tree.to_xml().unwrap(), xml);
}
#[test]
fn custom_profile_metadata_is_explicit_and_affects_identity() {
    let root = std::env::temp_dir().join(format!("idml-custom-profile-{}", std::process::id()));
    let dir = root.join("14.0-test/package");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Story.rnc"),"# Adobe InDesign Test 14.0.0.1\nnamespace idPkg=\"http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging\"\ngrammar {\nstart=Story_File\nStory_File=element idPkg:Story {attribute DOMVersion {string \"14.0\"}, Test_Object*}\nTest_Object=element Story {attribute Self {xsd:string}, attribute TrackChanges {xsd:boolean}?}\n}\n").unwrap();
    let sidecar = dir.parent().unwrap().join("profile.tsv");
    std::fs::write(&sidecar, "locale\tde_CH\nplugin\tWordsFlow\t1.2\nplugins_complete\ttrue\n").unwrap();
    let catalog = SchemaCatalog::from_compressed(&idml_codegen::profiles::bytes(&root).unwrap()).unwrap();
    let p = match catalog.resolve("14.0".parse().unwrap(), true) {
        ProfileResolution::Exact(p) => p,
        _ => panic!(),
    };
    assert_eq!(p.snapshot.locale.as_deref(), Some("de_CH"));
    assert!(p.snapshot.plugins_complete);
    assert_eq!(p.snapshot.plugins.as_ref().unwrap()[0].name, "WordsFlow");
    let xml = format!("<idPkg:Story {NS} DOMVersion='14.0'><Story Self='s' TrackChanges='false'/></idPkg:Story>");
    let mut t = XmlTree::parse(&xml, &ReadOptions::source()).unwrap();
    let id = t.views::<idml::views::Story>().next().unwrap().node_id();
    assert!(t.view_mut::<idml::views::Story>(id).unwrap().set_track_changes(&true).is_ok());
    t.view_mut::<idml::views::Story>(id).unwrap().set_checked(&catalog, p, "TrackChanges", &true).unwrap();
    assert!(catalog.validate(p, &t).is_supported());
    let old = p.fingerprint.clone();
    std::fs::write(sidecar, "locale\tde_CH\nplugin\tWordsFlow\t1.3\nplugins_complete\ttrue\n").unwrap();
    let changed = SchemaCatalog::from_compressed(&idml_codegen::profiles::bytes(&root).unwrap()).unwrap();
    assert!(changed.profiles().iter().all(|p| p.fingerprint != old));
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn editing_one_attribute_keeps_lexical_entity_and_cdata_nodes() {
    let xml = "<?xml version='1.0'?><r a='old'>one&amp;two<![CDATA[three]]>&#xD;<!--keep--><?unknown value?></r>";
    let mut t = XmlTree::parse(xml, &ReadOptions::source()).unwrap();
    t.set_attribute(t.root(), "a", "new").unwrap();
    assert_eq!(t.to_xml().unwrap(), xml.replace("a='old'", "a=\"new\""));
}
