#![allow(clippy::unwrap_used, clippy::panic)]
use idml::profile::{IssueKind, SchemaCatalog, Support, ValidationOptions};
use idml::{Authoring, Element, Package, ReadOptions, XmlTree};

fn catalog(label: &str) -> (std::path::PathBuf, SchemaCatalog) {
    let root = std::env::temp_dir().join(format!("idml-validation-budget-{}-{label}", std::process::id()));
    let dir = root.join("8.0-budget/package");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("designmap.rnc"),
        r#"# Adobe InDesign Test 8.0.0.1
namespace idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"
grammar {
start=Document_Object
Document_Object=element Document {
  attribute DOMVersion {string "8.0"}, attribute Self {xsd:string},
  (element Item {attribute Kind {string "a" | string "b"}, element Text {text}?} | element idPkg:Story {attribute src {xsd:string}})*
}
}
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Story.rnc"),
        r#"# Adobe InDesign Test 8.0.0.1
namespace idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"
grammar {
start=Story_File
Story_File=element idPkg:Story {attribute DOMVersion {string "8.0"}, element Story {attribute Self {xsd:string}}}
}
"#,
    )
    .unwrap();
    let catalog = SchemaCatalog::from_compressed(&idml_codegen::profiles::bytes(&root).unwrap()).unwrap();
    (root, catalog)
}

#[test]
fn repetitive_choices_stop_with_unknown_at_configured_work_and_depth_limits() {
    let (dir, catalog) = catalog("choices");
    let profile = catalog.effective_profile("8.0".parse().unwrap()).unwrap();
    let items = "<Item Kind='a'><Text>x</Text></Item>".repeat(200);
    let tree = XmlTree::parse(&format!("<Document Self='d' DOMVersion='8.0'>{items}</Document>"), &ReadOptions::source()).unwrap();
    let limited = catalog.validate_with_options(profile, &tree, &ValidationOptions { max_work: 100, max_depth: 256 });
    assert_eq!(limited.support(), Support::Unknown);
    assert_eq!(limited.issues.len(), 1);
    assert_eq!(limited.issues[0].kind, IssueKind::Limit);
    assert!(limited.issues[0].message.contains("during"));
    let deep = catalog.validate_with_options(profile, &tree, &ValidationOptions { max_work: 10_000_000, max_depth: 1 });
    assert_eq!(deep.support(), Support::Unknown);
    assert_eq!(deep.issues[0].kind, IssueKind::Limit);
    assert!(catalog.validate(profile, &tree).is_supported());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn package_validation_charges_every_part_against_one_budget() {
    let (dir, catalog) = catalog("package");
    let profile = catalog.effective_profile("8.0".parse().unwrap()).unwrap();
    let mut package = Package::new("8.0".parse().unwrap(), "d").unwrap();
    let mut part = Element::new("idPkg:Story");
    let mut story = Element::new("Story");
    story.set_attr("Self", "s");
    part.children.push(story);
    package.add_part("Stories/s.xml", &part, &ReadOptions::source()).unwrap();
    assert!(package.validate(&catalog, profile).is_supported());

    // Find the least work sufficient for each independent part, then use a
    // budget enough for either part alone but not the aggregate.
    let minimum = |tree: &XmlTree| {
        let mut lo = 0usize;
        let mut hi = 1_000_000usize;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let options = ValidationOptions { max_work: mid, max_depth: 256 };
            if catalog.validate_with_options(profile, tree, &options).is_supported() {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        lo
    };
    let needs: Vec<_> = package.parts().map(|(_, tree)| minimum(tree)).collect();
    assert_eq!(needs.len(), 2);
    let options = ValidationOptions { max_work: *needs.iter().max().unwrap(), max_depth: 256 };
    assert!(package.parts().all(|(_, tree)| catalog.validate_with_options(profile, tree, &options).is_supported()));
    let report = package.validate_with_options(&catalog, profile, &options);
    assert_eq!(report.support(), Support::Unknown);
    assert_eq!(report.issues[0].kind, IssueKind::Limit);
    assert!(!report.issues[0].location.part.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn checked_authoring_propagates_limits_and_never_removes_a_limit_diagnostic() {
    let (dir, catalog) = catalog("authoring");
    let profile = catalog.effective_profile("8.0".parse().unwrap()).unwrap();
    let package = Package::new("8.0".parse().unwrap(), "d").unwrap();
    let tiny = ValidationOptions { max_work: 0, max_depth: 256 };
    let error = package.write_checked_with_options(&catalog, profile, &tiny).unwrap_err();
    assert!(error.message().contains("limit"));
    assert_eq!(error.kind(), idml::ErrorKind::Limit);
    assert!(Authoring::from_package_with_options(&catalog, profile, package.clone(), tiny).is_err());
    let plan = package.plan_conversion_for_review_with_options(&catalog, profile, &tiny).unwrap();
    assert_eq!(plan.validation().support(), Support::Unknown);
    assert!(plan.changes().iter().all(|change| !change.lossy));
    assert!(!plan.can_apply());

    let ample = ValidationOptions { max_work: 5_000_000, max_depth: 256 };
    let mut author = Authoring::from_package_with_options(&catalog, profile, package, ample).unwrap();
    assert_eq!(author.validation_options(), &ample);
    let draft = author
        .plan_edit(|package| {
            let tree = package.document_mut()?;
            let root = tree.root();
            let mut item = Element::new("Item");
            item.set_attr("Kind", "a");
            tree.insert_element(root, 0, &item)?;
            Ok(())
        })
        .unwrap();
    let draft = draft.for_target_for_review(profile).unwrap();
    assert!(draft.can_apply(), "{:?}", draft.validation());
    draft.apply(&mut author).unwrap();
    assert!(!author.write().unwrap().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}
