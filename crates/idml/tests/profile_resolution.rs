#![allow(clippy::unwrap_used, clippy::panic)]

use idml::profile::{ProfileResolution, SchemaCatalog};
use idml::{Authoring, ErrorKind};

#[test]
fn convenient_authoring_starts_at_the_exact_requested_old_version() {
    let catalog = SchemaCatalog::builtin().unwrap();
    let version = "8.0".parse().unwrap();
    let profile = catalog.effective_profile(version).unwrap();
    assert_eq!(profile.snapshot.dom_version, version);
    let author = Authoring::for_version(catalog, version, "document").unwrap();
    assert_eq!(author.profile(), profile);
    assert_eq!(author.package().document().dom_version(), Some("8.0"));
    assert!(!author.write().unwrap().is_empty());
}

#[test]
fn missing_and_ambiguous_versions_never_fall_back() {
    let dir = std::env::temp_dir().join(format!("idml-profile-resolution-{}", std::process::id()));
    for (name, version, build, form) in [
        ("8-a", "8.0", "8.0.0.1", "package"),
        ("8-b", "8.0", "8.0.0.2", "package"),
        ("9-single", "9.0", "9.0.0.1", "single"),
        ("10", "10.0", "10.0.0.1", "package"),
    ] {
        let part = dir.join(name).join(form);
        std::fs::create_dir_all(&part).unwrap();
        let grammar = format!(
            "# Adobe InDesign Test {build}\ngrammar {{ start=Document_Object\nDocument_Object=element Document {{attribute DOMVersion {{string \"{version}\"}}, attribute Self {{xsd:string}}}}\n}}\n"
        );
        std::fs::write(part.join("designmap.rnc"), grammar).unwrap();
    }
    let catalog = SchemaCatalog::from_compressed(&idml_codegen::profiles::bytes(&dir).unwrap()).unwrap();
    let version8 = "8.0".parse().unwrap();
    let candidates = match catalog.resolve(version8, true) {
        ProfileResolution::Ambiguous(profiles) => profiles,
        other => panic!("expected ambiguous: {other:?}"),
    };
    assert_eq!(candidates.len(), 2);
    let err = catalog.effective_profile(version8).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Schema);
    assert!(err.message().contains("8.0"));
    for profile in candidates {
        assert!(err.message().contains(&profile.id));
        // An informed caller can still choose an exact build explicitly.
        let author = Authoring::new(&catalog, profile, "d").unwrap();
        assert_eq!(author.profile(), profile);
    }
    assert!(Authoring::for_version(&catalog, version8, "d").is_err());
    assert!(catalog.resolve(version8, false).require_exact().unwrap_err().message().contains("multiple exact package profiles"));

    let version9 = "9.0".parse().unwrap();
    match catalog.resolve(version9, true) {
        ProfileResolution::Unknown { before, after } => {
            assert_eq!(before, Some(version8));
            assert_eq!(after, Some("10.0".parse().unwrap()));
        }
        other => panic!("expected unknown: {other:?}"),
    }
    let err = catalog.effective_profile(version9).unwrap_err();
    assert!(err.message().contains("9.0"));
    assert!(err.message().contains("8.0"));
    assert!(err.message().contains("10.0"));
    assert!(err.message().contains("do not establish compatibility"));
    assert!(Authoring::for_version(&catalog, version9, "d").is_err());
    assert_eq!(catalog.effective_profile("10.0".parse().unwrap()).unwrap().snapshot.dom_version, "10.0".parse().unwrap());
    assert!(catalog.resolve("10.0".parse().unwrap(), false).require_exact().unwrap().amendments_sha256.is_none());
    std::fs::remove_dir_all(dir).unwrap();
}
