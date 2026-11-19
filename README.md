# idml-utils

A version-neutral Rust library for reading, authoring and writing Adobe InDesign
IDML. `Package` stores source XML in a lossless, iterative arena. New documents
require an explicit version; reading and writing never upgrade to the newest.
Unknown extensions, values, comments, instructions and per-part versions survive.

Generated `views` and `Builder<T>` share a typed vocabulary across the available
schema snapshots. `Authoring` validates staged edits against an exact chosen
profile, reports compatible targets, and lets you explicitly upgrade when a new
feature is needed. Conversion forbids data and semantic loss by default.

`SchemaCatalog` identifies raw/effective profiles by DOMVersion, build, form,
locale, plug-in inventory and input hashes. Compatibility is checked independently
per snapshot; missing releases/configurations stay unknown. Source preservation
works without a matching schema. Validation does not establish InDesign rendering
or semantic cross-reference validity.

The opt-in `owned` feature exposes `OwnedPackage`, `model`, `PartFile` and
`XmlPart<T>` for adapters needing an owned projection into one configured schema.
That adapter is separate from universal authoring. See [architecture.md](architecture.md)
for the decision review and [schema-investigation.md](schema-investigation.md) for
native-export evidence.

| Path                              | Content                                                                                                     |
| --------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| `crates/idml`                     | The library: generated model (`src/model/`), XML and package I/O                                            |
| `crates/idml/schema-map.tsv`      | Every schema item, the Rust item representing it, and the IDML version that introduced it                   |
| `crates/idml/schema-history.tsv`  | Every schema item of every version: introduced, moved, changed, removed                                     |
| `crates/idml/schema-upgrades.tsv` | How older documents project into the optional owned model: a rule for every schema difference                                |
| `crates/idml/schema-migrations.tsv` | Reviewed source migrations with explicit endpoints, loss impact and evidence |
| `crates/idml/schema-amendments.tsv` | Version-scoped corrections with evidence; separate from Adobe’s original snapshots and history        |
| `docs/`                          | Generated GitHub Pages site: schema changes and upgrade reports (open `docs/index.html`)                    |
| `crates/idml-codegen`             | Schema → model generator, and the test oracle                                                               |
| `schemas/`                        | Download script, checksums and documentation; downloaded schema folders are local and ignored by Git        |
| `scripts/generate-idml-schema/`   | Generates schemas for the installed InDesign (macOS)                                                        |
| `architecture.md`                | Design, mapping rules, completeness argument                                                                |

## Build

Run the commands below from the repository root. Install Rust and Cargo
(Rust 1.88 or newer, edition 2024). The generated Rust model is checked in,
so building the library requires neither schema downloads nor InDesign:

```sh
cargo build --workspace --locked
```

For an optimized build, use `cargo build --workspace --release --locked`.
This project provides a library and a schema generator; it has no document
converter CLI.

## Download schemas and run tests

Tests and model generation need the schema snapshots. Their version folders
under `schemas/` are ignored by Git and must be restored after cloning.
The download script needs Bash, `curl`, and either `sha256sum` or `shasum`.
It downloads published schemas from pinned GitHub commits, checks their
SHA-256 hashes, and writes their source manifests. InDesign is not required.

```sh
./schemas/download.sh
./schemas/download.sh --verify
cargo test --workspace --locked
cargo test --workspace --all-features --locked
```

Existing schema files are checked and kept. After the schemas and Cargo
dependencies are downloaded, tests can run offline:

```sh
cargo test --workspace --locked --offline
```

Tests also check that the generated Rust model, schema maps and HTML reports
are up to date. A missing-schema error means the download step is needed.
See [schemas/README.md](schemas/README.md) for sources and versions.

## Start at a chosen version

```rust
let package = idml::Package::new("8.0".parse()?, "document")?;
let bytes = package.write()?; // All new parts inherit 8.0 unless explicitly versioned.
```

For checked authoring, resolve an exact profile and keep it with the document:

```rust
use idml::{Authoring, LossPolicy};
use idml::profile::SchemaCatalog;
let catalog = SchemaCatalog::builtin()?;
let profile_8 = catalog.effective_profile("8.0".parse()?)?;
let mut document = Authoring::new(catalog, profile_8, "document")?;
let draft = document.plan_edit(|package| {
    // Add parts with package.add_part(...), or edit through generated views.
    // A new feature may make draft.can_apply() false at 8.0.
    Ok(())
})?;
let candidates = draft.compatible_upgrades()?;
// Select a profile explicitly; this example keeps 8.0.
let plan = draft.for_target(profile_8, LossPolicy::Forbid)?;
plan.apply(&mut document)?;
let bytes = document.write()?;
```

`draft.validation()` explains failures against the committed target.
`draft.upgrade_options(Forbid)` includes validation and rewrite reports for each
available same-version/newer target. `draft.compatibility()` checks unchanged
content independently against each snapshot. Neither method guesses support in
a missing release. A failed/stale plan leaves the committed document unchanged.
`Authoring::for_version(catalog, version, id)` is a shorter way to start at an
exact effective package profile. Both it and `effective_profile` report unknown
or ambiguous versions instead of guessing; use `resolve` to inspect candidates
and explicitly choose the right build when several profiles share a DOMVersion.

The runnable example starts at **8.0**, stages a **ColorGroup**, reports compatible
snapshots and explicitly chooses **10.0**, without jumping to 21.5:

```sh
cargo run --locked -p idml --example versioned_authoring
```

## Read and edit source data

For document-level inspection, `package.source_document()` borrows stories and
text containers from the original XML. `Story::content()` yields text, explicit
breaks and opaque nodes; tables, footnotes, extensions and unfamiliar instructions
are kept available as source fragments. Values retain their absent/known/
unrecognized state. The facade supplies no implicit styles, units or defaults.
`package.semantic_graph()` separately inventories identities and a closed,
documented catalog of references (`semantic::ReferenceKind`): story/frame links,
applied paragraph/character/object/table/cell styles, item layers, applied
masters, fill/stroke swatches and hyperlink sources. Each kind lists its owner
elements, expected target types and whether `n` is a nil marker, as observed in
the local corpus. `Link` URIs are recorded as external resources, never resolved.
`text_threads()` (or `source_document().text_flows()` for borrowed container views)
orders frames by their links and reports non-reciprocal links, joins, cycles,
mixed stories and stories split across threads. Reciprocal previous/next pairs are
legitimate reference cycles. These findings do not decide grammar validity or
whether InDesign can render the document.

For newly authored stories, `compose` provides a small producer API that also
registers the part reference and StoryList identity atomically:

```rust
use idml::compose::{Paragraph, StoryBuilder, TextRun};
let draft = document.plan_edit(|package| {
    let run = TextRun::new("Hello\tworld")
        .format(|attributes| attributes.set_point_size(&18.0))?
        .with_break();
    let story = StoryBuilder::new("imported-story-1")?
        .paragraph(Paragraph::new().run(run));
    package.add_story("Stories/imported.xml", story, &idml::ReadOptions::source())?;
    Ok(())
})?;
// Inspect validation and explicitly retarget if needed, then apply.
draft.apply(&mut document)?;
```

These producers create new content without projecting an existing story. Styling
uses the generated setters, and the authoring plan checks its exact target.
Breaks and formatting are explicit; the producer does not normalize newlines or
invent page geometry, fonts or style definitions. See the `compose_story` and
`semantic_inspection` examples. Future format importers can use this layer and
the generated part builders while keeping source-format conversions explicit.

`TextFrameBuilder::rectangle` and `geometry::{Rect, Transform}` create an
unthreaded frame with explicit local point coordinates and a parent transform.
`add_text_frame` checks the source story and spread identities and commits the
frame atomically; it does not guess page membership. The `compose_in_template`
example combines new stories and frames in a caller-provided complete template,
checked against an explicit exact profile. A story-only package is a useful
schema example and does not constitute a complete native page layout.

To edit existing simple Content nodes, turn `StoryEvent::Text` selections into
`TextReplacement`s with `event.replace_text(...)`, then call
`package.replace_text(&replacements)` within a draft. A batch checks every source
identity/revision before committing, supports multiple selections in one part,
and rejects opaque or unrecognized content. Raw source edits can be measured
with `package.check_limits(&options)`; parsing limits are not permanent bounds
attached to a mutable package.

For a future Quark importer, choose an exact destination profile first, then
map identities, text runs, styles and coordinate spaces explicitly. Stage the
new IDML parts in one authoring draft and inspect its target report before
applying. Keep source-format projection losses in the importer's own report;
the IDML conversion report describes changes between IDML targets. Preserve
unmapped source data as attachments only with an explicit policy and clear
provenance. The generated vocabulary remains available for features outside the
small convenience producers. Use controlled native readbacks to qualify each
new mapping and feature combination.

```rust
let mut package = idml::Package::read(&bytes, &idml::ReadOptions::source())?.value;
let content = package.parts().find_map(|(path, tree)| {
    tree.views::<idml::views::Content>().next()
        .map(|view| (path.to_owned(), view.node_id()))
});
if let Some((path, node)) = content {
    if let Some(tree) = package.part_mut(&path) {
        if let Some(mut view) = tree.view_mut::<idml::views::Content>(node) {
            view.set_text("Edited")?;
        }
    }
}
let output = package.write()?;
```

Use `views::Story`, `views::CharacterStyleRange`, etc. for named field access.
`Field::Unrecognized` retains its raw lexical value. Named setters check the
shared vocabulary without inferring a profile or changing a version. For a
field-level capability check, supply an exact catalog/profile to `set_checked`.
Use Authoring or `Package::write_checked` when output must satisfy a target.
Raw edits explicitly retain extension data. Nodes keep their IDs and lexical
form through structural edits, including namespace context. The default depth
limit is 4096; arena parsing/writing/clone/drop do not recurse with document depth.

Schema validation has a separate finite work budget and grammar-operation depth
limit. Exhausting either yields an `Unknown` validation report with a `Limit`
diagnostic, not a schema rejection. `Package::validate_with_options` shares its
budget across all parts; `SchemaCatalog::validate_with_options` checks one tree.
For unusually complex trusted documents, set `profile::ValidationOptions` and use
`write_checked_with_options`, `plan_conversion_with_options`, or construct an
authoring session with `Authoring::from_package_with_options` /
`new_with_options`. The session carries the same limits through draft validation,
target planning and checked output. Each candidate in a migration search has a
fresh budget; its search is also bounded. Work units are implementation-specific,
not a duration or a rendering guarantee.

For explicit conversion:

```rust
use idml::profile::{SchemaCatalog, ProfileResolution};
let catalog = SchemaCatalog::builtin()?;
let target = match catalog.resolve("21.5".parse()?, true) {
    ProfileResolution::Exact(target) => target,
    other => return Err(format!("Choose an exact profile: {other:?}").into()),
};
let plan = package.plan_conversion(catalog, target, idml::LossPolicy::Forbid)?;
// Review plan.changes() and plan.validation() before applying.
if plan.can_apply() {
    let changes = plan.apply(&mut package)?;
}
```

A blocked plan leaves the source untouched. `LossPolicy::Allow` explicitly
permits deletions, reports original fragments and locations, and still requires
target validation. Missing snapshots, incomplete conversion rules or required
invalid data can block a conversion; changing a version label is not a fallback.
Reviewed source migrations use explicit directed endpoints and evidence in
`schema-migrations.tsv`; they are independent of the configured owned model.
A merely similar enum replacement counts as semantic loss. Custom catalogs and
`MigrationRegistry` rules extend validation and conversion independently of the
library's document types. No reverse or missing-release rewrites are invented.
The bounded search proposes one available candidate, not an exhaustive list of
all routes or possible losses; generic removals are considered in one pass.

To review each loss separately, build a proposal with
`package.plan_conversion_for_review(catalog, target)` or retarget an authoring
draft with `draft.for_target_for_review(target)`. These include available
optional removals and start with every loss unapproved. Inspect `losses()` for
each proposal-scoped ID and its change: the original and replacement,
location, `loss_impact()`, and, for reviewed migrations, rule and evidence.
Call `set_loss_approval(&id, true)` for each accepted loss; pass `false` to
revoke an approval. For example:

```rust
let mut plan = package.plan_conversion_for_review(catalog, target)?;
// Example policy: approve only this particular obsolete extension attribute.
let accepted: Vec<_> = plan.losses()
    .filter(|(_, change)| change.attribute.as_deref() == Some("ObsoleteExtension"))
    .map(|(id, _)| id)
    .collect();
for id in accepted {
    plan.set_loss_approval(&id, true)?;
}
if plan.can_apply() {
    plan.apply(&mut package)?;
}
```

Alternatively, `decide_losses` asks a callback about every loss in the
proposal and replaces earlier decisions, including a bulk `Allow`; for example,
approve semantic substitutions (`LossImpact::SemanticChange`) but refuse data
removal. `refused_losses()` lists what still blocks the proposal.

```rust
use idml::{LossDecision, package::LossImpact};
plan.decide_losses(|change| match change.loss_impact() {
    Some(LossImpact::SemanticChange) => LossDecision::Approve,
    _ => LossDecision::Refuse,
});
```

IDs and callback decisions belong to one proposal (and its clones), so
retargeting or replanning requires a fresh review. Approval never bypasses exact target validation,
checked package-envelope requirements, or stale-source protection. A proposal
with refused loss stays unapplied; its preview may still show those proposed
changes so the complete candidate can be reviewed.

For the optional owned projection, enable `features = ["owned"]` and use
`OwnedPackage::read` / `OwnedPackage::from_elements`. Its new content targets
`model::DOM_VERSION` and its recursive reader caps depth at 48. Its schema-upgrade
rules do not drive universal source conversion. The old unchecked
`Layout::dom_version` serialization override has been removed.

```sh
cargo run --locked -p idml --example profiles
cargo run --release --locked -p idml --example corpus -- --source --roundtrip /path/to/corpus
cargo run --release --locked -p idml --example corpus -- --validate --semantic /path/to/corpus
cargo run --release --locked -p idml --features owned --example corpus -- --owned --roundtrip /path/to/corpus
```

Corpus mode defaults to source parsing and entry preservation; `--owned`
also exercises strict/lenient owned projections. Both write TSV details to stdout
and grouped counts to stderr. The TSV includes read warnings, representation,
strict/lenient mode, part, element path and line/column (zero when unknown), so
projection omissions are visible alongside envelope warnings. Owned mode uses
the configured projection schema; it does not select an exact profile for the
source. Source strictness concerns package/XML structure, not schema acceptance.
In source mode, `--validate` resolves each designmap `DOMVersion` to an exact
effective profile (reporting unlabelled, ambiguous and unobserved versions
without choosing one) and validates against it; `--semantic` adds duplicate,
non-resolved reference and thread rows plus grouped reference/link counts.

The ZIP reader rejects archives that readers could interpret differently: a
local header whose name, method, or (without a data descriptor) CRC-32/sizes
disagree with the central directory, entries sharing bytes, disagreeing entry
counts or central-directory size, and an end record whose comment does not end
the input.

`ReadOptions` bounds expanded ZIP size, entry size and entry count, as well as
XML depth, node count and attribute count per part. `Package::check_limits`
checks a modified package before writing, including total entry sizes. Edited XML
is serialized in memory to measure it, without ZIP compression. The limits are
not a bound on the caller's input buffer or on schema-validation work. For owned
producer construction, limits cover the input elements and envelope; the typed
projection may generate different output.

`ReadOptions` also bounds ZIP entry count and per-entry expanded size, and XML
node/attribute counts per part. The source and owned readers enforce these
before constructing XML arenas/DOMs. Producer entry limits are checked before
parts are committed. These bounds are distinct from a total process-memory or
schema-validation work budget; use `..ReadOptions::source()` when specifying
individual limits.

The `evidence` module records operation-specific observations with exact scope,
fixture digests, report references and limitations. Proposed/refuted observations
remain inspectable; missing dimensions, missing fixture provenance and conflicting
observations produce an unknown assessment. Native reopen, readback and rendering
are separate operations. There are no built-in native support claims. The optional
`scripts/qualify-idml/run.sh` captures a native reopen/export in a fresh evidence
directory; see [native-qualification.md](native-qualification.md) for its scope.

## Regenerate the model and reports

After downloading the schemas, run the generator from the repository root:

```sh
cargo run --locked -p idml-codegen
```

Full generation reads the schema/history paths in `[package.metadata.idml]`,
then updates the optional owned model, union views, catalogs, migration data,
TSV maps and HTML reports. Check without writing:

```sh
cargo run --locked -p idml-codegen -- --check
```

Generate only the universal runtime artifacts, independently of owned-model
lowering and without changing the projection/reports:

```sh
cargo run --locked -p idml-codegen -- --universal
cargo run --locked -p idml-codegen -- --universal --check
```

Export standalone RELAX NG XML grammars for external validation:

```sh
cargo run --locked -p idml-codegen -- --export-schemas target/effective-schemas
cargo run --locked -p idml-codegen -- --export-schemas target/raw-schemas --raw
java -jar /path/to/jing.jar target/effective-schemas/designmap.rng designmap.xml
```

Export a portable catalog (load with `SchemaCatalog::from_compressed`):

```sh
cargo run --locked -p idml-codegen -- --export-catalog target/idml-profiles.bin
```

Snapshots may include a `profile.tsv` beside their `package/` directory. Rows
are `locale<TAB>de_CH`, `plugin<TAB>Name<TAB>Version`, and
`plugins_complete<TAB>true|false`. Omitted inventory is unknown. Locale,
structured inventory, completeness and corrections affect the profile identity;
the upstream schema hash stays separate. Publishing new mandatory snapshots
still requires pinned download sources and checksums.

Raw exports reproduce Adobe's grammar; effective exports include the corrections.
To also run Jing over generated packages with recorded layouts and typed edits:

```sh
IDML_JING_JAR=/path/to/jing.jar cargo test --locked -p idml --features owned --test roundtrip jing_validates_written_packages_with_layouts
```

## Preview and publish the documentation

The generator writes the complete static site to `docs/`, starting with
`docs/index.html`. Open that file in a browser, or serve the site locally
with Python 3:

```sh
python3 -m http.server 8000 --directory docs
# open http://localhost:8000/
```

Commit the generated `docs/` files along with generator changes. In the
repository's **Settings → Pages**, select **Deploy from a branch**, choose
the branch containing these files and the **/docs** folder, then save.
See [GitHub's publishing source instructions](https://docs.github.com/en/pages/getting-started-with-github-pages/configuring-a-publishing-source-for-your-github-pages-site).

The generated `.nojekyll` file lets GitHub serve the HTML and CSS directly.
All site links are relative, so they work under the repository's Pages path.
`docs/` contains only generated files; regeneration removes stale files there.
The [architecture notes](architecture.md) live outside the published site.

## Update to a new InDesign version

Generating a new schema snapshot requires macOS and an installed Adobe
InDesign. Download the existing snapshots first so the generator has the
full version history.

```sh
./scripts/generate-idml-schema/run.sh   # writes schemas/<build>/
# set [package.metadata.idml] schema in crates/idml/Cargo.toml
cargo run --locked -p idml-codegen      # regenerates everything; lists missing schema-upgrades.tsv rules
git diff crates/idml/schema-map.tsv     # what changed, and how it is represented
cargo test --workspace --locked
```

Resolve any missing rules in `crates/idml/schema-upgrades.tsv` and rerun the
generator. New schema folders stay local and ignored by Git. To make a new
snapshot reproducible for other contributors, add a published source to
`schemas/download.sh` and its file hashes to `schemas/SHA256SUMS`.

## License

This repository's original code is dual-licensed under [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option, matching DesignCraft.
Copyright (c) 2026 signalwerk and the idml-utils contributors.

Third-party schemas retain their respective licenses.
