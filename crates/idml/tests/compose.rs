#![allow(clippy::unwrap_used, clippy::panic)]
use idml::{
    Authoring, Package, ReadOptions,
    compose::{Paragraph, StoryBuilder, TextRun},
    profile::{ProfileResolution, SchemaCatalog},
};

#[test]
fn story_producers_validate_at_exact_historical_targets_and_roundtrip_content() {
    let catalog = SchemaCatalog::builtin().unwrap();
    for version in ["7.0", "7.5", "8.0", "10.0", "11.0", "15.0", "21.5"] {
        let profile = match catalog.resolve(version.parse().unwrap(), true) {
            ProfileResolution::Exact(p) => p,
            other => panic!("{other:?}"),
        };
        let mut author = Authoring::new(catalog, profile, "document").unwrap();
        let plan = author
            .plan_edit(|package| {
                let body = TextRun::new("Hello\t<&>\u{7} 日本語").format(|a| a.set_point_size(&18.0))?.with_break();
                let story = StoryBuilder::new("story1")?.paragraph(Paragraph::new().run(body));
                package.add_story("Stories/body.xml", story, &ReadOptions::source())?;
                Ok(())
            })
            .unwrap();
        assert!(plan.can_apply(), "{version}: {:?}", plan.validation());
        plan.apply(&mut author).unwrap();
        let bytes = author.write().unwrap();
        let read = Package::read(&bytes, &ReadOptions::source()).unwrap().value;
        assert_eq!(read.part("Stories/body.xml").unwrap().dom_version(), Some(version));
        let tree = read.part("Stories/body.xml").unwrap();
        let content = tree.views::<idml::views::Content>().next().unwrap();
        assert_eq!(tree.text(content.node_id()).unwrap(), "Hello\t<&>\u{7} 日本語");
        let graph = read.semantic_graph();
        assert!(graph.duplicates().is_empty());
        assert_eq!(graph.references().len(), 1);
    }
}
#[test]
fn duplicate_story_or_path_leaves_the_source_untouched_and_never_retargets() {
    let mut package = Package::new("99.3".parse().unwrap(), "document").unwrap();
    package.add_story("Stories/body.xml", StoryBuilder::new("story").unwrap(), &ReadOptions::source()).unwrap();
    let before = package.write().unwrap();
    assert!(package.add_story("Stories/other.xml", StoryBuilder::new("story").unwrap(), &ReadOptions::source()).is_err());
    assert!(package.add_story("Stories/body.xml", StoryBuilder::new("other").unwrap(), &ReadOptions::source()).is_err());
    for path in ["../escape.xml", "/absolute.xml", "Stories/body.bin", "Stories//body.xml"] {
        assert!(package.add_story(path, StoryBuilder::new("other").unwrap(), &ReadOptions::source()).is_err());
    }
    assert_eq!(before, package.write().unwrap());
    assert_eq!(package.document().dom_version(), Some("99.3"));
    assert!(StoryBuilder::new("two words").is_err());
}
#[test]
fn breaks_and_whitespace_are_only_added_when_requested() {
    let part = StoryBuilder::new("story").unwrap().paragraph(Paragraph::text("\r\n x ")).build().unwrap();
    let xml = part.to_xml().unwrap();
    assert!(!xml.contains("<Br"));
    assert!(xml.contains("&#xd;\n x "));
    let tree = idml::XmlTree::from_element(&part).unwrap();
    let content = tree.views::<idml::views::Content>().next().unwrap();
    assert_eq!(tree.text(content.node_id()).unwrap(), "\r\n x ");
}

#[test]
fn explicit_rectangle_frames_validate_and_keep_story_and_spread_identities() {
    use idml::{
        Element,
        compose::TextFrameBuilder,
        geometry::{Rect, Transform},
    };
    let catalog = SchemaCatalog::builtin().unwrap();
    for version in ["7.0", "8.0", "10.0", "15.0", "21.5"] {
        let mut author = Authoring::for_version(catalog, version.parse().unwrap(), "doc").unwrap();
        let plan = author
            .plan_edit(|package| {
                package.add_story("Stories/s.xml", StoryBuilder::new("story")?.paragraph(Paragraph::text("body")), &ReadOptions::source())?;
                let mut spread = Element::new("Spread");
                spread.set_attr("Self", "spread");
                let mut part = Element::new("idPkg:Spread");
                part.children.push(spread);
                package.add_part("Spreads/s.xml", &part, &ReadOptions::source())?;
                let frame = TextFrameBuilder::rectangle("frame", "story", Rect::new(0.0, 0.0, 200.0, 100.0)?, Transform::translation(36.0, 72.0)?)?;
                package.add_text_frame("Spreads/s.xml", "spread", frame, &ReadOptions::source())?;
                Ok(())
            })
            .unwrap();
        assert!(plan.can_apply(), "{version}: {:?}", plan.validation());
        plan.apply(&mut author).unwrap();
        let package = author.package();
        let view = package.source_document().text_containers().next().unwrap();
        assert_eq!(view.parent_story(), idml::Field::Known("story".into()));
        assert_eq!(view.item_transform(), idml::Field::Known(vec![1.0, 0.0, 0.0, 1.0, 36.0, 72.0]));
        assert!(
            package
                .semantic_graph()
                .references()
                .iter()
                .all(|r| matches!(r.resolution, idml::semantic::ReferenceResolution::Resolved(_) | idml::semantic::ReferenceResolution::Unlinked))
        );
        let tree = package.part("Spreads/s.xml").unwrap();
        let anchors: Vec<_> = tree.views::<idml::views::PathPointType>().map(|p| p.anchor()).collect();
        assert_eq!(
            anchors,
            vec![
                idml::Field::Known(vec![0.0, 0.0]),
                idml::Field::Known(vec![0.0, 100.0]),
                idml::Field::Known(vec![200.0, 100.0]),
                idml::Field::Known(vec![200.0, 0.0])
            ]
        );
    }
    assert!(Rect::new(0.0, 0.0, f64::INFINITY, 1.0).is_err());
    assert!(Rect::new(2.0, 0.0, 1.0, 1.0).is_err());
    assert!(Transform::new([f64::NAN; 6]).is_err());
    assert_eq!(Transform::translation(4.0, 5.0).unwrap().apply([2.0, 3.0]), [6.0, 8.0]);
}

#[test]
fn frame_errors_and_limits_do_not_commit_partial_source_changes() {
    use idml::{
        Element,
        compose::TextFrameBuilder,
        geometry::{Rect, Transform},
    };
    let mut package = Package::new("8.0".parse().unwrap(), "doc").unwrap();
    package.add_story("Stories/s.xml", StoryBuilder::new("story").unwrap(), &ReadOptions::source()).unwrap();
    let mut spread = Element::new("Spread");
    spread.set_attr("Self", "spread");
    let mut part = Element::new("idPkg:Spread");
    part.children.push(spread);
    package.add_part("Spreads/s.xml", &part, &ReadOptions::source()).unwrap();
    let before = package.write().unwrap();
    let frame =
        |identity, story| TextFrameBuilder::rectangle(identity, story, Rect::new(0.0, 0.0, 100.0, 100.0).unwrap(), Transform::IDENTITY).unwrap();
    assert!(package.add_text_frame("Spreads/s.xml", "spread", frame("frame", "missing"), &ReadOptions::source()).is_err());
    assert!(package.add_text_frame("Spreads/s.xml", "missing", frame("frame", "story"), &ReadOptions::source()).is_err());
    assert!(package.add_text_frame("Spreads/s.xml", "spread", frame("story", "story"), &ReadOptions::source()).is_err());
    assert!(
        package.add_text_frame("Spreads/s.xml", "spread", frame("frame", "story"), &ReadOptions { max_nodes: 2, ..ReadOptions::source() }).is_err()
    );
    assert_eq!(before, package.write().unwrap());
}

#[test]
fn new_part_references_follow_observed_designmap_slots_in_any_construction_order() {
    let catalog = SchemaCatalog::builtin().unwrap();
    let parts = [
        ("Mapping", "XML/Mapping.xml"),
        ("Story", "Stories/s.xml"),
        ("BackingStory", "XML/BackingStory.xml"),
        ("Spread", "Spreads/s.xml"),
        ("MasterSpread", "MasterSpreads/m.xml"),
        ("Tags", "XML/Tags.xml"),
        ("Preferences", "Resources/Preferences.xml"),
        ("Styles", "Resources/Styles.xml"),
        ("Fonts", "Resources/Fonts.xml"),
        ("Graphic", "Resources/Graphic.xml"),
    ];
    for version in ["7.0", "7.5", "8.0", "10.0", "11.0", "15.0", "21.5"] {
        let profile = catalog.effective_profile(version.parse().unwrap()).unwrap();
        let mut package = Package::new(profile.snapshot.dom_version, "doc").unwrap();
        for (kind, path) in parts {
            package.add_part(path, &idml::Element::new(format!("idPkg:{kind}")), &ReadOptions::source()).unwrap();
        }
        let report = catalog.validate(profile, package.document());
        assert!(report.is_supported(), "{version}: {report:?}");
        let tree = package.document();
        let order: Vec<_> = tree.elements(tree.root()).map(|node| tree.name(node).unwrap().local.as_str()).collect();
        assert_eq!(order, ["Graphic", "Fonts", "Styles", "Preferences", "Tags", "MasterSpread", "Spread", "BackingStory", "Story", "Mapping"]);
    }
}

#[test]
fn new_references_reuse_the_existing_packaging_namespace_binding() {
    let mut package = Package::new("21.5".parse().unwrap(), "doc").unwrap();
    let reference = package.add_story("Stories/s.xml", StoryBuilder::new("story").unwrap(), &ReadOptions::source()).unwrap();
    let fragment = package.document().fragment(reference).unwrap();
    assert!(fragment.contains("idPkg:Story"));
    assert!(!fragment.contains("xmlns:"), "{fragment}");
    assert_eq!(package.document().namespace_binding(reference, "idPkg").as_deref(), Some("http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"));
    let parsed = Package::read(&package.write().unwrap(), &ReadOptions::source()).unwrap();
    assert!(parsed.warnings.is_empty());
}
