# Schema investigation and versioning proposal

Investigated on 26 October 2026 against the integration in the sibling
`indd-utils` checkout and its 682-file IDML corpus.

## What the evidence establishes

The failures are not all a Rust-generator interpretation error. A fresh schema
export from the installed **InDesign 21.5.1.73** has exactly the same reachable
patterns as the published **21.5.0.64** snapshot used by this crate. Comparing
standalone RNG exports of both snapshots gives no differences. The source
files differ in build headers and missing comment markers in `datatype.rnc`,
not in the grammar. The new local snapshot was kept outside `schemas/` so an
unpublished build does not become a required generation input.

Jing independently rejects an external hyperlink destination copied from a
corpus export against Adobe's raw grammar: `type="list"` and `ListItem` are
not allowed. It accepts the same XML against the effective grammar exported
by the updated generator. Thus this particular contradiction exists before
Rust lowering or reading. The same schema text excludes the reported endnote,
cross-reference text and table-cell XML content.

Adobe explicitly says that schema generation depends on the installed plug-in
configuration, in the [IDML Cookbook, schema validation section](https://community.adobe.com/havfw69955/attachments/havfw69955/indesign/632677/1/IDML_cookbook_9627253.pdf).
The [application API](https://developer.adobe.com/indesign/uxp/dom/api/a/application/)
provides runtime schema generation. A DOMVersion is consequently insufficient
to identify every allowed plug-in object or property. WordsFlow and MathTools
content should not be presented as universal Adobe schema features. Some corpus
files are also hand-authored test fixtures with injected attributes; their mere
presence is not proof of native output.

## Changes made

- Corrections now have stable IDs, inclusive application bounds, target paths,
  operations and evidence. Supported operations add content/attributes, add a
  choice alternative, replace a value pattern (including ranges), and make an
  attribute optional. Undefined references and missing applicable targets fail
  generation.
- Adobe's original item history stays raw. Corrections are identified separately
  in the map, history, generated Rust documentation and amendment report. The
  model exposes separate upstream and amendment hashes. Application bounds are
  not claimed introduction dates.
- The effective grammar supports external hyperlink lists, cross-reference text,
  direct story hyperlink sources, cell XML and zero hyperlink zoom, alongside
  the earlier endnote and text-field corrections. Further native evidence found
  while removing earlier blockers supports dangling hyperlinks without Source
  (12.0 and 20.4 exports), and orphan PDF comments without CommentDestination
  (17.0 export). These remain actual objects rather than being skipped.
- `OwnedPackage` (feature `owned`) retains source XML and a baseline of its typed projection. Unchanged
  parts reuse their source; edits patch known attributes/text without discarding
  surrounding data. This includes old fields removed from the projection,
  unknown plug-in data, namespaces, comments, instructions, XML spelling,
  original per-part DOMVersions, nonstandard rootfile paths, and leniently
  accepted package envelopes. `XmlPart<T>` provides this for standalone parts.
- Unsafe or ambiguous source edits return `ErrorKind::Write`. Structural
  replacements require agreement between source and baseline. Version-label
  changes, additions to a different source DOMVersion, and changes to migrated
  attribute values are conservatively rejected. `discard_source()` explicitly
  opts into potentially lossy serialization of the projection.
- Lenient optional choices that match no alternative can be omitted from the
  projection with a warning while their source remains intact. Required invalid
  data still fails. The lower-level `PartFile` API is explicitly a projection.
- Standalone raw/effective RNG exports permit independent validation. The optional
  Jing integration test checks written packages with recorded layouts and edits.
  The corpus example reports strict/lenient failures by path and checks entry
  contents after writing.

## Owned-projection corpus results

| Result | Before | After |
| --- | ---: | ---: |
| Strictly readable | 446 | 553 |
| Additional lenient reads | 108 | 113 |
| Unreadable | 128 | 16 |
| Total readable | 554 | 666 |

Unedited readable packages preserve all non-directory entry contents, including
XML and old DOMVersions. ZIP timestamps, extra fields and archive comments are
not covered by this content-preservation contract; the archive itself is
regenerated. The remaining failures comprise corrupt/truncated archives, missing
parts/required identities, and fixtures using a non-IDML DesignMap root or
namespace. Some of those fixtures are labeled valid by their source repository;
that label does not make them native InDesign output.

Strict failures still include WordsFlow and MathTools extensions, legacy fields
such as UntitledDocumentCount/PageNumberStyle, and unusual fixture content.
These are preserved by lenient package reads, but are not all exposed as typed
fields. There is no claim that every corpus file is a valid native export or
that passing the effective grammar proves InDesign can render a document.

Reproduce, after restoring schema snapshots:

```sh
cargo run --release --locked -p idml --features owned --example corpus -- --owned --roundtrip ../indd-utils/corpus > /tmp/idml-audit.tsv
cargo run --locked -p idml-codegen -- --export-schemas target/effective-schemas
cargo run --locked -p idml-codegen -- --export-schemas target/raw-schemas --raw
IDML_JING_JAR=/path/to/jing.jar cargo test --workspace --all-features --locked
```

## Implemented version architecture

Use three separate identities:

1. **Document version:** the exact DOMVersion on each source part. Reading and
   ordinary edits preserve it; parts can have different versions.
2. **Schema snapshot:** DOMVersion, product/build, package/consolidated form,
   locale, known plug-in configuration and original schema hash. Several snapshots
   can share a DOMVersion. Missing plug-in inventory is explicitly unknown.
3. **Compatibility profile:** an identified set of snapshots plus reviewed
   corrections, with an independent hash and evidence. “Observed in 19.0” must
   remain distinct from “introduced in 19.0” or “valid for every 19.x installation.”

Keep a lossless XML tree as the authoritative document. Generate typed views
and builders over its nodes, sharing one node interface with producer-built
`Element` trees. Unknown attributes, children, enum literals and property-choice
alternatives remain raw nodes/values. Distinguish absent, understood and
unrecognized values; do not turn an unrecognized value into a default. Stable
node IDs should own formatting and provenance, with Self used as an additional
semantic identity. That supports insertion and movement without assigning a
neighbour's layout, and removes the extra serialize/parse pass in from_elements.

Generate a shared typed vocabulary from the snapshot union, with capability
rules per profile. Keep historical spellings and alternatives available instead
of dropping them just to fit the newest snapshot. Do not duplicate the entire
large model into one module per InDesign release. Typed views can expose known
variants while preserving raw values for newer or plug-in-specific variants.

Make conversion a separate operation, conceptually:

```text
package.plan_conversion(target_profile, loss_policy = Forbid)
  -> edits, exact validations, uncertain capabilities, unsupported content
```

The plan must account for retained extensions and raw values as well as typed
fields. Lossless rewrites can proceed; unsupported or uncertain data blocks a
claimed validated conversion. A caller who explicitly selects a lossy policy
receives a complete report with source locations and original fragments. Changing
only DOMVersion is never a conversion. Ordinary reads must not select this
policy implicitly.

Capability checks should have **supported / unsupported / unknown** outcomes.
A historical interval such as `> 11.0 <= 15.0` cannot prove that an item existed
in 12.0. Likewise, a missing plug-in schema is unknown rather than permission to
strip that plug-in's data. Conversion should succeed only when the chosen
validation guarantee is met; preservation should work even without such proof.

## Implementation status

The primary source-backed path is implemented in `Package`, `XmlTree`,
`view`/generated `views`, and `profile`/generated `profile_data`. The arena stores
lexical spans and stable node IDs; namespace context travels with moved nodes.
A streaming quick-xml parser replaces recursive XML parsing on this path. An
800-level edit/clone/write/drop test passes on a 2 MiB thread stack. Unknown
schema values remain source nodes rather than being defaulted or skipped.

Generated views/builders use the union of the available snapshots, including
historical enum literals. Exact raw/effective grammar catalogs share interned
patterns across releases, preserve per-part definition scopes, and expose
supported/unsupported/unknown results. A missing DOMVersion snapshot is not
inferred from its neighbours. Custom catalogs can carry locale and structured
plug-in inventories; missing/incomplete inventories remain explicit. Named
setters check the union vocabulary without choosing a schema. Authoring sessions
validate complete drafts against explicit profiles; `set_checked` provides an
optional explicit field-level check.

Conversion plans apply reviewed non-lossy rewrites before considering removals,
keep original fragments in every loss report, validate all retained DOM parts,
and compare the exact source state before applying atomically. Forbid is the
default policy. Unknown target capabilities and required-data failures still
block application even with Allow. Source migrations have explicit directed endpoints, impact and evidence in
`schema-migrations.tsv`, separate from owned projection rules. Non-lossy routes
are preferred and intermediate outputs are validated. A closest-remaining enum
replacement is explicitly semantic loss; reverse/missing-release rewrites are
not guessed.

The owned current-model API is opt-in (`owned`) and exposes `OwnedPackage` for adapters. Its conservative
source merger and stack-sensitive traversal are not the authoritative editing
path. `OwnedPackage::from_elements` reads Element-built arena nodes directly,
without an XML serialization/reparse step. The source API avoids that owned
projection entirely.

Validation covers schema grammar, values, ranges, order, cardinality and required
data; it does not establish rendering equivalence or semantic reference validity
inside InDesign. No snapshots were fabricated for missing versions. The 1e11
floating-point exponent threshold still has the previously documented evidence
limitation and was not changed by guessing.

The source corpus audit reads 666 of 682 files (662 with strict package/XML
structure and four additional lenient envelopes). All 666 retain every entry's
contents byte for byte on an unedited round trip. These counts describe parsing
and preservation, not schema acceptance. The remaining failures are malformed,
missing required package parts or use non-IDML roots/namespaces.

## Version-first authoring review (28 October 2026)

`Package` is now the version-neutral primary API, with
`Package::new(version, self_id)` requiring an explicit version. New parts inherit
only a missing version. No new-package path in the universal API chooses latest.
The previous current-schema Package has been renamed `OwnedPackage`, behind an
opt-in feature; the unchecked Layout version override has been removed.

`Authoring` holds an exact committed target. Staged edits can use newer shared
vocabulary, expose validation/upgrade options, and explicitly choose another
profile. Plans commit the document and target atomically, checking source bytes,
arena identity and revisions. Repeated retargeting starts from the original draft
so loss cannot disappear from the report. Compatibility is per observed snapshot,
not a monotone range. Missing snapshots remain unknown. The executable
`versioned_authoring` example demonstrates 8.0 → 10.0; tests also cover 8.0 → 15.0,
unknown future versions, non-monotone capability matrices and semantic-loss blocks.

See [architecture.md](architecture.md) for the complete decision review.
