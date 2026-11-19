#![allow(clippy::unwrap_used)]
use idml::{evidence::*, profile::Support};

fn observation(id: &str, outcome: Support) -> Observation {
    Observation {
        id: id.into(),
        feature: "threaded-story".into(),
        operation: Operation::NativeReopen,
        scope: EvidenceScope {
            profile_fingerprint: Some("exact-test-profile".into()),
            application_build: Some("test-build".into()),
            platform: Some("test-platform".into()),
            locale: Some("test-locale".into()),
            plugin_inventory: Some("captured-test-inventory".into()),
            context: "two frames".into(),
            ..EvidenceScope::default()
        },
        outcome,
        status: EvidenceStatus::Observed,
        evidence: "controlled-test-report".into(),
        artifacts: vec![Artifact { source: "fixture.idml".into(), sha256: "a".repeat(64) }],
        limitations: "No rendering comparison".into(),
    }
}
fn query() -> EvidenceQuery {
    let o = observation("a", Support::Supported);
    EvidenceQuery { feature: o.feature, operation: o.operation, scope: o.scope }
}
#[test]
fn evidence_is_exact_and_does_not_imply_rendering_or_other_versions() {
    let catalog = EvidenceCatalog::new(vec![observation("a", Support::Supported)]).unwrap();
    let mut q = query();
    assert_eq!(catalog.assess(&q).support, Support::Supported);
    q.operation = Operation::Rendering;
    assert_eq!(catalog.assess(&q).support, Support::Unknown);
    q = query();
    q.scope.application_build = None;
    assert_eq!(catalog.assess(&q).support, Support::Unknown);
    q = query();
    q.scope.profile_fingerprint = Some("other-profile".into());
    assert_eq!(catalog.assess(&q).support, Support::Unknown);
    q = query();
    q.scope.context = "three frames".into();
    assert_eq!(catalog.assess(&q).support, Support::Unknown);
}
#[test]
fn conflicts_and_uncertainty_cannot_claim_support() {
    let catalog = EvidenceCatalog::new(vec![observation("a", Support::Supported), observation("b", Support::Unsupported)]).unwrap();
    let result = catalog.assess(&query());
    assert_eq!(result.support, Support::Unknown);
    assert!(result.conflicting);
    assert_eq!(result.observations.len(), 2);
    let catalog = EvidenceCatalog::new(vec![observation("a", Support::Supported), observation("b", Support::Unknown)]).unwrap();
    assert_eq!(catalog.assess(&query()).support, Support::Unknown);
}
#[test]
fn refutations_and_proposals_remain_visible_but_do_not_qualify() {
    let mut a = observation("a", Support::Supported);
    a.status = EvidenceStatus::Refuted;
    let mut b = observation("b", Support::Unsupported);
    b.status = EvidenceStatus::Proposed;
    let catalog = EvidenceCatalog::new(vec![a, b]).unwrap();
    let result = catalog.assess(&query());
    assert_eq!(result.support, Support::Unknown);
    assert_eq!(result.observations.len(), 2);
    assert_eq!(EvidenceCatalog::default().assess(&query()).support, Support::Unknown);
}
#[test]
fn malformed_or_ambiguous_evidence_is_rejected() {
    assert!(EvidenceCatalog::new(vec![observation("a", Support::Supported), observation("a", Support::Supported)]).is_err());
    let mut a = observation("a", Support::Supported);
    a.artifacts[0].sha256 = "not-a-digest".into();
    assert!(EvidenceCatalog::new(vec![a]).is_err());
}

#[test]
fn missing_dimensions_and_missing_artifacts_do_not_qualify_a_claim() {
    let mut o = observation("a", Support::Supported);
    o.scope.application_build = None;
    let mut q = query();
    q.scope.application_build = None;
    let catalog = EvidenceCatalog::new(vec![o]).unwrap();
    let result = catalog.assess(&q);
    assert_eq!(result.support, Support::Unknown);
    assert!(!result.complete_scope);
    assert!(result.insufficient_provenance);
    let mut o = observation("a", Support::Supported);
    o.artifacts.clear();
    let catalog = EvidenceCatalog::new(vec![o]).unwrap();
    assert_eq!(catalog.assess(&query()).support, Support::Unknown);
}
