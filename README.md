# idml-utils

A Rust library for Adobe InDesign IDML whose document model is generated from
the RELAX NG schemas InDesign generates. Every element, attribute and value
the schemas allow has a Rust type, and only schema-valid values can be
represented. Documents of older versions (6.0 and later) are upgraded while
reading.

| Path                              | Content                                                                                                     |
| --------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| `crates/idml`                     | The library: generated model (`src/model/`), XML and package I/O                                            |
| `crates/idml/schema-map.tsv`      | Every schema item, the Rust item representing it, and the IDML version that introduced it                   |
| `crates/idml/schema-history.tsv`  | Every schema item of every version: introduced, moved, changed, removed                                     |
| `crates/idml/schema-upgrades.tsv` | How documents of older versions are read: a rule for every schema difference                                |
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
```

Existing schema files are checked and kept. After the schemas and Cargo
dependencies are downloaded, tests can run offline:

```sh
cargo test --workspace --locked --offline
```

Tests also check that the generated Rust model, schema maps and HTML reports
are up to date. A missing-schema error means the download step is needed.
See [schemas/README.md](schemas/README.md) for sources and versions.

## Use the library

```rust
let mut package = idml::Package::read(&bytes, &idml::ReadOptions::default())?.value;
for part in &mut package.document.story {
    for story in &mut part.content.story {
        story.story_title = Some("Edited".into());
    }
}
let bytes = package.write()?;
```

## Regenerate the model and reports

After downloading the schemas, run the generator from the repository root:

```sh
cargo run --locked -p idml-codegen
```

It reads the schema and history paths in `[package.metadata.idml]` in
`crates/idml/Cargo.toml`, then updates the generated model, TSV maps and HTML
reports. To check them without writing files:

```sh
cargo run --locked -p idml-codegen -- --check
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
