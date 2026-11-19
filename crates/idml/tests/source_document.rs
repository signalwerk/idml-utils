#![allow(clippy::unwrap_used, clippy::panic)]
mod common;

use idml::document::{StoryEvent, TextContainerKind};
use idml::{Field, Package, ReadOptions};

const NS: &str = "xmlns:idPkg='http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging'";

fn fixture() -> Package {
    let designmap =
        format!("<Document {NS} DOMVersion='21.5' Self='d'><idPkg:Story src='Stories/s.xml'/><idPkg:Spread src='Spreads/p.xml'/></Document>");
    let story = format!(
        "<idPkg:Story {NS} xmlns:p='urn:extension' DOMVersion='21.5'><Story Self='s' TrackChanges='future'><ParagraphStyleRange><CharacterStyleRange><Content>start &amp; <?ACE 00a0?>end</Content><Br/><Footnote><Content>hidden</Content></Footnote><p:Custom><Content>extension</Content></p:Custom><?keep raw?><Content>A<?keep inner?>B</Content><Content>bad<?ACE invalid?></Content></CharacterStyleRange></ParagraphStyleRange></Story></idPkg:Story>"
    );
    let spread = format!(
        "<idPkg:Spread {NS} DOMVersion='21.5'><Spread Self='spread'><TextFrame Self='f' ParentStory='s' NextTextFrame='n' ItemTransform='1 0 broken 1 0 0'/></Spread></idPkg:Spread>"
    );
    let files = vec![
        ("mimetype".into(), idml::MIMETYPE.into()),
        ("META-INF/container.xml".into(), common::CONTAINER.into()),
        ("designmap.xml".into(), designmap),
        ("Stories/s.xml".into(), story),
        ("Spreads/p.xml".into(), spread),
    ];
    Package::read(&common::zip(&files), &ReadOptions::source()).unwrap().value
}

#[test]
fn facade_preserves_source_and_exposes_only_main_story_text() {
    let p = fixture();
    let before = p.write().unwrap();
    let doc = p.source_document();
    let story = doc.stories().next().unwrap();
    assert_eq!(story.self_id(), Field::Known("s".to_string()));
    assert!(matches!(story.track_changes(), Field::Unrecognized { raw, .. } if raw == "future"));
    assert_eq!(story.source().location().part, "Stories/s.xml");
    let events: Vec<_> = story.content().collect();
    assert_eq!(events.len(), 7);
    assert!(matches!(&events[0], StoryEvent::Text { value: Field::Known(text), .. } if text == "start & \u{a0}end"));
    assert!(matches!(events[1], StoryEvent::Break { .. }));
    let fragments: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            StoryEvent::Opaque { source } => Some(source.fragment().unwrap()),
            _ => None,
        })
        .collect();
    assert_eq!(fragments.len(), 4);
    assert!(fragments.iter().any(|f| f.contains("<Footnote><Content>hidden</Content></Footnote>")));
    assert!(fragments.iter().any(|f| f.contains("<p:Custom><Content>extension</Content></p:Custom>")));
    assert!(fragments.iter().any(|f| f == "<?keep raw?>"));
    assert!(fragments.iter().any(|f| f.contains("<?keep inner?>")));
    assert!(matches!(&events[6], StoryEvent::Text { value: Field::Unrecognized { .. }, .. }));
    assert_eq!(p.write().unwrap(), before);
}

#[test]
fn frames_expose_raw_fields_without_inventing_defaults_or_units() {
    let p = fixture();
    let doc = p.source_document();
    let frame = doc.text_containers().next().unwrap();
    assert_eq!(frame.kind(), TextContainerKind::TextFrame);
    assert_eq!(frame.parent_story(), Field::Known("s".to_string()));
    assert_eq!(frame.previous_text_frame(), Field::Absent);
    assert_eq!(frame.next_text_frame(), Field::Known("n".to_string()));
    assert!(matches!(frame.item_transform(), Field::Unrecognized { raw, .. } if raw == "1 0 broken 1 0 0"));
    assert_eq!(frame.source().location().part, "Spreads/p.xml");
}

#[test]
fn explicit_content_edits_preserve_opaque_siblings_and_reject_stale_selections() {
    let mut package = fixture();
    let before = common::unzip(&package.write().unwrap());
    let events: Vec<_> = package.source_document().stories().next().unwrap().content().collect();
    let replacement = events[0].replace_text("changed <&>\u{7}").unwrap();
    assert!(events[1].replace_text("no").is_err());
    assert!(events[2].replace_text("no").is_err());
    assert!(events[6].replace_text("no").is_err());
    // A public event can be constructed by a caller, so validate the actual
    // source rather than trusting a caller-supplied Known value.
    let StoryEvent::Opaque { source } = events[5] else { panic!("expected opaque Content") };
    let forged = StoryEvent::Text { source, value: Field::Known("pretend".into()) };
    assert!(forged.replace_text("no").is_err());
    let StoryEvent::Text { source, .. } = events[6] else { panic!("expected invalid ACE content") };
    let forged = StoryEvent::Text { source, value: Field::Known("pretend".into()) };
    assert!(forged.replace_text("no").is_err());
    package.replace_text(std::slice::from_ref(&replacement)).unwrap();
    let after = common::unzip(&package.write().unwrap());
    assert_eq!(before["designmap.xml"], after["designmap.xml"]);
    assert_eq!(before["Spreads/p.xml"], after["Spreads/p.xml"]);
    assert!(after["Stories/s.xml"].contains("<Content>changed &lt;&amp;&gt;<?ACE 7?></Content>"));
    assert!(after["Stories/s.xml"].contains("<Footnote><Content>hidden</Content></Footnote>"));
    assert!(after["Stories/s.xml"].contains("A<?keep inner?>B"));
    assert!(package.replace_text(&[replacement]).is_err());
    assert_eq!(common::unzip(&package.write().unwrap()), after);
}

#[test]
fn content_batch_validates_all_selections_before_commit() {
    let mut package = idml::Package::new("8.0".parse().unwrap(), "doc").unwrap();
    package
        .add_story(
            "Stories/body.xml",
            idml::compose::StoryBuilder::new("story")
                .unwrap()
                .paragraph(idml::compose::Paragraph::new().run(idml::compose::TextRun::new("a")).run(idml::compose::TextRun::new("b"))),
            &ReadOptions::source(),
        )
        .unwrap();
    let replacements: Vec<_> = package.source_document().stories().next().unwrap().content().filter_map(|e| e.replace_text("edited").ok()).collect();
    assert_eq!(replacements.len(), 2);
    let before = package.write().unwrap();
    assert!(package.replace_text(&[replacements[0].clone(), replacements[0].clone()]).is_err());
    assert_eq!(before, package.write().unwrap());
    let invalid = package.source_document().stories().next().unwrap().content().last().unwrap().replace_text("\u{fffe}").unwrap();
    assert!(package.replace_text(&[replacements[0].clone(), invalid]).is_err());
    assert_eq!(before, package.write().unwrap());
    let mut wrong_source = idml::Package::read(&before, &ReadOptions::source()).unwrap().value;
    assert!(wrong_source.replace_text(&replacements).is_err());
    assert_eq!(wrong_source.write().unwrap(), before);
    package.replace_text(&replacements).unwrap();
    let texts: Vec<_> = package
        .source_document()
        .stories()
        .next()
        .unwrap()
        .content()
        .filter_map(|e| match e {
            StoryEvent::Text { value: Field::Known(t), .. } => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["edited", "edited"]);
}

#[test]
fn text_flows_order_borrowed_containers_by_thread_links() {
    let designmap = format!(
        "<Document {NS} DOMVersion='21.5' Self='d' StoryList='s unplaced'><idPkg:Story src='Stories/s.xml'/><idPkg:Spread src='Spreads/p.xml'/></Document>"
    );
    let story = format!("<idPkg:Story {NS} DOMVersion='21.5'><Story Self='s'/><Story Self='unplaced'/></idPkg:Story>");
    // Tree order differs from thread order; the TextPath continues the thread.
    let spread = format!(
        "<idPkg:Spread {NS} DOMVersion='21.5'><Spread Self='spread'><TextFrame Self='f2' ParentStory='s' PreviousTextFrame='f1' NextTextFrame='p'/><GraphicLine Self='g'><TextPath Self='p' ParentStory='s' PreviousTextFrame='f2' NextTextFrame='n'/></GraphicLine><TextFrame Self='f1' ParentStory='s' PreviousTextFrame='n' NextTextFrame='f2'/></Spread></idPkg:Spread>"
    );
    let files = vec![
        ("mimetype".into(), idml::MIMETYPE.into()),
        ("META-INF/container.xml".into(), common::CONTAINER.into()),
        ("designmap.xml".into(), designmap),
        ("Stories/s.xml".into(), story),
        ("Spreads/p.xml".into(), spread),
    ];
    let p = Package::read(&common::zip(&files), &ReadOptions::source()).unwrap().value;
    let before = p.write().unwrap();
    let document = p.source_document();
    let flows = document.text_flows();
    assert_eq!(flows.threads().len(), 1);
    let thread = &flows.threads()[0];
    assert!(thread.issues.is_empty());
    let containers = flows.containers(thread);
    let order: Vec<_> = containers.iter().map(|c| (c.self_id(), c.kind())).collect();
    assert_eq!(
        order,
        [
            (Field::Known("f1".into()), TextContainerKind::TextFrame),
            (Field::Known("f2".into()), TextContainerKind::TextFrame),
            (Field::Known("p".into()), TextContainerKind::TextPath)
        ]
    );
    assert_eq!(containers[2].source().location().part, "Spreads/p.xml");
    let story = flows.story(thread).unwrap();
    assert_eq!(story.self_id(), Field::Known("s".into()));
    let stories: Vec<_> = document.stories().collect();
    assert_eq!(flows.threads_of(&stories[0]).count(), 1);
    // An unplaced story has no thread; this is not reported as an error.
    assert_eq!(flows.threads_of(&stories[1]).count(), 0);
    assert_eq!(p.write().unwrap(), before);
}
