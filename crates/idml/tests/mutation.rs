//! Deterministic mutation tests for malformed XML, ZIP metadata and namespace
//! edits. Rejected input must fail with an error, not a panic. Accepted input
//! must keep every entry's contents, agree with an independent ZIP reader, and
//! failed edits must leave the tree unchanged.
#![allow(clippy::unwrap_used, clippy::panic)]
mod common;

use idml::{Element, NodeId, Package, ReadOptions, XmlTree};
use std::collections::BTreeMap;
use std::io::{Cursor, Read};

/// xorshift64*: small, deterministic and dependency-free.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const NS: &str = "xmlns:idPkg='http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging'";

fn files() -> Vec<(String, String)> {
    let designmap = format!(
        "<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\n<?aid style='50'?>\n<Document {NS} DOMVersion='21.5' Self='d' StoryList='s'><idPkg:Story src='Stories/s.xml'/><idPkg:Spread src='Spreads/p.xml'/></Document>"
    );
    let story = format!(
        "<idPkg:Story {NS} xmlns:p='urn:plugin' DOMVersion='21.5'><Story Self='s'><ParagraphStyleRange AppliedParagraphStyle='ParagraphStyle/x'><CharacterStyleRange p:id='1'><Content>a &amp; b<?ACE 7?></Content><Br/><p:Note><![CDATA[x<y]]></p:Note><!-- c --></CharacterStyleRange></ParagraphStyleRange></Story></idPkg:Story>"
    );
    let spread = format!(
        "<idPkg:Spread {NS} DOMVersion='21.5'><Spread Self='sp'><TextFrame Self='f1' ParentStory='s' PreviousTextFrame='n' NextTextFrame='f2'/><TextFrame Self='f2' ParentStory='s' PreviousTextFrame='f1' NextTextFrame='n'><Properties><Label><KeyValuePair Key='k' Value='&#xD;'/></Label></Properties></TextFrame></Spread></idPkg:Spread>"
    );
    vec![
        ("mimetype".into(), idml::MIMETYPE.into()),
        ("META-INF/container.xml".into(), common::CONTAINER.into()),
        ("designmap.xml".into(), designmap),
        ("Stories/s.xml".into(), story),
        ("Spreads/p.xml".into(), spread),
    ]
}

const TOKENS: &[&str] = &[
    "<",
    ">",
    "&",
    "&amp;",
    "&#x0;",
    "&#xD800;",
    "&#x1F600;",
    "\"",
    "'",
    "=",
    ":",
    "/",
    " xmlns:p='urn:other'",
    " xmlns=''",
    " xmlns:q=''",
    " p:x='1'",
    " q:y='2'",
    "<?ACE 7?>",
    "<?ACE zz?>",
    "<![CDATA[",
    "]]>",
    "<!--",
    "-->",
    "<!DOCTYPE x [<!ENTITY e 'v'>]>",
    "&e;",
    "\u{0}",
    "é",
    "\u{FEFF}",
    " Self='s'",
    " NextTextFrame='f1'",
];

fn mutate(rng: &mut Rng, text: &str) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    for _ in 0..1 + rng.below(3) {
        let at = rng.below(chars.len() + 1);
        match rng.below(4) {
            0 => {
                let end = (at + 1 + rng.below(4)).min(chars.len());
                chars.drain(at.min(end)..end);
            }
            1 => {
                let token = TOKENS[rng.below(TOKENS.len())];
                chars.splice(at..at, token.chars());
            }
            2 => {
                let end = (at + 1 + rng.below(12)).min(chars.len());
                let copy = chars[at.min(end)..end].to_vec();
                chars.splice(at..at, copy);
            }
            _ if chars.len() > 1 => {
                let (first, other) = (at.min(chars.len() - 1), rng.below(chars.len()));
                chars.swap(first, other);
            }
            _ => {}
        }
    }
    chars.into_iter().collect()
}

/// Every non-directory entry, read by the independent `zip` crate.
fn entries(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, Box<dyn std::error::Error>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut out = BTreeMap::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        out.insert(entry.name()?.into_owned(), data);
    }
    Ok(out)
}

#[test]
fn malformed_parts_fail_cleanly_or_keep_every_entry() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut accepted = 0;
    for _ in 0..2000 {
        let mut input = files();
        let part = 2 + rng.below(3);
        input[part].1 = mutate(&mut rng, &input[part].1);
        let bytes = common::zip(&input);
        let Ok(parsed) = Package::read(&bytes, &ReadOptions::source()) else { continue };
        accepted += 1;
        let package = parsed.value;
        // Read-only views over unusual but well-formed input never panic.
        let graph = package.semantic_graph();
        let _ = graph.text_threads();
        for story in package.source_document().stories() {
            story.content().for_each(drop);
        }
        let _ = package.source_document().text_flows();
        let written = package.write().unwrap_or_else(|e| panic!("{e}: {}", input[part].1));
        assert_eq!(entries(&written).unwrap(), entries(&bytes).unwrap(), "{}", input[part].1);
    }
    // Some mutations keep the input well-formed; the property must be exercised.
    assert!(accepted > 200, "{accepted}");
}

#[test]
fn mutated_archives_are_rejected_or_read_like_an_independent_reader() {
    let original = common::zip(&files());
    // Bias flips toward ZIP metadata: local headers and the central directory
    // with its end record. Entry data is protected by CRC-32 in both readers.
    let central = u32::from_le_bytes(original[original.len() - 6..original.len() - 2].try_into().unwrap()) as usize;
    let mut metadata: Vec<usize> = (central..original.len()).collect();
    metadata.extend(original.windows(4).enumerate().filter(|(_, w)| *w == b"PK\x03\x04").flat_map(|(i, _)| i..i + 30));
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let (mut both, mut ours_only) = (0, 0);
    for _ in 0..4000 {
        let mut bytes = original.clone();
        for _ in 0..1 + rng.below(2) {
            let at = if rng.below(4) == 0 { rng.below(bytes.len()) } else { metadata[rng.below(metadata.len())] };
            bytes[at] ^= 1 << rng.below(8);
        }
        let ours = Package::read(&bytes, &ReadOptions::source());
        let theirs = entries(&bytes);
        match (ours, theirs) {
            (Ok(ours), Ok(theirs)) => {
                both += 1;
                let changed: Vec<_> = (0..bytes.len()).filter(|i| bytes[*i] != original[*i]).map(|i| (i, original.len() - i)).collect();
                assert_eq!(entries(&ours.value.write().unwrap()).unwrap(), theirs, "changed (offset, from end): {changed:?}");
            }
            (Ok(_), Err(e)) => {
                ours_only += 1;
                let changed: Vec<_> = (0..bytes.len()).filter(|i| bytes[*i] != original[*i]).map(|i| (i, original.len() - i)).collect();
                eprintln!("accepted only by idml: changed (offset, from end) {changed:?}: {e}");
            }
            _ => {}
        }
    }
    assert!(both > 300, "{both}");
    // Accepting what the independent reader rejects needs a documented reason.
    assert_eq!(ours_only, 0);
}

fn qualified(prefix: &str, local: &str) -> String {
    if prefix.is_empty() { local.to_owned() } else { format!("{prefix}:{local}") }
}

#[test]
fn random_namespace_edits_are_atomic_and_serialize_reparseably() {
    let story = files().remove(3).1;
    let mut rng = Rng(0x0123_4567_89AB_CDEF);
    let (mut applied, mut refused) = (0, 0);
    for _ in 0..200 {
        let mut tree = XmlTree::parse(&story, &ReadOptions::source()).unwrap();
        for _ in 0..30 {
            let elements: Vec<NodeId> = tree.descendants(tree.root()).filter(|n| tree.name(*n).is_some()).collect();
            let node = elements[rng.below(elements.len())];
            let before = tree.to_xml().unwrap();
            let prefix = ["p", "q", "idPkg", "xml", "xmlns", ""][rng.below(6)];
            let uri = ["urn:plugin", "urn:other", "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging", "http://www.w3.org/XML/1998/namespace", ""]
                [rng.below(5)];
            let op = rng.below(6);
            let detail = format!("op {op} on {:?} prefix {prefix:?} uri {uri:?}\n{before}", tree.name(node).map(|n| n.qualified()));
            let result = match op {
                0 => tree.declare_namespace(node, prefix, uri),
                1 => tree.set_attribute(node, &qualified(prefix, "x"), "v<&\"'\r\n"),
                2 => tree.remove_attribute(node, &qualified(prefix, "id")),
                3 => tree.move_node(node, elements[rng.below(elements.len())], rng.below(3)),
                4 => tree.insert_element(node, rng.below(3), &Element::new(qualified(prefix, "Added"))).map(drop),
                _ => tree.remove_node(node),
            };
            let after = tree.to_xml().unwrap_or_else(|e| panic!("{e} after {result:?}: {detail}"));
            match result {
                Err(_) => {
                    refused += 1;
                    assert_eq!(after, before, "a refused edit changed the tree");
                }
                Ok(()) => {
                    applied += 1;
                    let reparsed = XmlTree::parse(&after, &ReadOptions::source()).unwrap_or_else(|e| panic!("{e}: {after}"));
                    assert_eq!(reparsed.to_xml().unwrap(), after);
                }
            }
        }
    }
    assert!(applied > 300 && refused > 300, "{applied} applied, {refused} refused");
}
