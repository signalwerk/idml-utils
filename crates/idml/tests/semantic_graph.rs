#![allow(clippy::unwrap_used, clippy::panic)]
mod common;

use idml::profile::{ProfileResolution, SchemaCatalog};
use idml::semantic::{ReferenceKind, ReferenceResolution};
use idml::{Package, ReadOptions};

const NS: &str = "xmlns:idPkg='http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging'";

fn package(designmap: &str, parts: &[(&str, &str)]) -> Package {
    let mut files = vec![
        ("mimetype".into(), idml::MIMETYPE.into()),
        ("META-INF/container.xml".into(), common::CONTAINER.into()),
        ("designmap.xml".into(), designmap.into()),
    ];
    files.extend(parts.iter().map(|(p, xml)| (p.to_string(), xml.to_string())));
    Package::read(&common::zip(&files), &ReadOptions::source()).unwrap().value
}

#[test]
fn indexes_cross_part_text_links_and_keeps_source_bytes() {
    let designmap = format!(
        "<Document {NS} xmlns:p='urn:plugin' DOMVersion='21.5' Self='d' StoryList='s1  missing'>\n<idPkg:Story src='Stories/s.xml'/><idPkg:Spread src='Spreads/p.xml'/><p:TextFrame Self='plugin' ParentStory='untracked'/><p:Story Self='s1'/></Document>"
    );
    let story = format!("<idPkg:Story {NS} DOMVersion='21.5'><Story Self='s1'/></idPkg:Story>");
    let spread = format!(
        "<idPkg:Spread {NS} DOMVersion='21.5'><Spread Self='spread'><TextFrame Self='f1' ParentStory='s1' PreviousTextFrame='n' NextTextFrame='f2'/><TextPath Self='f2' ParentStory='s1' PreviousTextFrame='f1' NextTextFrame='n'/></Spread></idPkg:Spread>"
    );
    let p = package(&designmap, &[("Stories/s.xml", &story), ("Spreads/p.xml", &spread)]);
    let before = p.write().unwrap();
    let graph = p.semantic_graph();
    assert_eq!(p.write().unwrap(), before);
    assert!(graph.duplicates().is_empty());
    assert_eq!(graph.objects_named("s1").count(), 1);
    assert_eq!(graph.objects_named("plugin").count(), 0);
    assert_eq!(graph.references().len(), 8);
    let list: Vec<_> = graph.references().iter().filter(|r| r.kind == ReferenceKind::StoryList).collect();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].raw_attribute.as_ref(), "s1  missing");
    assert_eq!(list[1].token_index, 1);
    assert_eq!(list[1].resolution, ReferenceResolution::Unresolved);
    let ReferenceResolution::Resolved(story_index) = list[0].resolution else { panic!("story not resolved") };
    assert_eq!(graph.object(story_index).unwrap().source.part, "Stories/s.xml");
    assert_eq!(graph.references().iter().filter(|r| r.resolution == ReferenceResolution::Unlinked).count(), 2);
    let f1 = graph.objects_named("f1").next().unwrap();
    assert_eq!(f1.source.location(&p).unwrap().part, "Spreads/p.xml");
    assert!(f1.source.location(&p).unwrap().element.contains("TextFrame"));
}

#[test]
fn ambiguity_unexpected_targets_and_cycles_remain_distinct_observations() {
    let designmap = format!("<Document {NS} DOMVersion='21.5' Self='d'><idPkg:Spread src='Spreads/p.xml'/></Document>");
    let spread = format!(
        "<idPkg:Spread {NS} DOMVersion='21.5'><Spread Self='s'><TextFrame Self='f1' ParentStory='duplicate' PreviousTextFrame='f2' NextTextFrame='f2'/><TextFrame Self='f2' ParentStory='odd' PreviousTextFrame='f1' NextTextFrame='f1'/><Story Self='duplicate'/><Story Self='duplicate'/><PluginObject Self='odd'/></Spread></idPkg:Spread>"
    );
    let p = package(&designmap, &[("Spreads/p.xml", &spread)]);
    let graph = p.semantic_graph();
    assert_eq!(graph.duplicates().len(), 1);
    assert_eq!(graph.duplicates()[0].self_id, "duplicate");
    assert_eq!(graph.duplicates()[0].objects.len(), 2);
    assert!(graph.references().iter().any(|r| r.target == "duplicate" && matches!(&r.resolution, ReferenceResolution::Ambiguous(v) if v.len() == 2)));
    assert!(graph.references().iter().any(|r| r.target == "odd" && matches!(r.resolution, ReferenceResolution::UnexpectedTarget(_))));
    assert_eq!(graph.references().iter().filter(|r| matches!(r.resolution, ReferenceResolution::Resolved(_))).count(), 4);
}

#[test]
fn source_handles_are_stale_after_edits_and_schema_validity_is_separate() {
    let designmap = format!("<Document {NS} DOMVersion='21.5' Self='d' StoryList='missing'/>");
    let mut p = package(&designmap, &[]);
    let graph = p.semantic_graph();
    assert_eq!(graph.references()[0].resolution, ReferenceResolution::Unresolved);
    let catalog = SchemaCatalog::builtin().unwrap();
    let ProfileResolution::Exact(profile) = catalog.resolve("21.5".parse().unwrap(), true) else { panic!("missing profile") };
    assert!(p.validate(catalog, profile).is_supported());
    assert!(graph.references()[0].source.location(&p).is_some());
    let root = p.document().root();
    p.document_mut().unwrap().set_attribute(root, "Self", "changed").unwrap();
    assert!(graph.references()[0].source.location(&p).is_none());
    assert_eq!(p.semantic_graph().objects_named("changed").count(), 1);
}

#[test]
fn story_list_can_include_the_backing_xml_story() {
    let designmap =
        format!("<Document {NS} DOMVersion='21.5' Self='d' StoryList='backing'><idPkg:BackingStory src='XML/BackingStory.xml'/></Document>");
    let backing = format!("<idPkg:BackingStory {NS} DOMVersion='21.5'><XmlStory Self='backing'/></idPkg:BackingStory>");
    let p = package(&designmap, &[("XML/BackingStory.xml", &backing)]);
    let graph = p.semantic_graph();
    assert!(matches!(graph.references()[0].resolution, ReferenceResolution::Resolved(_)));
}

fn kinds(graph: &idml::semantic::SemanticGraph, kind: ReferenceKind) -> Vec<(String, ReferenceResolution)> {
    graph.references().iter().filter(|r| r.kind == kind).map(|r| (r.target.clone(), r.resolution.clone())).collect()
}

#[test]
fn catalog_relationships_use_scoped_owners_expected_targets_and_observed_nil_markers() {
    let designmap = format!(
        "<Document {NS} xmlns:p='urn:plugin' DOMVersion='21.5' Self='d'><idPkg:Graphic src='Resources/Graphic.xml'/><idPkg:Styles src='Resources/Styles.xml'/><idPkg:MasterSpread src='MasterSpreads/m.xml'/><idPkg:Spread src='Spreads/p.xml'/><idPkg:Story src='Stories/s.xml'/><Layer Self='layer'/><Hyperlink Self='h' Source='hts'/><Hyperlink Self='h2' Source='n'/></Document>"
    );
    let graphic = format!("<idPkg:Graphic {NS} DOMVersion='21.5'><Color Self='Color/Black'/><Swatch Self='Swatch/None'/></idPkg:Graphic>");
    let styles = format!(
        "<idPkg:Styles {NS} DOMVersion='21.5'><RootCharacterStyleGroup><CharacterStyle Self='CharacterStyle/c'/></RootCharacterStyleGroup><RootObjectStyleGroup><ObjectStyle Self='ObjectStyle/o'><TransparencySetting><InnerGlowSetting Source='EdgeSourced'/></TransparencySetting></ObjectStyle></RootObjectStyleGroup></idPkg:Styles>"
    );
    let master = format!("<idPkg:MasterSpread {NS} DOMVersion='21.5'><MasterSpread Self='master'/></idPkg:MasterSpread>");
    let spread = format!(
        "<idPkg:Spread {NS} xmlns:p='urn:plugin' DOMVersion='21.5'><Spread Self='s'><Page Self='page1' AppliedMaster='master'/><Page Self='page2' AppliedMaster='n'/><Rectangle Self='r' ItemLayer='n' FillColor='Color/Black' StrokeColor='Swatch/None' AppliedObjectStyle='n' p:AppliedObjectStyle='missing'/><p:Rectangle Self='plugin' ItemLayer='missing'/><Oval Self='o' ItemLayer='layer' AppliedObjectStyle='ObjectStyle/o' FillColor='layer'/></Spread></idPkg:Spread>"
    );
    let story = format!(
        "<idPkg:Story {NS} DOMVersion='21.5'><Story Self='st'><ParagraphStyleRange AppliedParagraphStyle='CharacterStyle/c'><CharacterStyleRange AppliedCharacterStyle='n'><HyperlinkTextSource Self='hts' AppliedCharacterStyle='CharacterStyle/c'/></CharacterStyleRange></ParagraphStyleRange></Story></idPkg:Story>"
    );
    let p = package(
        &designmap,
        &[
            ("Resources/Graphic.xml", &graphic),
            ("Resources/Styles.xml", &styles),
            ("MasterSpreads/m.xml", &master),
            ("Spreads/p.xml", &spread),
            ("Stories/s.xml", &story),
        ],
    );
    let before = p.write().unwrap();
    let graph = p.semantic_graph();
    assert_eq!(p.write().unwrap(), before);
    let resolved = |r: &ReferenceResolution| matches!(r, ReferenceResolution::Resolved(_));

    let masters = kinds(&graph, ReferenceKind::AppliedMaster);
    assert!(resolved(&masters[0].1));
    assert_eq!(masters[1].1, ReferenceResolution::Unlinked);
    // `n` is a nil marker only where native exports used it for that relationship.
    let layers = kinds(&graph, ReferenceKind::ItemLayer);
    assert_eq!(layers[0], ("n".into(), ReferenceResolution::Unresolved));
    assert!(resolved(&layers[1].1));
    assert_eq!(kinds(&graph, ReferenceKind::HyperlinkSource)[1].1, ReferenceResolution::Unresolved);
    assert!(resolved(&kinds(&graph, ReferenceKind::HyperlinkSource)[0].1));
    assert!(kinds(&graph, ReferenceKind::StrokeColor).iter().all(|(_, r)| resolved(r)));
    let fills = kinds(&graph, ReferenceKind::FillColor);
    assert!(resolved(&fills[0].1));
    assert!(matches!(fills[1].1, ReferenceResolution::UnexpectedTarget(i) if graph.object(i).unwrap().element == "Layer"));
    assert_eq!(kinds(&graph, ReferenceKind::AppliedObjectStyle).len(), 2);
    assert!(matches!(kinds(&graph, ReferenceKind::AppliedParagraphStyle)[0].1, ReferenceResolution::UnexpectedTarget(_)));
    let characters = kinds(&graph, ReferenceKind::AppliedCharacterStyle);
    assert_eq!(characters[0].1, ReferenceResolution::Unlinked);
    assert!(resolved(&characters[1].1));
    // Namespaced attributes/elements and same-named attributes on other owners are not interpreted.
    assert!(graph.references().iter().all(|r| r.target != "missing" && r.target != "EdgeSourced"));
    assert_eq!(graph.objects_named("plugin").count(), 0);
    let rectangle = graph.objects().iter().position(|o| o.self_id == "r").unwrap();
    assert_eq!(graph.references_from(rectangle).count(), 4);
    for kind in ReferenceKind::ALL {
        assert!(!kind.expected_targets().is_empty(), "{kind:?}");
        assert!(!kind.attribute_name().is_empty());
    }
}

#[test]
fn external_links_are_recorded_as_stored_and_never_resolved() {
    let designmap = format!("<Document {NS} DOMVersion='21.5' Self='d'><idPkg:Spread src='Spreads/p.xml'/></Document>");
    let spread = format!(
        "<idPkg:Spread {NS} DOMVersion='21.5'><Spread Self='s'><Rectangle Self='r'><Image Self='i'><Link Self='l1' LinkResourceURI='file:/Volumes/Missing/a%20b.tif' StoredState='Normal'/></Image></Rectangle><Rectangle Self='r2'><Image Self='i2'><Link Self='l2' LinkResourceURI='relative.png' StoredState='Embedded'/></Image></Rectangle><Rectangle Self='r3'><EPS Self='e'><Link Self='l3' LinkResourceURI='cclibraries:asset'/></EPS></Rectangle></Spread></idPkg:Spread>"
    );
    let p = package(&designmap, &[("Spreads/p.xml", &spread)]);
    let before = p.write().unwrap();
    let graph = p.semantic_graph();
    assert_eq!(p.write().unwrap(), before);
    let links = graph.external_links();
    assert_eq!(links.len(), 3);
    assert_eq!(links[0].uri, "file:/Volumes/Missing/a%20b.tif");
    assert_eq!(links[0].scheme(), Some("file"));
    assert_eq!(links[0].stored_state.as_deref(), Some("Normal"));
    assert_eq!(links[1].scheme(), None);
    assert_eq!(links[1].stored_state.as_deref(), Some("Embedded"));
    assert_eq!(links[2].stored_state, None);
    assert!(links[2].source.location(&p).unwrap().element.ends_with("EPS/Link"));
    // An unavailable external file is not an unresolved package reference.
    assert!(graph.references().is_empty());
    assert_eq!(graph.objects_named("l1").count(), 1);
}

fn threads_by_first(graph: &idml::semantic::SemanticGraph) -> std::collections::BTreeMap<String, idml::semantic::TextThread> {
    graph.text_threads().into_iter().map(|t| (graph.object(t.frames[0]).unwrap().self_id.clone(), t)).collect()
}

fn ids(graph: &idml::semantic::SemanticGraph, frames: &[usize]) -> Vec<String> {
    frames.iter().map(|i| graph.object(*i).unwrap().self_id.clone()).collect()
}

#[test]
fn text_threads_report_only_inconsistent_link_patterns() {
    use idml::semantic::ThreadIssue;
    let frame = |id: &str, story: &str, prev: &str, next: &str| {
        format!("<TextFrame Self='{id}' ParentStory='{story}' PreviousTextFrame='{prev}' NextTextFrame='{next}'/>")
    };
    let designmap = format!(
        "<Document {NS} DOMVersion='21.5' Self='d' StoryList='s1 s2 s3 s4 s5 s6 s7 s8'><idPkg:Story src='Stories/s.xml'/><idPkg:Spread src='Spreads/a.xml'/><idPkg:Spread src='Spreads/b.xml'/></Document>"
    );
    let stories: String = (1..=8).map(|i| format!("<Story Self='s{i}'/>")).collect();
    let story = format!("<idPkg:Story {NS} DOMVersion='21.5'>{stories}</idPkg:Story>");
    // A consistent thread across parts, written in reverse tree order, with a TextPath.
    let a = format!("<idPkg:Spread {NS} DOMVersion='21.5'><Spread Self='sa'>{}</Spread></idPkg:Spread>", frame("t1", "s1", "n", "t2"));
    let b = format!(
        "<idPkg:Spread {NS} DOMVersion='21.5'><Spread Self='sb'><GraphicLine Self='g'><TextPath Self='t3' ParentStory='s1' PreviousTextFrame='t2' NextTextFrame='n'/></GraphicLine>{}{}{}{}{}{}{}{}{}{}{}{}{}{}</Spread></idPkg:Spread>",
        frame("t2", "s1", "t1", "t3"),
        frame("c1", "s2", "c2", "c2"),
        frame("c2", "s2", "c1", "c1"),
        frame("j1", "s3", "n", "j3"),
        frame("j2", "s3", "n", "j3"),
        frame("j3", "s3", "j1", "n"),
        frame("m1", "s4", "n", "m2"),
        frame("m2", "s5", "m1", "n"),
        frame("a1", "s6", "n", "n"),
        frame("a2", "s6", "n", "n"),
        frame("u1", "s7", "missing", "n"),
        frame("r1", "s8", "n", "n"),
        frame("r2", "s8", "r1", "n"),
        "<TextFrame Self='bare'/>",
    );
    let p = package(&designmap, &[("Stories/s.xml", &story), ("Spreads/a.xml", &a), ("Spreads/b.xml", &b)]);
    let before = p.write().unwrap();
    let graph = p.semantic_graph();
    let threads = threads_by_first(&graph);
    assert_eq!(p.write().unwrap(), before);
    let story_id = |t: &idml::semantic::TextThread| t.story.map(|s| graph.object(s).unwrap().self_id.clone());
    let index = |id: &str| graph.objects().iter().position(|o| o.self_id == id).unwrap();

    // Reciprocal pairs are legitimate reference cycles, not thread issues.
    assert_eq!(ids(&graph, &threads["t1"].frames), ["t1", "t2", "t3"]);
    assert!(threads["t1"].issues.is_empty(), "{:?}", threads["t1"].issues);
    assert_eq!(story_id(&threads["t1"]).as_deref(), Some("s1"));

    assert_eq!(ids(&graph, &threads["c1"].frames), ["c1", "c2"]);
    assert_eq!(threads["c1"].issues, [ThreadIssue::Cycle { frame: index("c1") }]);
    assert_eq!(story_id(&threads["c1"]).as_deref(), Some("s2"));

    assert_eq!(ids(&graph, &threads["j1"].frames), ["j1", "j3"]);
    assert_eq!(ids(&graph, &threads["j2"].frames), ["j2"]);
    assert!(threads["j2"].issues.contains(&ThreadIssue::JoinsAnotherThread { frame: index("j3") }));
    assert!(threads["j2"].issues.contains(&ThreadIssue::NotReciprocal { frame: index("j2"), kind: ReferenceKind::NextTextFrame }));
    assert!(threads["j1"].issues.contains(&ThreadIssue::StoryInSeveralThreads));

    assert_eq!(threads["m1"].issues, [ThreadIssue::ParentStoryMismatch { frame: index("m2") }]);
    assert_eq!(threads["m1"].story, None);

    assert_eq!(threads["a1"].issues, [ThreadIssue::StoryInSeveralThreads]);
    assert_eq!(threads["a2"].issues, [ThreadIssue::StoryInSeveralThreads]);
    assert_eq!(threads["u1"].issues, [ThreadIssue::UnfollowedLink { frame: index("u1"), kind: ReferenceKind::PreviousTextFrame }]);
    assert!(threads["r2"].issues.contains(&ThreadIssue::NotReciprocal { frame: index("r2"), kind: ReferenceKind::PreviousTextFrame }));
    // Absent links end a thread silently; every container appears exactly once.
    assert!(threads["bare"].issues.is_empty());
    assert_eq!(threads["bare"].story, None);
    let mut all: Vec<_> = threads.values().flat_map(|t| t.frames.clone()).collect();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), graph.objects().iter().filter(|o| o.element.contains("Text")).count());
}

#[test]
fn grammar_valid_frames_can_still_form_a_thread_cycle() {
    use idml::compose::{Paragraph, StoryBuilder, TextFrameBuilder};
    use idml::geometry::{Rect, Transform};
    use idml::semantic::ThreadIssue;
    let catalog = SchemaCatalog::builtin().unwrap();
    let mut author = idml::Authoring::for_version(catalog, "21.5".parse().unwrap(), "doc").unwrap();
    let plan = author
        .plan_edit(|package| {
            package.add_story("Stories/s.xml", StoryBuilder::new("story")?.paragraph(Paragraph::text("body")), &ReadOptions::source())?;
            let mut spread = idml::Element::new("Spread");
            spread.set_attr("Self", "spread");
            let mut part = idml::Element::new("idPkg:Spread");
            part.children.push(spread);
            package.add_part("Spreads/s.xml", &part, &ReadOptions::source())?;
            for (id, other) in [("f1", "f2"), ("f2", "f1")] {
                let frame = TextFrameBuilder::rectangle(id, "story", Rect::new(0.0, 0.0, 100.0, 50.0)?, Transform::IDENTITY)?.format(|a| {
                    a.set_previous_text_frame(&other.to_owned())?;
                    a.set_next_text_frame(&other.to_owned())
                })?;
                package.add_text_frame("Spreads/s.xml", "spread", frame, &ReadOptions::source())?;
            }
            Ok(())
        })
        .unwrap();
    assert!(plan.can_apply(), "{:?}", plan.validation());
    plan.apply(&mut author).unwrap();
    let graph = author.package().semantic_graph();
    assert!(graph.references().iter().all(|r| matches!(r.resolution, ReferenceResolution::Resolved(_))));
    let threads = graph.text_threads();
    assert_eq!(threads.len(), 1);
    assert!(matches!(threads[0].issues.as_slice(), [ThreadIssue::Cycle { .. }]));
}
