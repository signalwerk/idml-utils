# idml-utils

Rust workspace for version-neutral Adobe InDesign IDML reading, authoring and
writing. Preserve unknown data; parsing, schema acceptance and conversion are
separate operations.

**Every agent must update this file whenever meaningful changes are made,
assumptions are clarified, or new project context is discovered. Keep it concise,
retain useful guidance, and remove stale information.**

## Architecture and decisions

- `crates/idml/src/package.rs`: authoritative `Package`, explicit-version
  construction, parts, ZIP I/O and conversion plans. `Package::new(version, id)`
  never selects the newest release. New parts inherit the document version only
  when their own version is absent. Reads/writes preserve mixed part versions.
- `tree.rs`: iterative lossless XML arena; stable node IDs own lexical spans,
  namespace context and edits. `view.rs` and generated `views.rs` expose the
  union vocabulary of all snapshots. `Field` distinguishes absent, known and
  unrecognized values. Named setters check vocabulary; they do not select a
  schema or silently upgrade. `set_checked` checks an explicit exact profile.
- `authoring.rs`: exact-target `Authoring` sessions. Stage edits with `plan_edit`,
  inspect validation/upgrade options, explicitly `for_target`, then apply.
  Failed or stale plans leave the committed document/profile unchanged. Retarget
  each proposal from the original draft so earlier loss cannot disappear from
  its report. Check source arena identity/revision and uncompressed entry data on
  apply; planning must not repeatedly compress ZIPs. Raw attachments are shared
  across drafts with copy-on-write access through `other_files_mut`.
- `profile.rs` / generated `profile_data.rs`: raw/effective exact schema catalog,
  full grammar validation and supported/unsupported/unknown capabilities.
  Compatibility is a per-snapshot matrix, never an assumed monotone range.
  Whitespace-only leaf content weakly matches as empty (RELAX NG 6.2.7);
  InDesign writes indented empty elements. Generated oracle instances are
  unindented, so test whitespace handling separately.
- `semantic.rs` and `document.rs`: opt-in source identity/link diagnostics and
  borrowed story/text-container views. Unresolved links are observations, not
  native validity claims. Opaque story content stays opaque; do not flatten it.
  `ReferenceKind` is a closed catalog (owners, expected targets, nil `n`); add a
  kind only after corpus evidence shows consistent targets. `BasedOn` lives in
  typed `Properties` children with object and string forms: not interpreted.
  `Link` URIs are external, never resolved. Thread issues (cycle, join,
  non-reciprocal, mixed story, split story) depart from the observed native
  pattern; reciprocal prev/next pairs are legitimate cycles. Native exports
  repeat `XMLElement`/`XMLAttribute` identities, so duplicates stay neutral.
- `compose.rs`: producers for new story content, with explicit identity and
  breaks. `add_story` registers part/StoryList atomically; exact-target authoring
  validates the resulting draft. Rectangular text frames use explicit local
  point geometry/parent transforms (`geometry.rs`); page membership is not inferred.
  Producers never rebuild existing stories. Source text replacement batches
  reject stale/duplicate selections and retain opaque sibling content.
- New designmap references use the observed sibling assembly slots and reuse
  an existing idPkg namespace binding. A 21.5.1.73 native control ignored new
  story payloads with a redundant declaration on the reference; preserve such
  source XML on reads rather than turning this observation into a parse guard.
- A designmap built by `Package::from_elements`/`Package::new` carries InDesign's
  `aid` processing instruction for its DOMVersion (`XmlTree::producer_instruction`,
  counted in size and node limits like the generated declaration). InDesign 2026
  refuses to open a package whose designmap lacks it; read designmaps keep their own.
- `evidence.rs`: caller-supplied operation-scoped observations, separate from
  schema acceptance. Incomplete environment/provenance and conflicting evidence
  remain unknown; native reopening never implies rendering equivalence.
- `migration.rs` / generated `migration_data.rs` / `schema-migrations.tsv`:
  reviewed directed migration edges with explicit endpoints, loss impact and
  evidence, independent of the owned model. A closest-remaining replacement is
  semantic loss. Default loss policy is Forbid; final target validation is
  mandatory even with Allow. Missing routes/profiles remain blocked.
- Feature `owned` (off by default) enables `model/`, `OwnedPackage`
  (`projection.rs`), `XmlPart<T>`, `PartFile` and the recursive XML projection.
  This adapter targets the single effective schema configured in Cargo metadata;
  `schema-upgrades.tsv` describes that projection only. It must never drive
  universal source migrations. There is no Layout version-label override.
- `crates/idml-codegen`: RNC/RNG parsing, scoped grammars, catalogs, union views,
  reviewed migrations, optional owned projection, test oracle and HTML reports.
  Do not hand-edit generated files. Regenerate Rust, TSV and `docs/` together.
- `schemas/`: pinned download script and checksums; actual version folders are
  ignored. `[package.metadata.idml]` in `crates/idml/Cargo.toml` configures the
  owned projection schema/history. `--universal` generates the runtime catalog/views/migrations independently of
  that projection; `--export-catalog` exports a portable catalog. Optional `profile.tsv` beside `package/` records locale,
  plugin name/version rows and inventory completeness; missing inventory stays
  explicit.
- `schema-amendments.tsv`: reviewed corrections with IDs, inclusive scopes,
  operations and evidence. Scope is not an introduction date. Upstream snapshots
  and history remain raw; effective grammars apply corrections. Hash separately.

DOMVersion alone does not identify a schema: build, form, locale and plug-ins
matter. Missing snapshots/configuration imply **unknown**. A common field can
change or disappear; a version-label change alone is not a validated conversion.
Unknown/future versions remain readable/writable as source data without a schema
projection. Exact schema acceptance does not prove rendering or semantic
cross-reference validity in InDesign. See `architecture.md` and
`schema-investigation.md` for design and evidence.

`learnings/quarkxpress-cross-review.md` records the qxpkit comparison and its
implementation mapping. Scope/limitations of optional native experiments live
in `native-qualification.md`; ignored local evidence is not a public fixture.

## Build and verification

Rust 1.88+ / edition 2024. From the workspace root:

```sh
cargo build --workspace --locked
./schemas/download.sh
./schemas/download.sh --verify
cargo run --locked -p idml-codegen
cargo run --locked -p idml-codegen -- --check
cargo run --locked -p idml-codegen -- --universal --check
cargo test --workspace --locked
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all -- --check
cargo check --locked -p idml --target wasm32-unknown-unknown
cargo run --locked -p idml --example versioned_authoring
```

After dependencies/schemas are restored, add `--offline`. Generated files are
checked in; ordinary builds need neither schema downloads nor InDesign.
The freshness test checks Rust, TSV and HTML artifacts.

```sh
cargo run --locked -p idml-codegen -- --export-schemas target/effective-schemas
cargo run --locked -p idml-codegen -- --export-schemas target/raw-schemas --raw
cargo run --locked -p idml-codegen -- --export-catalog target/idml-profiles.bin
cargo run --locked -p idml --example profiles
IDML_JING_JAR=/path/to/jing.jar cargo test --workspace --all-features --locked
cargo run --release --locked -p idml --example corpus -- --roundtrip /path/to/corpus
cargo run --release --locked -p idml --features owned --example corpus -- --owned --roundtrip /path/to/corpus
```

Jing is optional and external. The oracle shares generator inputs; Jing checks
output independently. Corpus mode defaults to universal source parsing, writes
TSV details to stdout and grouped counts to stderr. Owned mode is explicit.
Source-mode `--validate` (exact profile per designmap version, never a
neighbour) and `--semantic` are separate opt-in operations.

## Conventions and gotchas

- Unsafe Rust, unwrap/expect, panic, todo and unimplemented are denied in
  production. Tests may use narrow allowances.
- Runtime dependencies: `roxmltree`, `quick-xml`, `miniz_oxide`. The arena uses
  streaming quick-xml; roxmltree remains for shallow container XML and the owned
  adapter. Avoid ZIP dependencies introducing WebAssembly host imports.
- Default source depth limit is 4096. The recursive owned reader additionally
  caps depth at 48; use source views for deep groups. `Element` is a producer
  convenience, not the authoritative storage for deep or mixed XML content.
- `ReadOptions` also bounds entry count/size and per-part XML node/attribute
  counts. `Package::check_limits` measures current entries without compression;
  edited XML is serialized in memory. `ValidationOptions` bounds grammar work
  and recursion independently; exhaustion is Unknown, never removable content.
  The source corpus audit preserved entry contents for all 666 readable inputs
  of 682 files; 121 of the 142 at an available exact profile validate (18
  Unknown from work limits). See the learnings report for warnings, rejected
  inputs, semantic counts and one-run resource measurements. Do not equate an
  audit exit code with coverage.
- The ZIP reader rejects reader-ambiguous archives: local/central name, method or
  (without data descriptor) CRC/sizes mismatch, shared entry bytes, disagreeing
  end-record counts or directory size, or an end-record comment not ending the
  input. `tests/mutation.rs` cross-checks the `zip` crate and edit atomicity.
- Per-loss approvals are scoped to a conversion proposal; retargeting never
  carries them forward. Reviewed changes expose rule/evidence and loss impact.
- Preserve namespaces on moves and expanded attribute names. Keep unknown nodes,
  instructions, comments, lexical values and unreferenced files. ZIP timestamps,
  extra fields and archive comments are regenerated; non-directory entry contents
  are the round-trip contract. Reject non-UTF-8 filenames instead of replacing.
- Keep InDesign's number formatting. The 1e11 exponent threshold has limited
  evidence; change it only with an oracle/export establishing more.
- Sibling `../indd-utils` is external integration/corpus data. Its old adapter
  needs migration to the breaking API; keep that checkout unchanged unless the
  user authorizes edits there.
- macOS + installed InDesign: `./scripts/generate-idml-schema/run.sh` exports a
  snapshot without overwriting one. Do not make unpublished snapshots mandatory.
