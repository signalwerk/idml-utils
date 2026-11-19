# Learning from qxpkit: directions for idml-utils

This report compares the current working trees of idml-utils and the standalone qxpkit crate in the sibling [QuarkXPress repository](../../quarkxpress-file-format/crates/qxpkit/README.md). It is an architectural backlog, not a claim that the two formats have the same document semantics. Priorities suggest an implementation order; each item needs design and evidence before it becomes a compatibility guarantee.

The initial implementation now maps these directions into the following scoped APIs. The recommendations below retain the broader architectural rationale; unobserved behavior and relationships remain unknown.

| Direction | Implementation and verification surface |
| --- | --- |
| Semantic diagnostics | [semantic.rs](../crates/idml/src/semantic.rs): source identities, duplicates and a closed `ReferenceKind` catalog (text links, applied styles, layers, masters, swatches, hyperlink sources), each with owner elements, expected targets and observed nil markers; external `Link` resources; text-thread reconstruction with explicit inconsistency rules. Separate from schema/native validity. |
| Source facade and authoring | [document.rs](../crates/idml/src/document.rs): borrowed stories/frames, opaque text events, transactional source text batches and `text_flows` (containers in thread order from one graph snapshot). [compose.rs](../crates/idml/src/compose.rs): new stories and rectangular frames with explicit [geometry](../crates/idml/src/geometry.rs), identities and target validation. |
| Precise loss review | [package.rs](../crates/idml/src/package.rs) and [authoring.rs](../crates/idml/src/authoring.rs): revocable proposal-scoped approvals, a `decide_losses` policy callback and `refused_losses`, impacts and migration provenance, with final validation and stale checks. |
| Evidence and corpus visibility | [evidence.rs](../crates/idml/src/evidence.rs); the [corpus audit](../crates/idml/examples/corpus.rs) keeps warnings and adds opt-in `--validate` (exact profile resolution and validation) and `--semantic` operations; [optional native harness/observations](../native-qualification.md). No built-in native or rendering certification. |
| Resource bounds | [ReadOptions](../crates/idml/src/options.rs), ZIP length prechecks and local/central/end-record consistency in [archive.rs](../crates/idml/src/archive.rs), XML preflights and edited package checks; [ValidationOptions](../crates/idml/src/profile.rs) for grammar work/depth bounds; deterministic [mutation tests](../crates/idml/tests/mutation.rs) for XML, ZIP metadata and namespace edits. |

Historical-profile, preservation, stale/failed-plan, opaque-content, adversarial-input, budget, thread and mutation tests exercise these APIs. Native observations cover a source no-op, text edit and new story/frame composition in one controlled template on one application build; a minimal story-only package was rejected at native open. Full cross-format conversion, comprehensive layout semantics and rendering equivalence are future work, not inferred from these APIs.

## 1. Add semantic diagnostics separately from schema validation

`Package::validate` validates each part against a grammar; `check_references` checks designmap-to-part paths ([package.rs](../crates/idml/src/package.rs)). Neither establishes that the document's object graph is usable. qxpkit's document model makes pages, stories, styles, assets and their relationships explicit, which suggests a useful additional layer.

Design an **opt-in semantic graph** over source node IDs and part paths. Start with evidence-backed checks for duplicate or unresolved object identities, ambiguous references and story/thread links; expand to style, asset and other relationship types only as their rules are established. Distinguish invalid, unresolved and unknown: an external asset, extension namespace, incomplete package or unrecognized relationship is not automatically broken. Specify which cycles are actually forbidden rather than banning graph cycles generally. Diagnostics should contain stable locations, raw values and scope, and never mutate the source. Keep this result distinct from XML structure, exact schema acceptance and native application behavior. Do not make ordinary preservation reads depend on semantic completeness.

**Acceptance direction:** fixtures covering references across parts, duplicate IDs, missing and external resources, extension content and legitimate cycles; unchanged package bytes after diagnostics; independent tests showing grammar-valid documents can still have graph issues.

## 2. Consider a source-backed semantic facade

qxpkit provides a convenient document-level API for layouts, pages, frames, stories and styles ([model overview](../../quarkxpress-file-format/crates/qxpkit/README.md)). IDML's typed union views expose source XML vocabulary, and `owned` is a projection of one configured schema, not a cross-version semantic document model ([architecture](../architecture.md)). Applications could benefit from higher-level operations without sacrificing the source tree.

First design a read-only, optional facade for a small evidence-backed slice, such as stories and frames, with explicit units, coordinate spaces, defaults, identity and ordering. Return source locations and diagnostics for unmapped, ambiguous or unknown content. Define how IDML-specific spreads, masters, text flow and inheritance work; do not transplant Quark's object ownership or coordinate assumptions. Only after round-trip semantics are understood, add edits as transactional `Authoring` plans against `Package`/`XmlTree`. Preserve unknown siblings, mixed content, versions and lexical data. Treat any alternative rebuilt representation as an explicitly lossy projection, with a report; it must not replace the source authority or drive migrations.

**Decision:** whether this facade belongs in the core crate or an optional adapter, and what source-backed handles and invalidation rules it needs. Prototype on real documents before committing to a large universal model.

## 3. Make loss decisions more precise

`AuthoringPlan` already provides previews, target selection, stale-plan protection and a global loss policy ([authoring.rs](../crates/idml/src/authoring.rs)); qxpkit also lets callers decide per issue ([compatibility API](../../quarkxpress-file-format/crates/qxpkit/src/compat/mod.rs)). Consider a complete, stable loss report with machine-readable issue identity, source location, original fragment, intended replacement, impact, target and rule/evidence. Allow explicit per-issue decisions or a policy callback, including semantic substitutions, rather than forcing callers to approve all loss at once.

Plans must always be recomputed from the original draft for each target and policy; approval of one loss must not hide another. Recheck final target grammar and stale source/profile identity before an atomic apply. Unknown routes, required missing data and failed validation remain blocked even when loss is accepted. Keep existing global forbid as the safe default, and retain a convenient explicit allow-all option if useful.

**Acceptance direction:** mixed accepted/refused losses, changed or stale source, repeated retargets, a similar-but-not-equivalent replacement, and failed final validation all leave the committed document and profile intact.

## 4. Record behavior and writer evidence independently of grammar

qxpkit maintains versioned field descriptions and a research trail from controlled native experiments to schema and tests ([schema](../../quarkxpress-file-format/crates/qxpkit/schema/README.md), [research instructions](../../quarkxpress-file-format/AGENTS.md)). IDML already separates raw/effective grammar and reviewed migrations, and its oracle and optional Jing test grammar acceptance ([architecture](../architecture.md)). Add a separate, evidence-graded catalog for **observed behavior and implementation capabilities**, not more claims in the grammar: what is readable, source-editable, generatable, semantically understood, natively accepted or visually qualified, with unknown as a first-class result.

Scope observations to exact InDesign build, platform, schema/profile, locale and plug-in inventory when known, feature and context. Record fixture hashes, controlled changes, negative controls, outcomes, confidence and counterexamples. Keep inferred/default behavior distinct from schema facts; observations do not automatically become amendments or migration rules. Build small reproducible native fixtures around defaults, linked text, references, fonts/assets, color and layout. Record no-op preservation, edited reopen/readback, fresh generation and rendering separately: a successful reopen is not proof of visual equivalence. Native tests should be optional and usable without making installed InDesign a build requirement.

Improve the current corpus audit as an early step: it discards `Parsed` warnings ([corpus.rs](../crates/idml/examples/corpus.rs)). Include warning counts and locations, strict/lenient and source/owned modes, profile and per-operation results, alongside the existing decompressed-entry comparisons. This makes typed projection omissions and envelope warnings visible without claiming source loss.

## 5. Bound and measure work on difficult documents

IDML already limits XML depth and total decompressed size ([options.rs](../crates/idml/src/options.rs)). Audit archive, arena, schema-validation and cloned-plan memory/work amplification before adding a coherent set of limits: potentially entry count and size, nodes/attributes/text, validation work and conversion search. Make limit failures explicit and separate from invalid-format results; define whether caller-supplied limits cover both parsing and later edits. Benchmark real corpora and large/deep adversarial inputs, including compressed expansion and many small entries, before choosing defaults or introducing streaming APIs. Fuzz malformed XML, ZIP metadata and namespace edits, while keeping unknown data preservation testable.

One concrete early fix: for stored ZIP entries, [archive.rs](../crates/idml/src/archive.rs) currently charges the declared uncompressed size against the limit but copies the packed slice before checking that its length matches. A malformed entry can therefore cause an allocation larger than the configured decompressed-size budget before it is rejected. Check the lengths before copying and add a regression case. Audit local/central directory consistency and compare with independent ZIP readers as a separate interoperability check.

## Sequence and boundaries

Start with corpus warning visibility and a scoped semantic graph; these provide evidence for the facade and native fixture selection. Design per-loss approval against the existing plan invariants. Then expand native observations and resource limits based on measured cases. Keep schema validity, semantic checks, migration loss, source preservation and rendering as separate claims throughout. This report proposes work; it does not certify untested native behavior or an IDML-to-Quark converter.

## Implementation verification

All workspace checks listed in [AGENTS.md](../AGENTS.md) pass: default and
all-feature tests, both Clippy configurations with warnings denied, formatting,
generator freshness (`--check` and `--universal --check`), workspace build, the
default WASM check and the authoring example. All 86 pinned schema files match
their manifests.

### Corpus evidence

One audit of 682 local files (`--roundtrip --validate --semantic`) read 662
strictly and four more leniently; 16 were incomplete, malformed or used
unsupported envelopes. All 666 readable packages kept every non-directory
entry's decompressed contents exactly. The stricter ZIP consistency checks
changed no read outcome. Two already-rejected archives, whose end records are
followed by stray bytes and whose central-directory offsets are wrong, now report
that specifically; Python's `zipfile` also rejects them. Details are in the
local, ignored `target/corpus-audit-final.tsv` and `.log`. A successful audit
exit does not imply that every input was readable.

- **Profiles.** 142 readable packages resolve to an exact available effective
  profile, 522 use releases without a snapshot (for example 159 at 21.0, 65 at
  6.0 and 61 at 14.0), and 2 have no `DOMVersion`. Unobserved versions are
  reported, never validated against a neighbouring snapshot.
- **Validation and a validator defect.** Before this pass, none of the 142
  validated: 124 Unsupported, 18 Unknown. The leading diagnostics were indented
  empty elements such as `<TabList type="list">\n</TabList>`,
  `<TransformParameters>\n</TransformParameters>` and empty `PathPointArray`s,
  whose grammars allow zero children. RELAX NG weak matching (section 6.2.7)
  lets whitespace-only content match as empty. The validator now does this,
  without letting whitespace satisfy required data or treating ACE characters as
  whitespace. Afterwards 121 validate, 19 are Unknown (18 exhaust the default
  validation work budget on large parts; one carries a MathType `MTMathZone`
  attribute outside the profile), and 2 are Unsupported (third-party designmap,
  Fonts and Styles content). The same defect caused the earlier checked-import
  rejection of the native 21.5 baseline; see
  [native-qualification.md](../native-qualification.md). No amendment was needed.
- **Threads.** 49,913 reconstructed threads: 49,521 single-frame, 209 with two
  frames, 51 with three, 30 with four and 102 with five or more. All 2,366
  resolved `NextTextFrame` links were reciprocal. There were no cycles, joins,
  mixed `ParentStory` values or stories split across threads. The only thread
  issue was 120 unfollowable `PreviousTextFrame` links in 6 files. These
  observations define the reported inconsistencies; they are not native
  rejection claims.
- **References.** Every catalog kind resolved to its expected element types.
  Non-resolved values were rare: for example 96 `ParentStory`, 93 `ItemLayer`,
  77 `FillColor` (plus 44 ambiguous), 63 `AppliedCharacterStyle` and 26
  `AppliedMaster`. `n` occurred as a nil marker for previous/next frames,
  applied paragraph, character and object styles, and applied masters (for
  example 28,940 character-style and 1,949 master nils), so only those kinds
  accept it. `Source` is interpreted only on `Hyperlink`; on `InnerGlowSetting`
  it is an enumeration. `BasedOn` lives in typed `Properties` children that mix
  object references and strings such as `$ID/[No paragraph style]`, so it is
  not interpreted.
- **Duplicates.** 1,293 duplicate identities in 67 files, mostly `XMLElement`
  and `XMLAttribute` identities repeated between story and backing-story parts.
  Duplicates therefore remain observations, never invalidity claims.
- **External links.** 7,093 `Link` resources: schemes `file` (6,800),
  `cclibraries` (216), `idinternalcontent` (63) and `idinternalSharedArticle`
  (14); `StoredState` Normal (5,885), Embedded (1,106), Contained (101) and
  absent (1). URIs are recorded, not resolved.

The full audit took 91.03 s wall time (78.98 s user, 5.50 s system CPU) with a
peak resident set of 1,060,716,544 bytes in one optimized run. Formatting and
Clippy ran concurrently, so this is not an isolated performance measurement.
An earlier inventory found entries up to 50,334,911 bytes, archives up to 889
entries and XML parts up to 44,067 elements / 235,469 ordinary attributes. These
measurements support configurable headroom; they do not make `max_size` a bound
on total process memory.

### Defects found by the new property tests

Deterministic mutation tests compare the reader with the independent `zip`
crate and require refused edits to leave trees unchanged. They found:

- An end record whose "entries on this disk" count differed from the total was
  read by the total, while the `zip` crate read by the per-disk count: two
  readers saw different archives. Both counts must now agree, and the
  central-directory size must match its parsed entries.
- An end record whose comment extended past the input was accepted. The end
  record must now end the input exactly; this also keeps a signature inside a
  comment from being chosen.
- `declare_namespace(node, "p", "")` succeeded and left a tree that could not be
  serialized, because Namespaces in XML 1.0 cannot undeclare a prefix. It is now
  refused, matching the parser and producer checks.

### Remaining work

- Native fixtures for defaults, linked text, fonts/assets, colour and layout
  need the optional InDesign harness and were not run. Native evidence is still
  limited to the observations in [native-qualification.md](../native-qualification.md).
  Those observations are prose, not machine-readable `evidence` records.
- Style inheritance (`BasedOn`, `NextStyle`), hyperlink destinations,
  conditions and XML structure links are not interpreted until their forms are
  established. Spreads, pages and masters have no facade, and page membership
  is not inferred.
- 18 large parts exhaust the default validation budget; choosing budgets from
  measured work, and snapshots for the 522 packages at unobserved versions, is
  future work.
