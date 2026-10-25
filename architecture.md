# Architecture

`idml` is a Rust library for IDML packages whose document model is generated
from the RELAX NG schemas that InDesign generates itself. This document
explains how the schema becomes code, why the result is complete, and the
decisions behind it.

## Pipeline

```text
schemas/<build>/package/*.rnc
  └─ idml-codegen
       rnc.rs     parse the compact syntax (the subset InDesign emits)
       rng.rs     parse the XML syntax (some published schemas exist only so)
       schema.rs  merge the part grammars; check shared definitions agree; reachability
       lower.rs   map every reachable pattern to the model; fail on anything unmapped
       history.rs date every item from all schemas/*/ snapshots → crates/idml/schema-history.tsv
       upgrade.rs check crates/idml/schema-upgrades.tsv against them → src/model/upgrades.rs
       emit.rs    Rust source            → crates/idml/src/model/*.rs
       map.rs     schema item → Rust item → crates/idml/schema-map.tsv
       report.rs  changes from version to version (HTML) → docs/
  └─ idml (runtime)
       model/     generated types (checked in, never edited)
       value.rs   XML Schema datatypes, checked newtypes, enum/range macros
       xml/       reader and writer that generated code drives
       package.rs ZIP container, designmap, conventions outside the schema
       upgrade.rs edits parts of older DOMVersions before they are read
```

The generated files are checked in, so the library builds without the
generator, and diffs show every model change. `cargo test` fails when they
differ from what the generator produces for the schema directory named in
`crates/idml/Cargo.toml` (`[package.metadata.idml] schema`).

## Mapping rules

| Schema | Rust |
|---|---|
| `X_Object`, `X_File` (element definitions) | `struct X` / `struct XFile` |
| `X_EnumValue` | `enum X` with `ALL`, `as_str`, `FromStr` |
| `X_TypeDef` with attributes/elements | `struct X` or `enum X` (choice) |
| `X_TypeDef` that is a value | `type X = …` |
| `attribute N { V }`, `attribute N { V }?` | field `n: V`, `n: Option<V>` |
| `attribute N { "literal" }` (`type`, `DOMVersion`, …) | no field: checked on read, always written |
| child `p`, `p?`, `p*` | `T`, `Option<T>`, `Vec<T>` |
| `p, q` | fields in schema order |
| `p & q` | fields; read in any order |
| interleave with ≥2 repeated members | one `Vec<OwnerChild>` keeping document order (story text!) |
| `(type="a", …) \| (type="b", …)` | `enum` with one variant per branch |
| `EnumA \| EnumB` as a value | `enum AOrB` |
| `xsd:double {minInclusive maxInclusive}` | `Double0To100` etc.: `new()` checks the range |
| `list { T, T }`, `list { T* }` | `[T; 2]`, `Vec<T>` (strings in lists: `Token`) |
| `xsd:string`, `boolean`, `int`, `short`, `double`, `dateTime` | `String`, `bool`, `i32`, `i16`, `f64`, `DateTime` |
| `text` | `String` |
| inline `element E { … }` | collapsed when it carries one kind of data (`<Label>` → `Vec<KeyValuePair>`, `<ListItem type="double">` → `f64`), else a struct/enum named `E` (or `OwnerE` when `E` has different contents in different places) |
| designmap `<idPkg:Spread src=…/>` | `Vec<Part<SpreadFile>>`: the reference *is* the part |

Names follow Rust conventions (`XMLElement` → `XmlElement`, `@Self` →
`self_`). Every type, field and variant documents its schema item; the
complete list is [`schema-map.tsv`](crates/idml/schema-map.tsv).

## Why the model is complete

The claim, for the pinned schema build: every document the package grammars
admit can be represented, and every representable value is written as a
document the grammars admit, with the same information.

1. **Total parsing.** The RNC parser accepts exactly the constructs
   InDesign's schemas use and fails on any other, so a new construct cannot
   be skipped silently. The one tolerated deviation, comment lines without
   `#` that InDesign writes into `datatype.rnc`, is listed in the schema map
   (the published 15.0 and 21.5 copies have them commented out).
2. **Constructive mapping.** Lowering is defined for each RELAX NG
   constructor (element, attribute, group, interleave, choice, optional,
   zero-or-more, list, data with facets, value, text, empty, reference) and
   composes them. Every pattern node it represents is recorded; generation
   fails unless every node of every reachable definition is represented.
   Constructs it cannot represent losslessly (optional literal attributes,
   heterogeneous lists, choices inside sequences, mixed content) stop
   generation instead of being approximated.
3. **Types carry the constraints.** Required, optional and repeated
   content, enumerations, ranges, list arity and token rules are types. A
   value of the model is therefore valid by construction; the writer only
   checks what XML itself cannot carry (U+FFFE and U+FFFF, control
   characters in attributes) and package rules (unique part paths).
4. **Independent oracle.** `idml_codegen::instance` generates random
   schema-valid documents by walking the grammar (not the model). The tests
   read them, write them again and compare the XML structurally; together the
   documents must exercise every attribute, element, choice branch and enum
   literal of the schemas. Writing is also checked to be stable for documents
   using every valid lexical form (`1.0`, ` +7 `, `1` for true) and any
   interleave order.
5. **External validation.** Jing, the reference RELAX NG validator,
   accepted 180 generated documents (all parts, every style) and the
   library's output for each of them, against the 21.5 schemas (of build
   21.5.1.73, which differ from 21.5.0.64 only in comment lines). This
   checks the oracle as well as the writer. It is a one-time check, not part
   of `cargo test`.
6. **Traceability.** `schema-map.tsv` lists each schema item with its Rust
   item and type. It is regenerated with the code, so `git diff` shows
   exactly what a new schema adds, removes or changes.

What the claim does not cover: whether InDesign accepts a schema-valid
document (references between objects, such as `ParentStory`, are plain
strings in the schema and are not checked), and the single-file schema
(`schemas/*/single/`), which describes a flattened form instead of a package.

## Version history

Every type, field and variant says which IDML version introduced it
(`Since IDML 8.0.`); `schema-history.tsv` lists every schema item of every
version with when it appeared, changed and was removed, and
[`docs/`](docs/index.html) shows the same per step from
one version to the next: counts by kind, then each added, removed or changed
item by definition, with its schema before and after and its Rust item.

- **Snapshots.** Every `schemas/<build>/package` (or `/single`) is compared,
  ordered by `DOMVersion`. They are the published InDesign-generated
  schemas found on GitHub, stored as published, with their source in
  `manifest.json`: 6.0 (CS4, single-file form only), 7.0, 7.5, 8.0, 10.0,
  11.0 (the last three in Trang's XML syntax, which `rng.rs` reads), 15.0 and
  21.5.0.64, the newest, which the model is generated from. Builds of one
  `DOMVersion` with identical items count once.
- **Item IDs** come from where the schema text defines an item:
  `TextFrame_Object/@FillColor`, `TextFrame_Object/<Properties>/<Label>`,
  `AnchorPoint_EnumValue="TopLeftAnchor"`, `…/<AppliedFont>|type=object
  xsd:string`. Items are collected per part grammar, because older schemas
  define some names differently in different parts. A fingerprint (an
  attribute's type and presence, a child's occurrence) detects changes.
- **Mapping to Rust.** Lowering records the pattern nodes behind every type,
  field and variant, including each further occurrence of a shared inline
  element, so an item is dated by its earliest occurrence. Generation fails
  if a model item has no history.
- **The single-file form** says nothing about the designmap and the part
  roots, which it lacks, or about its XMP wildcard (`anything`). For
  everything else it lists exactly the items of the package form; this was
  checked with build 21.5.1.73, for which `scripts/generate-idml-schema`
  generated both forms.

A version is exact where the snapshot before it is the previous version
(`7.5`). No schemas were found for 9.0, 12.0 to 14.0 or 16.0 to 21.0, so
there a version is a range: `> 11.0 <= 15.0` means after 11.0, by 15.0. The
same holds for changes and removals. `<= 7.0` marks items of the designmap
and part roots, which the single-file 6.0 schema cannot date. Adding a snapshot
(generate one with an installed InDesign, or copy a published one with a
`manifest.json`) refines the history without code changes. A renamed item
appears as removed and added; numbered variant definitions
(`XMLInstruction_Object1`) are compared by name.

## Reading older versions

The model is the current schema only; adapters see one model. Older
documents are upgraded while they are read, and written as the current
version.

- **Rules.** `crates/idml/schema-upgrades.tsv` (hand-written) has a rule for
  every schema item that an older snapshot has and the current one has not,
  or has differently: `drop` (with a note saying why; the reader warns),
  `wrap <A>` (the element is allowed only inside `<A>` now), `default "v"`
  (an attribute became required), `value "v"` (an enumeration value is
  gone) or `keep` (old values stay valid, such as a wider range).
  `idml-codegen` fails if a rule is missing (and prints a suggestion) or is
  not needed, and checks that a `wrap` target is allowed now.
- **Edits.** The generator turns the rules into edits by element path and
  `DOMVersion` range (`src/model/upgrades.rs`). The reader applies them to
  the text of an older part, rewrites its `DOMVersion` and then reads it like
  a current one, strictly by default. Line numbers stay, so error locations
  point into the original.
- **Proof.** Tests generate documents from each older snapshot's grammars
  (6.0: the 7.0 part roots around the 6.0 definitions) until every item of
  them is exercised, and read them strictly with the current model; the only
  warnings allowed are drops. Without the rules, they fail.
- **Move hints.** For a child that is no longer allowed in its parent, the
  history looks for it nested in elements without an identity (no `@Self`):
  a 6.0 `<Story>/<TextFrame>` is now `<Story>/<CharacterStyleRange>/<TextFrame>`.
  The reports and `schema-history.tsv` show these as moved, and missing
  rules are suggested as `wrap`.

The rules and their reasons are listed in
[`docs/upgrades.html`](docs/upgrades.html) and next to
each change in the reports.

## Decisions

| Decision | Reason |
|---|---|
| Generated typed structs, not typed views over an XML tree | Invalid states are unrepresentable; adapters match exhaustively, so a new enum value or field is a compile-time change for them. |
| Values compared in their value space | `1.0` and `1` are the same `xsd:double`; the writer uses one canonical form (shortest round-trip decimal, `INF`, `NaN`, `true`). `DateTime` keeps its lexical form. |
| Conservative datatypes | Where XML Schema versions or validators disagree, only the common subset is accepted: `24:00:00` (allowed by an XSD 1.0 erratum, rejected by Jing) is not a valid `DateTime`, `+INF` is not a valid double. |
| Interleaved singletons written in schema order | Their order carries no information; repeated members keep document order, which matters for text. |
| `required` fields come from `new()`, not `Default` | A placeholder value for required data would be silently wrong. Types without required data implement `Default`. |
| Strict by default, lenient on request | Strict reading rejects anything outside the schema with a location. Lenient reading skips unknown attributes and elements, invalid optional values and other `DOMVersion`s, reporting each as a warning (the first 1000 in detail), for documents from other InDesign versions. |
| No preservation of unknown XML | Kept content could make output invalid. Unreferenced ZIP entries and `META-INF/metadata.xml` are kept verbatim. Comments and processing instructions outside text are dropped. |
| `<?ACE n?>` in text ↔ control character | IDML encodes characters XML 1.0 cannot contain (U+0000–U+001F except tab, LF, CR) as processing instructions; the model stores the character. |
| Reading in place | Generated structs are large (hundreds of optional attributes). Children are read into their final heap location, so recursion uses about 1 KiB of stack per level in release builds. `ReadOptions::max_depth` (default 48) is enforced before parsing, as the XML parser recurses too. |
| No panics, no `unsafe`, no I/O, threads or clocks | Matches the consumers' rules (DesignCraft's never-crash standard, WASM builds). |
| Dependencies: `roxmltree`, `zip` | Maintained, pure Rust, WASM-capable. The writer is our own: it is small and controls escaping and indentation. |

## Updating to a new InDesign version

1. Generate its schemas: `./scripts/generate-idml-schema/run.sh` (macOS, InDesign).
2. Point `[package.metadata.idml] schema` in `crates/idml/Cargo.toml` at the
   new `schemas/<build>/package`.
3. `cargo run -p idml-codegen`. If it fails, the schema uses a construct the
   generator does not represent yet; extend `lower.rs`, never skip it.
4. If it lists items without an upgrade rule, decide how documents of the
   previous versions are read and add the rules to `schema-upgrades.tsv`
   (the suggestions are a start; a `drop` needs a reason).
5. Review `git diff crates/idml/schema-map.tsv crates/idml/schema-history.tsv`:
   added, removed and changed items, with their Rust representation and the
   version. The new snapshot joins the history by being in `schemas/`; its
   step is the last page of `docs/`.
6. `cargo test`. Adapters then fail to compile where they match on changed
   enums or build changed structs.

The model targets one schema build at a time. Older documents are upgraded
while reading; newer ones can be read in lenient mode. Writing always
produces the current `DOMVersion`.

## Adapters

The library owns IDML; adapters own their source or editor models:

- **indd-utils** builds a `Package` from its decoded INDD model instead of
  writing XML itself, keeping its INDD knowledge (attribute IDs, defaults).
- **DesignCraft** maps `Package` to and from its editor model. To keep IDML
  it does not model, it can retain the read `Package` and patch it on export.
- **QuarkXPress** and other converters build a `Package` directly.
