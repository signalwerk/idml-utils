//! Evidence about specific operations, independently of grammar validation.
//!
//! Applications can keep controlled observations here without turning a successful
//! native reopen into a rendering guarantee, or an absent observation into lack of
//! format support. This catalog contains no implicit built-in native claims. Queries
//! match an exact scope; missing dimensions are unknown, never wildcards.
use crate::profile::{CompatibilityProfile, Support};

/// Which operation an observation actually tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Operation {
    SourceRead,
    SourcePreservation,
    SourceEdit,
    SemanticProjection,
    Generation,
    Conversion,
    NativeReopen,
    NativeReadback,
    Rendering,
}

/// The exact context of an observation. Optional fields record missing knowledge;
/// they do not broaden a claim to other environments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvidenceScope {
    /// Crate/application revision that performed a library operation.
    pub implementation_revision: Option<String>,
    /// Exact schema/amendment identity, independently of application build.
    pub profile_fingerprint: Option<String>,
    pub application_build: Option<String>,
    pub platform: Option<String>,
    pub locale: Option<String>,
    /// A stable caller-defined identifier for the captured plug-in inventory.
    /// None means the inventory was not established.
    pub plugin_inventory: Option<String>,
    /// Feature interactions and fixture constraints, such as "one unthreaded frame".
    pub context: String,
}
impl EvidenceScope {
    pub fn for_profile(profile: &CompatibilityProfile, context: impl Into<String>) -> Self {
        Self { profile_fingerprint: Some(profile.fingerprint.clone()), context: context.into(), ..Self::default() }
    }
    /// Whether the dimensions needed to identify this kind of observation are
    /// known. A caller-defined context must also describe feature interactions,
    /// fonts/assets and rendering settings when those affect the experiment.
    pub fn is_complete_for(&self, operation: Operation) -> bool {
        let present = |value: &Option<String>| value.as_ref().is_some_and(|v| !v.trim().is_empty());
        if self.context.trim().is_empty() || !present(&self.platform) {
            return false;
        }
        match operation {
            Operation::NativeReopen | Operation::NativeReadback | Operation::Rendering => {
                present(&self.profile_fingerprint) && present(&self.application_build) && present(&self.locale) && present(&self.plugin_inventory)
            }
            Operation::Generation | Operation::Conversion => present(&self.implementation_revision) && present(&self.profile_fingerprint),
            _ => present(&self.implementation_revision),
        }
    }
}

/// Whether an observation can support an assessment. Proposed and refuted records
/// are retained for audit but cannot establish support or its absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceStatus {
    Proposed,
    Observed,
    Refuted,
}

/// One immutable artifact involved in an experiment. A digest is supplied by the
/// caller; the catalog neither reads files nor certifies their provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub source: String,
    pub sha256: String,
}

/// A controlled observation. Feature names and contexts are application-defined:
/// matching the string "text" does not imply coverage of every text feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub id: String,
    pub feature: String,
    pub operation: Operation,
    pub scope: EvidenceScope,
    pub outcome: Support,
    pub status: EvidenceStatus,
    /// Reference to a report containing the procedure, controls and readbacks.
    pub evidence: String,
    pub artifacts: Vec<Artifact>,
    /// Known exclusions, negative controls and the meaning of the measured result.
    pub limitations: String,
}

/// An exact query; success applies only to this feature, stage and scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceQuery {
    pub feature: String,
    pub operation: Operation,
    pub scope: EvidenceScope,
}

#[derive(Debug, Clone, Default)]
pub struct EvidenceCatalog {
    observations: Vec<Observation>,
}
impl EvidenceCatalog {
    pub fn new(observations: Vec<Observation>) -> Result<Self, crate::Error> {
        let mut ids = std::collections::BTreeSet::new();
        for o in &observations {
            if o.id.trim().is_empty() || o.feature.trim().is_empty() || o.evidence.trim().is_empty() || !ids.insert(&o.id) {
                return Err(crate::Error::new(crate::ErrorKind::Schema, "evidence requires unique nonempty IDs, features and report references"));
            }
            for artifact in &o.artifacts {
                if artifact.source.trim().is_empty() || artifact.sha256.len() != 64 || !artifact.sha256.bytes().all(|c| c.is_ascii_hexdigit()) {
                    return Err(crate::Error::new(crate::ErrorKind::Schema, "evidence artifacts require a source and a 64-digit SHA-256 digest"));
                }
            }
        }
        Ok(Self { observations })
    }
    pub fn observations(&self) -> &[Observation] {
        &self.observations
    }
    pub fn assess(&self, query: &EvidenceQuery) -> EvidenceAssessment<'_> {
        let observations: Vec<_> =
            self.observations.iter().filter(|o| o.feature == query.feature && o.operation == query.operation && o.scope == query.scope).collect();
        let complete_scope = query.scope.is_complete_for(query.operation);
        let usable = |o: &&Observation| o.status == EvidenceStatus::Observed && complete_scope && !o.artifacts.is_empty();
        let positive = observations.iter().any(|o| usable(o) && o.outcome == Support::Supported);
        let negative = observations.iter().any(|o| usable(o) && o.outcome == Support::Unsupported);
        let insufficient_provenance =
            observations.iter().any(|o| o.status == EvidenceStatus::Observed && (!complete_scope || o.artifacts.is_empty()));
        let uncertain = insufficient_provenance || observations.iter().any(|o| o.status == EvidenceStatus::Observed && o.outcome == Support::Unknown);
        let support = match (positive, negative, uncertain) {
            (true, false, false) => Support::Supported,
            (false, true, false) => Support::Unsupported,
            _ => Support::Unknown,
        };
        EvidenceAssessment { support, conflicting: positive && negative, complete_scope, insufficient_provenance, observations }
    }
}

/// The supplied evidence for one query. Even Supported is an observation-scoped
/// result, not a schema, semantic or rendering certification.
#[derive(Debug, Clone)]
pub struct EvidenceAssessment<'a> {
    pub support: Support,
    pub conflicting: bool,
    pub complete_scope: bool,
    pub insufficient_provenance: bool,
    /// Includes proposed/refuted records so the caller can inspect the history.
    pub observations: Vec<&'a Observation>,
}
