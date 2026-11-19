# Architecture

`idml` has one version-neutral document representation. A schema is a validator
and a vocabulary source, not the storage format. The default library can preserve,
edit and write content from any DOMVersion without first projecting it into the
newest schema. Checked authoring uses an explicit exact target profile.

## Decisions reviewed

| Concern | Decision and reason |
| --- | --- |
| Authoritative storage | Keep the iterative `XmlTree` arena. Stable node IDs carry original XML spans, namespace context and edits, so unknown data survives. |
| Primary package API | `Package` now names that source-backed representation. `new(version, self_id)` requires a version; no implicit latest target exists. |
| Owned structs for one release | Keep as the opt-in `owned` feature (`OwnedPackage`, `model`, `XmlPart`, `PartFile`). They are an adapter, not the universal document. Default builds exclude the recursive current-schema model. |
| Typed vocabulary | Generate one union of historical element names, attributes and values. Avoid complete duplicate models per release. A known typed value is not proof that it is allowed in the current target/context. |
| Version enforcement | Check complete staged edits against an exact profile. Temporary drafts can be incomplete or use a newer vocabulary. Named setters check lexical vocabulary; `set_checked` supplies an optional explicit field-level check. |
| Upgrade selection | The caller selects a target. Show compatibility and upgrade options per observed snapshot; never automatically select the newest or assume every later release supports a field. |
| Conversion rules | Separate reviewed, directed endpoint migrations from current-model projection rules. A successful grammar check is required at the destination. No inverse or missing-release transformations are invented. |
| Loss | Forbid data/semantic loss by default. Mapping to a merely similar replacement is lossy. Retain locations and original fragments in the report. |
| Unknown versions/extensions | Parsing/writing remains possible; schema acceptance remains unknown until a suitable catalog is supplied. No fallback drops or defaults unknown data. |
| Version labels | Preserve per-part source labels. New parts inherit an explicit document version only when missing their own label. Retargeting happens only in conversion proposals; the owned layout cannot override labels. |
| Schema evidence | Keep raw upstream grammars separate from evidence-backed effective corrections. Missing snapshots give observation bounds, not exact introduction dates. |
| Package metadata | Preserve non-directory entry contents, entry order/compression and XML spelling. ZIP timestamps, extra fields and archive comments are regenerated. Reject unsupported formats, and archives whose local headers, central directory, entry ranges or end record disagree, rather than guessing or silently replacing data. |

## Runtime layers

```text
Package (package.rs)
  ├─ XmlTree (tree.rs): flat arena, lexical source, stable IDs, iterative I/O
  ├─ views + view.rs: borrowed fields and producer Element builders
  ├─ SchemaCatalog (profile.rs): independent exact-profile validation
  └─ Authoring (authoring.rs): committed target + transactional drafts
        └─ ConversionPlan + MigrationRegistry (migration.rs)

optional feature owned
  └─ OwnedPackage (projection.rs) + model/ + xml/ + preserve.rs
       └─ projection rules (upgrade.rs), never source migration rules
```

`Package::read` checks XML/package structure, not the schema vocabulary. Unknown
attributes, children, enum literals, processing instructions and comments remain
accessible. `ReadOptions::default()` uses an explicit 4096-level resource limit;
parse, edit, write, clone and drop of the arena do not recurse with document depth.
`Field` distinguishes absent, known and unrecognized data, retaining raw lexical
values and reasons. Raw operations stay available for extension content.

`semantic::SemanticGraph` is a separate read-only identity/reference index over a
closed relationship catalog (`ReferenceKind`). Each kind declares its owner
elements, expected target types and whether `n` is a nil marker; each was added
only after the local corpus showed consistent targets. It reports duplicate
identities and unresolved, ambiguous or unexpected targets, without treating an
incomplete index as proof of invalidity. Typed `Properties` children such as
`BasedOn` mix object and string forms and stay uninterpreted. `Link` URIs are
external resources, never resolved against a file system. Thread reconstruction
follows resolved `NextTextFrame` links; reciprocal previous/next pairs and
story/frame relations are legitimate cycles, while a next-chain that returns to
itself, joins, non-reciprocal links, mixed `ParentStory` values and stories split
over several threads are reported as departures from the observed native pattern. `document` supplies
borrowed source views for stories and text containers. Its iterative text events
walk paragraph/character ranges, emit unfamiliar content as opaque source nodes
and avoid flattening nested stories or inferring geometry/defaults.
`text_flows` adds borrowed containers in thread order from one owned graph
snapshot, so its indices cannot go stale. This is a scoped facade, not a rebuilt
universal document model. `compose` produces new
story parts; `Package::add_story` registers identity/part/StoryList atomically and
can participate in the existing exact-target transactional authoring workflow.

`Element`/`Builder<T>` is a convenient producer representation. Construction from
it feeds the arena directly, avoiding XML serialization/reparsing. Parts added
with `add_part` get a designmap reference together with their content. They inherit
only a missing version, retain explicit namespaces and reject path collisions.
The source arena, rather than recursive Element trees, is appropriate for deep
or mixed-content editing.

## Version-first authoring

```rust
let mut document = Authoring::new(catalog, profile_8, "document")?;
let draft = document.plan_edit(|package| {
    // Insert parts or use typed views, including a newer feature.
    Ok(())
})?;
// draft.validation() explains failures at 8.0.
// draft.compatible_upgrades() lists observed compatible profiles.
let plan = draft.for_target(profile_10, LossPolicy::Forbid)?;
plan.apply(&mut document)?;
let bytes = document.write()?; // Explicitly selected 10.0, never latest.
```

See the runnable `versioned_authoring` example. It starts at 8.0 and adds a
ColorGroup to the designmap, supported by the 10.0 snapshot. It then chooses 10.0
explicitly. Tests also stage `SelectedPageItems` at 8.0 and move to 15.0.

`Authoring` owns a committed Package and its exact profile. `plan_edit` clones
that state, applies the closure and validates the result against the selected
target. A failing closure or blocked proposal leaves committed content intact.
`for_target` always starts from the original edited draft; repeated retargeting
cannot hide loss already performed in another preview. Apply checks the catalog,
profile, uncompressed entry content, arena identity and revision. Stale checks
avoid repeated ZIP compression. Raw attachments are shared across drafts with
copy-on-write mutation; writing borrows unchanged attachments. It commits document/profile
together. `Package::write_checked` is available without an Authoring session.

Unvalidated Package writes intentionally remain available: preservation of an
unknown native export must not depend on proving its schema. A checked authoring
session rejects invalid envelopes and unparsed/missing referenced parts.

## Identity, capabilities and conversion

Keep three identities distinct:

1. A document's DOMVersion, independently on each part.
2. A schema snapshot: DOMVersion, product/build, package/consolidated form,
   locale, plug-in inventory/completeness and original schema hash.
3. A compatibility profile: that snapshot plus explicit corrections, with
   separate upstream/amendment hashes and a combined identity.

Resolving a DOMVersion can be exact, ambiguous or unknown. The caller must choose
an exact profile when several snapshots share a version. Plug-in inventory is
never inferred from a version label. Portable catalogs and application-supplied
migration registries allow independent additions without replacing document types.

Compatibility is a matrix of independently validated observations. For each
available package profile, hypothetically change only version labels and check
all retained DOM parts. This does not mutate the source or apply rewrites. The
first compatible snapshot is not proof of the earliest compatible release, and
support can be non-monotone. Missing releases remain unknown. Upgrade options
separately assess reviewed rewrites and their validation/loss reports.

`schema-migrations.tsv` records explicit directed endpoints, path, operation,
loss impact, reason and evidence. The generator emits `migration_data.rs`.
`MigrationRegistry` also accepts application-supplied rules. Conversion first
tries unchanged content with the chosen labels; if validation fails, it searches
reviewed edges. Intermediate schemas must accept their output. A bridge to
another edge is allowed only after an independent grammar check. The search is
bounded to 64 states; missing/unresolved routes yield blocked proposals.
Lossless routes are preferred. No numeric-version interval makes a rule apply.
The result is one bounded candidate, not a complete enumeration of migration
paths or every loss a different route might entail. Generic removals are one
pass; if they expose further invalid content, final validation blocks the plan.

The initial reviewed edges wrap anchored objects from 6.0 into character ranges
at 7.0, and map the removed StaticOrderedList option from 8.0 to 10.0. That mapping
is explicitly lossy: it chooses a similar export behavior. `LossPolicy::Forbid`
blocks it; Allow still requires target validation. Explicit Allow may also remove
rejected optional content, reporting original fragments. Missing required data,
unknown target profiles or failed target validation continue to block application.

Review proposals (`plan_conversion_for_review` / `for_target_for_review`)
include the available lossy rewrites and optional removals, but initially approve
none of them. Each loss has an opaque proposal-scoped ID, a machine-readable
impact, location, original/replacement and, for reviewed rules, separate rule
and evidence fields. Callers can approve or revoke each loss, or decide all of
them with a `decide_losses` callback that replaces earlier decisions. IDs and
decisions cannot be carried into a separately planned proposal or retarget, and
partial approval never commits a partial preview. Exact profile validation, the checked package
envelope and stale-state checks remain mandatory for every applied proposal.

Schema validation covers grammar, datatypes/facets, literal values, attribute and
child contexts, order, cardinality and required data. Whitespace-only element
content weakly matches as RELAX NG specifies, so the indented empty elements
InDesign writes (`<TabList type="list">\n</TabList>`) match like `<TabList/>`;
whitespace never satisfies required data, and ACE characters are not whitespace. It does not establish
rendering equivalence or resolve semantic object references inside InDesign.
Version-specific rendering/default behavior needs separate native evidence.
Validation spends a finite work budget on grammar traversal, matching, values,
expression expansion and diagnostics, and bounds grammar recursion. Exhaustion
is an `Unknown` limit diagnostic rather than invalid content; package validation
shares one budget across parts. Callers can supply limits for checked output and
authoring, which carries them through subsequent drafts and target planning.

## Generation and evidence

```text
schemas/<build>/{package,single} + optional profile.tsv
  ├─ rnc.rs / rng.rs / schema.rs: parse scoped upstream grammars
  ├─ schema-amendments.tsv: reviewed effective corrections
  ├─ profiles.rs → profile_data.rs: interned raw/effective grammar catalog
  ├─ views.rs → views.rs: shared historical vocabulary and typed access
  ├─ schema-migrations.tsv → migration_data.rs: reviewed endpoint edges
  ├─ history.rs → schema-history.tsv: raw observations and bounded dates
  └─ configured projection schema only
       ├─ lower.rs / emit.rs → model/ + schema-map.tsv
       ├─ schema-upgrades.tsv / upgrade.rs → model/upgrades.rs
       └─ report.rs → docs/
```

`--universal` generates catalogs, union views and migration data independently of
lowering the owned projection, leaving its model/reports untouched.
`--export-catalog` exports the portable catalog independently as well. Its
schemas may have per-part differences; they are not merged into one global
meaning. `--export-schemas [--raw]` emits standalone RNG for independent Jing
validation. New unsupported schema constructs fail generation rather than being
silently approximated. Raw snapshots and amendments have separate provenance.

The owned lowering maps elements to structs, enumerations to enums, optional/
repeated content to Option/Vec, tagged alternatives to enums, constrained numbers
and lists to checked types, and interleaved text-bearing children to an ordered
child enum. It checks that every reachable pattern is represented. This is a
claim about the configured effective grammar only. The source arena does not
require an owned mapping for every future element.

Owned reads may omit legacy/unknown content from the projection with warnings,
while OwnedPackage/XmlPart retain the original XML. Their conservative merger
rejects ambiguous edits. Their recursive traversal caps depth at 48. Such
projection rules must not be interpreted as universal conversion guarantees.

## Verification

The schema oracle produces documents directly from grammars, independently of
the owned lowering. Profile tests cover every available raw/effective snapshot
and preserve each part's definition scope. Native corpus round trips check every
non-directory entry's decompressed contents. Source tests cover stable IDs,
namespace moves, unknown values, deep XML, transactional application, schema gaps
and non-monotone capabilities. Authoring tests cover starting at an older target,
new features, explicit upgrades, stale plans and acknowledged semantic loss.

Run default and all-features workspace tests, both Clippy configurations, generator
freshness, formatting and WASM checks. Optional Jing validates output independently
of the oracle/generator. A valid schema result is not a substitute for opening a
document in InDesign when a change claims rendering or native behavior.
