#![allow(clippy::unwrap_used, clippy::panic)]
mod common;
use idml::profile::{CompatibilityProfile, ProfileResolution, SchemaCatalog, Support};
use idml::{Authoring, DomVersion, Element, LossPolicy, Package, ReadOptions, Token};

fn profile(version: &str) -> &'static CompatibilityProfile {
    match SchemaCatalog::builtin().unwrap().resolve(version.parse().unwrap(), true) {
        ProfileResolution::Exact(profile) => profile,
        other => panic!("{other:?}"),
    }
}

#[test]
fn new_packages_and_parts_require_and_keep_the_chosen_version() {
    for version in ["7.0", "7.5", "8.0", "10.0", "11.0", "15.0", "21.5", "99.3"] {
        let mut package = Package::new(version.parse().unwrap(), "doc").unwrap();
        let mut part = Element::new("idPkg:Story");
        let mut story = Element::new("Story");
        story.set_attr("Self", "story");
        part.children.push(story);
        package.add_part("Stories/story.xml", &part, &ReadOptions::default()).unwrap();
        assert_eq!(package.document().dom_version(), Some(version));
        assert_eq!(package.part("Stories/story.xml").unwrap().dom_version(), Some(version));
        let files = common::unzip(&package.write().unwrap());
        assert!(files["designmap.xml"].contains(&format!("DOMVersion=\"{version}\"")));
        let read = Package::read(&package.write().unwrap(), &ReadOptions::default()).unwrap();
        assert_eq!(common::unzip(&read.value.write().unwrap()), files);
        let before = package.write().unwrap();
        assert!(package.add_part("Stories/story.xml", &part, &ReadOptions::default()).is_err());
        assert_eq!(package.write().unwrap(), before);
    }
}

#[test]
fn version_eight_authoring_can_adopt_a_new_feature_without_jumping_to_latest() {
    let catalog = SchemaCatalog::builtin().unwrap();
    let mut author = Authoring::new(catalog, profile("8.0"), "d").unwrap();
    let before = author.write().unwrap();
    let selected = vec![Token::new("page1").unwrap()];
    let plan = author
        .plan_edit(|draft| {
            let tree = draft.document_mut()?;
            let root = tree.root();
            tree.view_mut::<idml::views::Document>(root).unwrap().set_selected_page_items(&selected)
        })
        .unwrap();
    assert!(!plan.can_apply());
    assert_eq!(plan.validation().support(), Support::Unsupported);
    assert_eq!(author.write().unwrap(), before);
    let candidates = plan.compatible_upgrades().unwrap();
    assert_eq!(candidates.first().unwrap().snapshot.dom_version, "15.0".parse::<DomVersion>().unwrap());
    let plan = plan.for_target(profile("15.0"), LossPolicy::Forbid).unwrap();
    assert!(plan.can_apply(), "{:?}", plan.validation());
    plan.apply(&mut author).unwrap();
    assert_eq!(author.profile(), profile("15.0"));
    assert_eq!(author.package().document().dom_version(), Some("15.0"));
    assert_eq!(author.package().document().attribute(author.package().document().root(), "SelectedPageItems"), Some("page1"));
    assert!(!common::unzip(&author.write().unwrap())["designmap.xml"].contains("21.5"));
}

#[test]
fn generated_builders_and_new_parts_work_in_an_explicit_old_target() {
    let catalog = SchemaCatalog::builtin().unwrap();
    let mut author = Authoring::new(catalog, profile("8.0"), "d").unwrap();
    let plan = author
        .plan_edit(|package| {
            let graphic = idml::view::Builder::<idml::views::GraphicFile>::new().build();
            package.add_part("Resources/Graphic.xml", &graphic, &ReadOptions::default())?;
            let mut group = idml::view::Builder::<idml::views::ColorGroup>::new();
            group.set_self_(&"group".to_owned())?;
            let tree = package.document_mut()?;
            let root = tree.root();
            let index = tree.elements(root).count();
            tree.insert_element(root, index, &group.build())?;
            Ok(())
        })
        .unwrap();
    assert!(!plan.can_apply());
    let plan = plan.for_target(profile("10.0"), LossPolicy::Forbid).unwrap();
    assert!(plan.can_apply(), "{:?}", plan.validation());
    plan.apply(&mut author).unwrap();
    assert_eq!(author.package().part("Resources/Graphic.xml").unwrap().dom_version(), Some("10.0"));
    // A fresh forward proposal may explicitly choose a non-latest intermediate target.
    let plan = author.plan_upgrade(profile("11.0")).unwrap();
    assert!(plan.can_apply());
    plan.apply(&mut author).unwrap();
    assert_eq!(author.profile(), profile("11.0"));
}

#[test]
fn jing_accepts_new_eight_and_upgraded_ten_documents_when_available() {
    let Some(jar) = std::env::var_os("IDML_JING_JAR") else { return };
    let catalog = SchemaCatalog::builtin().unwrap();
    let root = idml_codegen::configured_history_dir(&idml_codegen::idml_crate_dir()).unwrap().unwrap();
    let temp = std::env::temp_dir().join(format!("idml-authoring-jing-{}", std::process::id()));
    std::fs::create_dir_all(&temp).unwrap();
    let mut author = Authoring::new(catalog, profile("8.0"), "d").unwrap();
    for version in ["8.0", "10.0"] {
        if version == "10.0" {
            author
                .plan_edit(|package| {
                    let mut group = idml::view::Builder::<idml::views::ColorGroup>::new();
                    group.set_self_(&"g".to_owned())?;
                    let tree = package.document_mut()?;
                    let root = tree.root();
                    tree.insert_element(root, 0, &group.build())?;
                    Ok(())
                })
                .unwrap()
                .for_target(profile(version), LossPolicy::Forbid)
                .unwrap()
                .apply(&mut author)
                .unwrap();
        }
        let schema = idml_codegen::schema::Schema::load_effective_parts(&root.join(&profile(version).source))
            .unwrap()
            .into_iter()
            .find(|s| s.entries.iter().any(|e| e.root.qualified() == "Document"))
            .unwrap();
        let entry = schema.entries.iter().find(|e| e.root.qualified() == "Document").unwrap();
        let grammar = temp.join(format!("{version}.rng"));
        let xml = temp.join(format!("{version}.xml"));
        std::fs::write(&grammar, idml_codegen::rng::write(&schema, entry).unwrap()).unwrap();
        std::fs::write(&xml, &common::unzip(&author.write().unwrap())["designmap.xml"]).unwrap();
        let output = std::process::Command::new("java").arg("-jar").arg(&jar).arg(grammar).arg(xml).output().unwrap();
        assert!(output.status.success(), "Jing {version}: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn draft_failures_and_stale_plans_never_change_committed_content_or_profile() {
    let mut author = Authoring::new(SchemaCatalog::builtin().unwrap(), profile("8.0"), "d").unwrap();
    let before = author.write().unwrap();
    assert!(
        author
            .plan_edit(|draft| {
                let tree = draft.document_mut()?;
                let root = tree.root();
                tree.set_attribute(root, "Name", "temporary")?;
                Err(idml::Error::new(idml::ErrorKind::Write, "abort"))
            })
            .is_err()
    );
    assert_eq!(author.write().unwrap(), before);
    let stale = author.plan_upgrade(profile("15.0")).unwrap();
    author
        .plan_edit(|draft| {
            let tree = draft.document_mut()?;
            let root = tree.root();
            tree.set_attribute(root, "Name", "new")
        })
        .unwrap()
        .apply(&mut author)
        .unwrap();
    let changed = author.write().unwrap();
    assert!(stale.apply(&mut author).is_err());
    assert_eq!(author.write().unwrap(), changed);
    assert_eq!(author.profile(), profile("8.0"));
    // Identical XML from another arena is not the same source identity.
    let mut other = Authoring::new(SchemaCatalog::builtin().unwrap(), profile("8.0"), "d").unwrap();
    let independent = Authoring::new(SchemaCatalog::builtin().unwrap(), profile("8.0"), "d").unwrap();
    let plan = independent.plan_upgrade(profile("15.0")).unwrap();
    assert!(plan.apply(&mut other).is_err());
}

#[test]
fn retargeting_a_draft_twice_does_not_hide_loss_from_the_first_target() {
    let catalog = SchemaCatalog::builtin().unwrap();
    let mut package = Package::new("8.0".parse().unwrap(), "d").unwrap();
    let mut part = Element::new("idPkg:Preferences");
    let mut pref = Element::new("HTMLExportPreference");
    pref.set_attr("NumberedListExportOption", "StaticOrderedList");
    part.children.push(pref);
    package.add_part("Resources/Preferences.xml", &part, &ReadOptions::default()).unwrap();
    let author = Authoring::from_package(catalog, profile("8.0"), package).unwrap();
    let plan = author
        .plan_edit(|_| Ok(()))
        .unwrap()
        .for_target(profile("10.0"), LossPolicy::Allow)
        .unwrap()
        .for_target(profile("15.0"), LossPolicy::Forbid)
        .unwrap();
    assert!(!plan.can_apply());
    assert!(plan.changes().iter().any(|c| c.lossy));
}

#[test]
fn closest_remaining_values_are_reported_as_semantic_loss() {
    let mut package = Package::new("8.0".parse().unwrap(), "d").unwrap();
    let mut part = Element::new("idPkg:Preferences");
    let mut pref = Element::new("HTMLExportPreference");
    pref.set_attr("NumberedListExportOption", "StaticOrderedList");
    part.children.push(pref);
    package.add_part("Resources/Preferences.xml", &part, &ReadOptions::default()).unwrap();
    let before = package.write().unwrap();
    let plan = package.plan_conversion(SchemaCatalog::builtin().unwrap(), profile("10.0"), LossPolicy::Forbid).unwrap();
    assert!(!plan.can_apply());
    assert!(plan.changes().iter().any(|c| c.lossy && c.kind == idml::package::ChangeKind::MapValue));
    let change = plan.changes().iter().find(|c| c.kind == idml::package::ChangeKind::MapValue).unwrap();
    assert_eq!(change.loss_impact(), Some(idml::package::LossImpact::SemanticChange));
    assert!(change.migration_rule.is_some());
    assert!(change.evidence.is_some());
    assert!(plan.apply(&mut package).is_err());
    assert_eq!(package.write().unwrap(), before);
    let plan = package.plan_conversion(SchemaCatalog::builtin().unwrap(), profile("10.0"), LossPolicy::Allow).unwrap();
    assert!(plan.can_apply(), "{:?}", plan.validation());
    plan.apply(&mut package).unwrap();
    assert_eq!(package.document().dom_version(), Some("10.0"));
    assert!(package.part("Resources/Preferences.xml").unwrap().to_xml().unwrap().contains("NumberedListExportOption=\"OrderedList\""));
}

#[test]
fn review_plan_requires_each_loss_and_keeps_ids_with_their_proposal() {
    let catalog = SchemaCatalog::builtin().unwrap();
    let mut author = Authoring::new(catalog, profile("8.0"), "d").unwrap();
    let before = author.write().unwrap();
    let draft = author
        .plan_edit(|package| {
            let tree = package.document_mut()?;
            let root = tree.root();
            tree.set_attribute(root, "PluginFirst", "one")?;
            tree.set_attribute(root, "PluginSecond", "two")?;
            Ok(())
        })
        .unwrap();
    let mut review = draft.for_target_for_review(profile("8.0")).unwrap();
    let losses: Vec<_> = review.losses().map(|(id, change)| (id, change.attribute.clone())).collect();
    assert_eq!(losses.len(), 2, "{:?}", review.changes());
    assert!(review.validation().is_supported());
    assert!(!review.can_apply());
    assert!(losses.iter().all(|(id, _)| !review.loss_is_approved(id)));
    assert!(review.losses().all(|(_, c)| c.loss_impact() == Some(idml::package::LossImpact::DataRemoval)));
    review.set_loss_approval(&losses[0].0, true).unwrap();
    assert!(!review.can_apply());
    assert_eq!(author.write().unwrap(), before);
    review.set_loss_approval(&losses[1].0, true).unwrap();
    assert!(review.can_apply());
    review.set_loss_approval(&losses[0].0, false).unwrap();
    assert!(!review.can_apply());
    review.set_loss_approval(&losses[0].0, true).unwrap();

    // Identical changes in a newly planned proposal do not accept old IDs.
    let mut other = author
        .plan_edit(|package| {
            let tree = package.document_mut()?;
            let root = tree.root();
            tree.set_attribute(root, "PluginFirst", "one")?;
            tree.set_attribute(root, "PluginSecond", "two")?;
            Ok(())
        })
        .unwrap()
        .for_target_for_review(profile("8.0"))
        .unwrap();
    assert!(other.set_loss_approval(&losses[0].0, true).is_err());
    let new_target = other.for_target_for_review(profile("10.0")).unwrap();
    assert!(!new_target.loss_is_approved(&losses[0].0));
    review.apply(&mut author).unwrap();
    assert_eq!(author.write().unwrap(), before);
}

#[test]
fn approvals_cannot_override_invalid_target_or_stale_source() {
    let catalog = SchemaCatalog::builtin().unwrap();
    let mut package = Package::new("8.0".parse().unwrap(), "d").unwrap();
    let tree = package.document_mut().unwrap();
    let root = tree.root();
    tree.remove_attribute(root, "Self").unwrap();
    tree.set_attribute(root, "PluginField", "value").unwrap();
    let before = package.write().unwrap();
    let mut plan = package.plan_conversion_for_review(catalog, profile("8.0")).unwrap();
    let losses: Vec<_> = plan.losses().map(|(id, _)| id).collect();
    assert!(!losses.is_empty());
    for id in &losses {
        plan.set_loss_approval(id, true).unwrap();
    }
    assert!(!plan.can_apply());
    assert!(plan.apply(&mut package).is_err());
    assert_eq!(package.write().unwrap(), before);

    let tree = package.document_mut().unwrap();
    let root = tree.root();
    tree.set_attribute(root, "Self", "d").unwrap();
    let mut plan = package.plan_conversion_for_review(catalog, profile("8.0")).unwrap();
    let losses: Vec<_> = plan.losses().map(|(id, _)| id).collect();
    for id in &losses {
        plan.set_loss_approval(id, true).unwrap();
    }
    assert!(plan.can_apply());
    package.document_mut().unwrap().set_attribute(root, "Name", "changed").unwrap();
    let changed = package.write().unwrap();
    assert!(plan.apply(&mut package).is_err());
    assert_eq!(package.write().unwrap(), changed);
}

#[test]
fn conversion_rejects_invalid_envelope_even_when_content_matches_target() {
    let catalog = SchemaCatalog::builtin().unwrap();
    let mut package = Package::new("8.0".parse().unwrap(), "d").unwrap();
    package.other_files_mut().remove("mimetype");
    let before = package.write().unwrap();
    let plan = package.plan_conversion_for_review(catalog, profile("8.0")).unwrap();
    assert!(!plan.can_apply());
    assert!(plan.validation().issues.iter().any(|issue| issue.message.contains("envelope")));
    assert!(plan.apply(&mut package).is_err());
    assert_eq!(package.write().unwrap(), before);
}

#[test]
fn changing_one_allow_all_decision_preserves_other_decisions() {
    let catalog = SchemaCatalog::builtin().unwrap();
    let mut package = Package::new("8.0".parse().unwrap(), "d").unwrap();
    let tree = package.document_mut().unwrap();
    let root = tree.root();
    tree.set_attribute(root, "PluginFirst", "one").unwrap();
    tree.set_attribute(root, "PluginSecond", "two").unwrap();
    let mut plan = package.plan_conversion(catalog, profile("8.0"), LossPolicy::Allow).unwrap();
    let losses: Vec<_> = plan.losses().map(|(id, _)| id).collect();
    assert_eq!(losses.len(), 2);
    assert!(plan.can_apply());
    let mut cloned = plan.clone();
    cloned.set_loss_approval(&losses[0], false).unwrap();
    assert!(!cloned.loss_is_approved(&losses[0]));
    assert!(cloned.loss_is_approved(&losses[1]));
    assert!(!cloned.can_apply());
    assert!(plan.can_apply());
    plan.set_loss_approval(&losses[0], false).unwrap();
    assert!(!plan.can_apply());
    plan.set_loss_approval(&losses[0], true).unwrap();
    assert!(plan.can_apply());
}

#[test]
fn unknown_versions_and_extensions_remain_writable_without_a_schema_projection() {
    let mut package = Package::new("42.0".parse().unwrap(), "d").unwrap();
    let tree = package.document_mut().unwrap();
    let root = tree.root();
    tree.set_attribute(root, "PluginValue", "future & raw").unwrap();
    tree.insert_element(root, 0, &Element::new("FutureObject")).unwrap();
    let before = common::unzip(&package.write().unwrap());
    let parsed = Package::read(&package.write().unwrap(), &ReadOptions::default()).unwrap();
    assert!(parsed.warnings.is_empty());
    assert_eq!(common::unzip(&parsed.value.write().unwrap()), before);
    assert!(!package.validate(SchemaCatalog::builtin().unwrap(), profile("21.5")).is_supported());
    assert!(Authoring::from_package(SchemaCatalog::builtin().unwrap(), profile("21.5"), package).is_err());
}

#[test]
fn absent_and_unrecognized_version_labels_stay_distinct_and_preserved() {
    for (attribute, absent) in [("", true), (" DOMVersion='future-label'", false)] {
        let xml = format!("<Document Self='d'{attribute}/>");
        let bytes = common::zip(&[
            ("mimetype".into(), idml::MIMETYPE.into()),
            ("META-INF/container.xml".into(), common::CONTAINER.into()),
            ("designmap.xml".into(), xml.clone()),
        ]);
        let package = Package::read(&bytes, &ReadOptions::default()).unwrap().value;
        let versions = package.versions();
        match &versions["designmap.xml"] {
            idml::Field::Absent => assert!(absent),
            idml::Field::Unrecognized { raw, .. } => {
                assert!(!absent);
                assert_eq!(raw, "future-label");
            }
            other => panic!("unexpected version state: {other:?}"),
        }
        assert_eq!(common::unzip(&package.write().unwrap())["designmap.xml"], xml);
    }
}

#[test]
fn plans_check_uncompressed_content_identity_and_revision_without_sharing_mutable_files() {
    let catalog = SchemaCatalog::builtin().unwrap();
    let mut original = Package::new("8.0".parse().unwrap(), "d").unwrap();
    original.other_files_mut().insert("asset.bin".into(), vec![7; 1024]);
    let plan = original.plan_conversion(catalog, profile("10.0"), LossPolicy::Forbid).unwrap();
    let mut branch = original.clone();
    branch.other_files_mut().get_mut("asset.bin").unwrap()[0] = 9;
    assert_eq!(original.other_files()["asset.bin"][0], 7);
    assert!(plan.clone().apply(&mut branch).is_err());
    let tree = original.document_mut().unwrap();
    let root = tree.root();
    tree.set_attribute(root, "Name", "temporary").unwrap();
    tree.remove_attribute(root, "Name").unwrap();
    // Restoring identical XML must not make the old revision current again.
    assert!(plan.apply(&mut original).is_err());
}

#[test]
fn compatibility_is_observed_per_snapshot_and_is_not_a_monotone_version_range() {
    let dir = std::env::temp_dir().join(format!("idml-authoring-matrix-{}", std::process::id()));
    for version in [8, 10, 15] {
        let snapshot = dir.join(format!("{version}.0-test/package"));
        std::fs::create_dir_all(&snapshot).unwrap();
        let value = if version == 10 { "middle" } else { "early" };
        let text = format!(
            "# Adobe InDesign Test {version}.0.0.1\ngrammar {{ start=Document_Object\nDocument_Object=element Document {{attribute DOMVersion {{string \"{version}.0\"}}, attribute Self {{xsd:string}}, attribute Mode {{string \"{value}\"}}?}}\n}}\n"
        );
        std::fs::write(snapshot.join("designmap.rnc"), text).unwrap();
    }
    let catalog = SchemaCatalog::from_compressed(&idml_codegen::profiles::bytes(&dir).unwrap()).unwrap();
    let mut package = Package::new("8.0".parse().unwrap(), "d").unwrap();
    let tree = package.document_mut().unwrap();
    let root = tree.root();
    tree.set_attribute(root, "Mode", "early").unwrap();
    let matrix = package.compatibility(&catalog, true).unwrap();
    assert_eq!(
        matrix.iter().map(|p| (p.profile.snapshot.dom_version.major, p.validation.support())).collect::<Vec<_>>(),
        [(8, Support::Supported), (10, Support::Unsupported), (15, Support::Supported)]
    );
    assert!(matches!(catalog.resolve("12.0".parse().unwrap(), true), ProfileResolution::Unknown { .. }));
    assert_eq!(package.document().dom_version(), Some("8.0"));
    // Application-supplied migrations work with independently supplied catalogs.
    let target = match catalog.resolve("10.0".parse().unwrap(), true) {
        ProfileResolution::Exact(p) => p,
        _ => panic!(),
    };
    let migrations = idml::migration::MigrationRegistry::new(vec![idml::migration::MigrationRule {
        id: "application-mode".into(),
        from: "8.0".parse().unwrap(),
        to: "10.0".parse().unwrap(),
        path: vec!["Document".into()],
        action: idml::migration::MigrationAction::MapValue { name: "Mode".into(), from: "early".into(), to: "middle".into() },
        lossy: true,
        reason: "application chooses different behavior".into(),
        evidence: "application's explicit mapping contract".into(),
    }])
    .unwrap();
    let blocked = package.plan_conversion_with_migrations(&catalog, target, LossPolicy::Forbid, &migrations).unwrap();
    assert!(!blocked.can_apply());
    let accepted = package.plan_conversion_with_migrations(&catalog, target, LossPolicy::Allow, &migrations).unwrap();
    assert!(accepted.can_apply(), "{:?}", accepted.validation());
    accepted.apply(&mut package).unwrap();
    assert_eq!(package.document().attribute(package.document().root(), "Mode"), Some("middle"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn loss_callbacks_decide_each_issue_and_never_outlive_their_proposal() {
    use idml::LossDecision;
    use idml::package::LossImpact;
    let catalog = SchemaCatalog::builtin().unwrap();
    let mut author = Authoring::new(catalog, profile("8.0"), "d").unwrap();
    let before = author.write().unwrap();
    let draft = author
        .plan_edit(|package| {
            let tree = package.document_mut()?;
            let root = tree.root();
            tree.set_attribute(root, "PluginKeep", "one")?;
            tree.set_attribute(root, "PluginDrop", "two")?;
            Ok(())
        })
        .unwrap();
    let mut review = draft.for_target_for_review(profile("8.0")).unwrap();
    assert_eq!(review.target(), profile("8.0"));
    let approved =
        review.decide_losses(|change| if change.attribute.as_deref() == Some("PluginDrop") { LossDecision::Approve } else { LossDecision::Refuse });
    assert_eq!(approved, 1);
    assert!(!review.can_apply());
    let refused: Vec<_> = review.refused_losses().map(|(_, c)| c.attribute.clone()).collect();
    assert_eq!(refused, [Some("PluginKeep".to_owned())]);
    assert!(review.validation().is_supported());
    assert_eq!(author.write().unwrap(), before);

    // A callback replaces earlier decisions, including a bulk Allow.
    assert_eq!(review.decide_losses(|_| LossDecision::Approve), 2);
    assert!(review.can_apply());
    assert_eq!(review.refused_losses().count(), 0);
    // Retargeting plans again from the original draft without earlier decisions.
    let mut retargeted = review.for_target_for_review(profile("10.0")).unwrap();
    assert!(retargeted.losses().count() >= 2);
    assert_eq!(retargeted.refused_losses().count(), retargeted.losses().count());
    assert!(!retargeted.can_apply());
    retargeted.decide_losses(|_| LossDecision::Approve);
    retargeted.apply(&mut author).unwrap();
    assert_eq!(author.profile(), profile("10.0"));

    let mut allowed = author
        .plan_edit(|package| {
            let tree = package.document_mut()?;
            let root = tree.root();
            tree.set_attribute(root, "PluginOnly", "x")
        })
        .unwrap()
        .for_target(profile("10.0"), LossPolicy::Allow)
        .unwrap();
    assert!(allowed.can_apply());
    assert_eq!(allowed.decide_losses(|_| LossDecision::Refuse), 0);
    assert!(!allowed.can_apply());

    // Decisions can depend on impact: approve semantic substitutions, refuse removals.
    let mut package = Package::new("8.0".parse().unwrap(), "d").unwrap();
    let mut part = Element::new("idPkg:Preferences");
    let mut pref = Element::new("HTMLExportPreference");
    pref.set_attr("NumberedListExportOption", "StaticOrderedList");
    part.children.push(pref);
    package.add_part("Resources/Preferences.xml", &part, &ReadOptions::default()).unwrap();
    let root = package.document().root();
    package.document_mut().unwrap().set_attribute(root, "PluginField", "value").unwrap();
    let original = package.write().unwrap();
    let mut plan = package.plan_conversion_for_review(catalog, profile("10.0")).unwrap();
    let impacts: Vec<_> = plan.losses().map(|(_, c)| c.loss_impact()).collect();
    assert!(impacts.contains(&Some(LossImpact::SemanticChange)) && impacts.contains(&Some(LossImpact::DataRemoval)), "{impacts:?}");
    plan.decide_losses(|c| if c.loss_impact() == Some(LossImpact::SemanticChange) { LossDecision::Approve } else { LossDecision::Refuse });
    assert!(plan.refused_losses().all(|(_, c)| c.loss_impact() == Some(LossImpact::DataRemoval)));
    assert!(plan.clone().apply(&mut package).is_err());
    assert_eq!(package.write().unwrap(), original);
    plan.decide_losses(|_| LossDecision::Approve);
    plan.apply(&mut package).unwrap();
    assert_eq!(package.document().dom_version(), Some("10.0"));
    assert!(package.document().attribute(root, "PluginField").is_none());
}
