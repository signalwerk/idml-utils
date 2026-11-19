//! Explicit-target authoring. A draft may use the shared vocabulary while the
//! committed document always satisfies its selected exact schema profile.
use crate::{
    Error, ErrorKind, Package,
    package::{ConversionChange, ConversionPlan, LossDecision, LossId, LossPolicy, ProfileCompatibility},
    profile::{CompatibilityProfile, DomVersion, SchemaCatalog, SchemaForm, ValidationOptions, ValidationReport},
};

#[derive(Debug)]
pub struct Authoring<'c> {
    catalog: &'c SchemaCatalog,
    profile: CompatibilityProfile,
    package: Package,
    validation_options: ValidationOptions,
}
impl<'c> Authoring<'c> {
    /// Start at an exact effective package profile for the requested version.
    /// Ambiguous versions require explicit profile selection; unobserved
    /// versions return an error rather than guessing a schema.
    pub fn for_version(catalog: &'c SchemaCatalog, version: DomVersion, self_id: impl Into<String>) -> Result<Self, Error> {
        Self::new(catalog, catalog.effective_profile(version)?, self_id)
    }
    pub fn for_version_with_options(
        catalog: &'c SchemaCatalog,
        version: DomVersion,
        self_id: impl Into<String>,
        options: ValidationOptions,
    ) -> Result<Self, Error> {
        Self::new_with_options(catalog, catalog.effective_profile(version)?, self_id, options)
    }
    pub fn new(catalog: &'c SchemaCatalog, profile: &CompatibilityProfile, self_id: impl Into<String>) -> Result<Self, Error> {
        Self::new_with_options(catalog, profile, self_id, ValidationOptions::default())
    }
    /// Start a checked session with explicitly configured validation work limits.
    pub fn new_with_options(
        catalog: &'c SchemaCatalog,
        profile: &CompatibilityProfile,
        self_id: impl Into<String>,
        options: ValidationOptions,
    ) -> Result<Self, Error> {
        Self::from_package_with_options(catalog, profile, Package::new(profile.snapshot.dom_version, self_id)?, options)
    }
    /// Importing into a checked authoring session never upgrades a document.
    /// For unknown/invalid source data, keep using the version-neutral Package.
    pub fn from_package(catalog: &'c SchemaCatalog, profile: &CompatibilityProfile, package: Package) -> Result<Self, Error> {
        Self::from_package_with_options(catalog, profile, package, ValidationOptions::default())
    }
    pub fn from_package_with_options(
        catalog: &'c SchemaCatalog,
        profile: &CompatibilityProfile,
        package: Package,
        options: ValidationOptions,
    ) -> Result<Self, Error> {
        package.write_checked_with_options(catalog, profile, &options)?;
        Ok(Self { catalog, profile: profile.clone(), package, validation_options: options })
    }
    pub fn validation_options(&self) -> &ValidationOptions {
        &self.validation_options
    }
    pub fn profile(&self) -> &CompatibilityProfile {
        &self.profile
    }
    pub fn package(&self) -> &Package {
        &self.package
    }
    pub fn into_package(self) -> Package {
        self.package
    }
    /// Stage any number of edits before checking constraints. A failed draft
    /// never mutates the committed package or changes its selected version.
    pub fn plan_edit(&self, edit: impl FnOnce(&mut Package) -> Result<(), Error>) -> Result<AuthoringPlan<'c>, Error> {
        let input = self.package.state()?;
        let mut draft = self.package.clone();
        edit(&mut draft)?;
        draft.state()?; // Check package/XML invariants without compressing a ZIP.
        draft.check_envelope()?;
        let conversion = ConversionPlan::staged(input, self.catalog, &self.profile, draft.clone(), &self.validation_options);
        Ok(AuthoringPlan {
            catalog: self.catalog,
            source_profile: self.profile.clone(),
            target: self.profile.clone(),
            draft,
            conversion,
            validation_options: self.validation_options,
        })
    }
    pub fn plan_upgrade(&self, target: &CompatibilityProfile) -> Result<AuthoringPlan<'c>, Error> {
        if target.snapshot.dom_version < self.profile.snapshot.dom_version {
            return Err(Error::new(
                ErrorKind::Schema,
                "plan_upgrade requires a same-version or newer target; use for_target for an explicit downgrade",
            ));
        }
        self.plan_edit(|_| Ok(()))?.for_target(target, LossPolicy::Forbid)
    }
    pub fn write(&self) -> Result<Vec<u8>, Error> {
        self.package.write_checked_with_options(self.catalog, &self.profile, &self.validation_options)
    }
}

#[derive(Debug)]
#[must_use = "review and apply the draft, or explicitly discard it"]
pub struct AuthoringPlan<'c> {
    catalog: &'c SchemaCatalog,
    source_profile: CompatibilityProfile,
    target: CompatibilityProfile,
    draft: Package,
    conversion: ConversionPlan,
    validation_options: ValidationOptions,
}
#[derive(Debug, Clone)]
pub struct UpgradeOption {
    pub profile: CompatibilityProfile,
    pub validation: ValidationReport,
    pub changes: Vec<ConversionChange>,
    pub can_apply: bool,
}
impl AuthoringPlan<'_> {
    pub fn target(&self) -> &CompatibilityProfile {
        &self.target
    }
    pub fn validation(&self) -> &ValidationReport {
        self.conversion.validation()
    }
    pub fn can_apply(&self) -> bool {
        self.conversion.can_apply()
    }
    pub fn preview(&self) -> &Package {
        self.conversion.preview()
    }
    pub fn changes(&self) -> &[ConversionChange] {
        self.conversion.changes()
    }
    /// Losses in this proposal. IDs remain valid only for this proposal and its clones.
    pub fn losses(&self) -> impl Iterator<Item = (LossId, &ConversionChange)> + '_ {
        self.conversion.losses()
    }
    pub fn set_loss_approval(&mut self, id: &LossId, approved: bool) -> Result<(), Error> {
        self.conversion.set_loss_approval(id, approved)
    }
    pub fn loss_is_approved(&self, id: &LossId) -> bool {
        self.conversion.loss_is_approved(id)
    }
    /// Decide each loss in this proposal with a callback, replacing earlier
    /// decisions. Retargeting plans again from the original draft and starts
    /// without these decisions; call this again for the new proposal.
    pub fn decide_losses(&mut self, decide: impl FnMut(&ConversionChange) -> LossDecision) -> usize {
        self.conversion.decide_losses(decide)
    }
    /// Losses that the current decisions leave unapproved, in change order.
    pub fn refused_losses(&self) -> impl Iterator<Item = (LossId, &ConversionChange)> + '_ {
        self.conversion.refused_losses()
    }
    /// Test the draft independently against every available snapshot. Gaps in
    /// the catalog remain gaps; the first success is not an introduction date.
    pub fn compatibility(&self) -> Result<Vec<ProfileCompatibility>, Error> {
        self.draft.compatibility_with_options(self.catalog, self.target.amendments_sha256.is_some(), &self.validation_options)
    }
    pub fn compatible_upgrades(&self) -> Result<Vec<CompatibilityProfile>, Error> {
        Ok(self.upgrade_options(LossPolicy::Forbid)?.into_iter().filter(|c| c.can_apply).map(|c| c.profile).collect())
    }
    /// Assess available same-version/newer profiles including reviewed rewrites.
    /// Unlike compatibility(), these options may require reported transformations.
    pub fn upgrade_options(&self, loss_policy: LossPolicy) -> Result<Vec<UpgradeOption>, Error> {
        let mut options = Vec::new();
        for profile in self.catalog.profiles().iter().filter(|p| {
            p.snapshot.form == SchemaForm::Package
                && p.snapshot.dom_version >= self.source_profile.snapshot.dom_version
                && p.amendments_sha256.is_some() == self.target.amendments_sha256.is_some()
        }) {
            let plan = self.draft.plan_conversion_with_options(self.catalog, profile, loss_policy, &self.validation_options)?;
            options.push(UpgradeOption {
                profile: profile.clone(),
                validation: plan.validation().clone(),
                changes: plan.changes().to_vec(),
                can_apply: plan.can_apply(),
            });
        }
        options.sort_by(|a, b| (a.profile.snapshot.dom_version, &a.profile.id).cmp(&(b.profile.snapshot.dom_version, &b.profile.id)));
        Ok(options)
    }
    /// Explicitly choose another target for this same draft. Required rewrites
    /// are reported by the conversion plan; no newest-version fallback exists.
    pub fn for_target(self, target: &CompatibilityProfile, loss_policy: LossPolicy) -> Result<Self, Error> {
        self.for_target_with_migrations(target, loss_policy, crate::migration::MigrationRegistry::builtin()?)
    }
    /// Retarget the original draft and present all available losses for
    /// individual review, initially refusing each one.
    pub fn for_target_for_review(self, target: &CompatibilityProfile) -> Result<Self, Error> {
        self.for_target_for_review_with_migrations(target, crate::migration::MigrationRegistry::builtin()?)
    }
    pub fn for_target_for_review_with_migrations(
        self,
        target: &CompatibilityProfile,
        migrations: &crate::migration::MigrationRegistry,
    ) -> Result<Self, Error> {
        let mut plan = self.for_target_with_migrations(target, LossPolicy::Allow, migrations)?;
        plan.conversion.require_loss_approval();
        Ok(plan)
    }
    pub fn for_target_with_migrations(
        mut self,
        target: &CompatibilityProfile,
        loss_policy: LossPolicy,
        migrations: &crate::migration::MigrationRegistry,
    ) -> Result<Self, Error> {
        if target.snapshot.form != SchemaForm::Package || self.catalog.profile(&target.id) != Some(target) {
            return Err(Error::new(ErrorKind::Schema, "authoring requires an exact registered package profile"));
        }
        let mut conversion =
            self.draft.plan_conversion_with_migrations_and_options(self.catalog, target, loss_policy, migrations, &self.validation_options)?;
        conversion.input = self.conversion.input;
        self.conversion = conversion;
        self.target = target.clone();
        Ok(self)
    }
    /// Both the source bytes and its exact selected profile must still match.
    pub fn apply(self, author: &mut Authoring<'_>) -> Result<Vec<ConversionChange>, Error> {
        if !std::ptr::eq(self.catalog, author.catalog)
            || self.source_profile != author.profile
            || self.validation_options != author.validation_options
        {
            return Err(Error::new(ErrorKind::Write, "authoring plan belongs to a different catalog/profile"));
        }
        let changes = self.conversion.apply(&mut author.package)?;
        author.profile = self.target;
        Ok(changes)
    }
}
