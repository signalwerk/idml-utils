//! Dating model items from schema snapshots.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::PathBuf;

use idml_codegen::history::{History, annotate, snapshots};
use idml_codegen::model::TypeKind;
use idml_codegen::{lower, report, schema::Schema, upgrade};

/// A part grammar for one version. `A` has had `<Props>` since 7.0; `B`,
/// which comes first and so is lowered first, has the same `<Props>` but
/// only since 8.0.
fn grammar(dom: &str, with_b: bool) -> String {
    let b = if with_b { "B_Object*," } else { "" };
    let b_def = if with_b { "B_Object = element B { element Props { attribute X { text }? }? }" } else { "" };
    format!(
        "# Adobe InDesign Test {dom}.0.1\nnamespace idPkg=\"http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging\"\ngrammar {{\n\
         start = Root_File\n\
         Root_File = element idPkg:Root {{ attribute DOMVersion {{ \"{dom}\" }}, {b} A_Object* }}\n\
         A_Object = element A {{ element Props {{ attribute X {{ text }}? }}? }}\n{b_def}\n}}\n"
    )
}

#[test]
fn shared_inline_types_take_their_earliest_occurrence() {
    let root: PathBuf = std::env::temp_dir().join(format!("idml-history-{}", std::process::id()));
    for (build, dom, with_b) in [("7.0.0.1", "7.0", false), ("8.0.0.1", "8.0", true)] {
        let dir = root.join(build).join("package");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Root.rnc"), grammar(dom, with_b)).unwrap();
    }
    let schema = Schema::load(&root.join("8.0.0.1/package")).unwrap();
    let mut model = lower::lower(&schema).unwrap();
    let history = History::new(snapshots(&root).unwrap());
    annotate(&mut model, &schema, &history).unwrap();
    fs::remove_dir_all(&root).unwrap();

    let ty = |name: &str| model.types.iter().find(|t| t.name == name).unwrap();
    assert_eq!(ty("B").trace.since.as_deref(), Some("Since IDML 8.0."));
    let props = ty("Props");
    assert_eq!(props.trace.items, ["A_Object/<Props>", "B_Object/<Props>"]);
    assert_eq!(props.trace.since.as_deref(), Some("Since IDML <= 7.0."));
    let TypeKind::Struct(c) = &props.kind else { panic!("Props is a struct") };
    assert_eq!(c.attrs[0].trace.since.as_deref(), Some("Since IDML <= 7.0."));

    // Nothing of 7.0 is gone or different in 8.0: no upgrade rules needed.
    let upgrades = upgrade::check(&history, Vec::new()).unwrap();
    let pages = report::html(&history, &model, &upgrades);
    assert_eq!(pages.keys().map(String::as_str).collect::<Vec<_>>(), ["7.0-to-8.0.html", "index.html", "style.css", "upgrades.html"]);
    let page = &pages["7.0-to-8.0.html"];
    assert!(page.contains("<code>B_Object</code> <span class=\"badge added\">added</span> <span class=\"rust\">B</span>"), "{page}");
    assert!(page.contains("<td><code>&lt;B&gt;</code></td><td><span>zero or more</span></td><td><code>RootFile.<wbr>b</code></td>"), "{page}");
    assert!(!page.contains("A_Object"), "{page}");
}
