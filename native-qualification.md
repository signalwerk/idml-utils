# Optional InDesign qualification

The scripts in `scripts/qualify-idml/` are an opt-in experiment, separate from schema validation and ordinary Rust tests. `run.sh` copies an input into a **new** evidence directory, opens only that copy without a window, records selected native readback, exports an IDML and closes only the document it opened without saving. `baseline.sh` creates and exports a separate unsaved one-page native document with a text frame. Both refuse an existing destination, restore the application's interaction setting, retain exit/error logs and hash captured IDML files. The parent of an evidence directory must already exist. If cleanup fails, inspect the reported cleanup errors before starting another experiment; do not close unrelated documents.

```sh
cargo run --locked -p idml --example compose_story -- 8.0 target/native-story.idml
scripts/qualify-idml/run.sh target/native-story.idml target/native-story-evidence
scripts/qualify-idml/baseline.sh target/native-baseline-evidence
cargo run --locked -p idml --example edit_source_text -- input.idml target/noop.idml target/edited.idml
scripts/qualify-idml/run.sh target/noop.idml target/noop-evidence
scripts/qualify-idml/run.sh target/edited.idml target/edited-evidence
```

Run native experiments serially. The harness captures the reported application version, build where available, locale, OS, open/export outcomes, page count, story contents and cleanup status. Captures can contain the original document's text: protect evidence directories accordingly. The harness does not capture a complete plug-in inventory, compare appearance, establish a font environment or demonstrate compatibility with other builds. A native open or re-export is an observation, not proof of rendering equivalence or full semantic preservation. Failures also need controls before identifying a cause.

## Observations on this machine

The following experiments used InDesign 21.5.1.73, `ENGLISH_LOCALE`, on macOS 15.7.4 (64-bit). The app did not expose a build number through the queried property, and the plug-in inventory was not captured. The evidence paths below are local ignored `target/` artifacts, not published fixtures.

- `compose_story` at 8.0 produced input SHA-256 `f03ff0b879088e397dbd5516b3ea8725b2362397ec075255e85ddeca95d99939`. InDesign rejected it at open with error 54 and did not export it (`target/native-qualification-compose-8-escalated/native-result.json`). Interaction settings were restored. The input deliberately contains a story without a complete page layout; the error does not identify which missing or unsupported package detail caused rejection.
- The open corpus fixture `Open Publishing Journal Article Template Academic Style 2.idml`, SHA-256 `788fd8139d3eb76dee83fb63578eb51a4ee7515f08f14c5f369f25e56b33e22b`, was read and written without edits. All 26 non-directory entry contents matched the original exactly. InDesign opened that source no-op and re-exported it (`target/native-qualification-corpus-noop/`); its native readback recorded 7 pages and 9 stories. The test document closed and interaction settings were restored. Neither readback nor export was visually compared to the original.
- A second source copy changed just one `Content` element in `Stories/Story_u120e4.xml`, adding ` [idml-utils native probe]`; the edited IDML SHA-256 was `7604e34a7c545a2fcb7d2a7d8caf9805bff2a75022cd9e0c8bca8a9ece1bf786`. InDesign opened and re-exported it (`target/native-qualification-corpus-edited-v2/`), recording 7 pages and 9 stories. Compared with the no-op native readback, exactly one story changed, and removing that suffix restored its no-op text. The document closed and settings were restored. The current `edit_source_text` example uses the atomic source text replacement API and produced byte-for-byte identical no-op and edited files (`target/native-source-noop-v3.idml` and `target/native-source-edited-v3.idml`) to those used in the native test. This is a single text-edit observation, not a claim about other edits or rendering.
- A new native one-page document with one text frame and `idml-utils native baseline` was created, read back and exported as `target/native-authoring-baseline/baseline.idml` (SHA-256 `701eb3e29bf00885224d2eb5f59b510db0941de1b460d2ec8fcb8035a196eec6`). The captured `DOMVersion` is 21.5. The created document closed and interaction settings were restored. The original test supplied numeric frame bounds, which were interpreted using the application's measurement settings; they are not evidence of intended point coordinates and place the frame beyond the page. The source includes a spread, page and text frame linked by `ParentStory` to a story, together with application-generated resources, master and XML backing story. This single capture does not determine which defaults or package parts are necessary, and it is not a visual comparison.
- A separate corrected baseline requested page dimensions `612pt` × `792pt` and frame bounds `[36pt, 36pt, 200pt, 400pt]`, using explicit unit strings. Its native readback recorded one page and the expected story text; the document closed and settings were restored. The exported `target/native-authoring-baseline-v2/baseline.idml` has SHA-256 `6548d4ad18bae7ddb81b40f4514f325300022844e1a6227415309f137956d72d`. InDesign independently reopened and re-exported this file (`target/native-qualification-baseline-v2/`), again reporting one page and the same story text, with successful cleanup. Native numeric frame-bound readback follows the application's ruler units, not necessarily points; inspect the IDML transforms and bounds before making coordinate claims. No rendering comparison was made.
- The first sandboxed native launcher could not resolve the installed application by bundle ID (`-1728`); the authorized unsandboxed run reached InDesign. A launcher failure is distinct from a document open failure.

## New story and frame composition

The `compose_in_template` example adds a new story and rectangular text frame to
the corrected one-page baseline. Local bounds are `[0,0]` to `[364,164]` points,
with an explicit translation `[-270,-360]` into this baseline's spread, and the
template's explicit active layer. These coordinates are specific to this test.
At the time, the default checked import rejected the unchanged baseline's
Preferences part against the available 21.5 profile, so the native experiment
explicitly used `--source`, preserving the template's 21.5 labels and all source
data. No amendment or missing-field cause was inferred from that diagnostic.
The cause was later identified as a validator defect, not a schema gap: InDesign
writes indented empty elements such as `<TransformParameters>\n</TransformParameters>`,
and whitespace-only content was not matched as empty, contrary to RELAX NG weak
matching. With that fixed, the unchanged baseline validates against the 21.5
effective profile, and the checked path (without `--source`) writes the same
bytes as the qualified input below. Disabling only the fix reproduces the
original Preferences diagnostic.

The initial composed input opened with one page and two stories, but the new
story read back empty (`target/native-qualification-composed-template/`). Adding
UserText or copying full native story metadata did not fix that result. A
consistent native-style ID/filename control also left the new text empty.
One earlier ID-renaming control accidentally changed the reference without
renaming the ZIP entry and is not evidence about ID syntax.

Removing only the redundant `xmlns:idPkg` declaration on the newly inserted
designmap reference made the original minimal producer story import correctly,
including its explicit Br as a carriage return
(`target/native-qualification-minimalnamespace/`). A separate control with a
native story payload showed the same namespace effect. This is an observed
importer behavior on this build, not a restriction on valid XML or source reads.
`Package::add_part` now reuses an existing in-scope idPkg binding when available;
it preserves declarations on existing source nodes.

The corrected library example was regenerated and independently reopened:
`target/native-qualification-composed-template-v2/` records one page, the original
`idml-utils native baseline` story and the new
`New IDML story from the public producer API\r` story, followed by successful
export, close and interaction restoration. This qualifies that source composition
and text readback on this template/build only. Appearance, font substitution,
plug-ins, other transforms, other builds and other document combinations were
not compared.

The corrected input SHA-256 is
`d89d1dc8d4282c7397d2b81609b4362128caf8036ee550b0e3629505b9970d6b`
(produced identically with and without `--source` after the validator fix);
the native re-export SHA-256 is
`4c135ddd72efae424b60b3340ba96e7e0ebf300b7b38e217b8a7c544174958d5`.

```sh
cargo run --locked -p idml --example compose_in_template -- \
  target/native-authoring-baseline-v2/baseline.idml 21.5 \
  Spreads/Spread_ucf.xml ucf -270 -360 target/composed.idml --source
scripts/qualify-idml/run.sh target/composed.idml target/composed-evidence
```
