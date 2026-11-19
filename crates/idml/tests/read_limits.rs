#![allow(clippy::unwrap_used)]

use idml::{DomVersion, Element, ErrorKind, Package, ReadOptions, XmlTree};
use std::collections::BTreeMap;

fn is_limit(result: Result<impl Sized, idml::Error>, name: &str) {
    let error = result.err().unwrap();
    assert_eq!(error.kind(), ErrorKind::Limit);
    assert!(error.message().contains(name), "{}", error.message());
}

#[test]
fn lexical_nodes_and_attributes_are_limited_per_part() {
    let xml = "<?xml version='1.0'?><a xmlns:p='urn:p' a='1'><!--c--><b/>x</a>";
    // declaration, a, comment, b, text: 5 nodes; two attributes.
    let good = ReadOptions { max_nodes: 5, max_attributes: 2, ..ReadOptions::source() };
    let tree = XmlTree::parse(xml, &good).unwrap();
    tree.check_limits(&good).unwrap();
    is_limit(XmlTree::parse(xml, &ReadOptions { max_nodes: 4, ..good.clone() }), "max_nodes");
    is_limit(XmlTree::parse(xml, &ReadOptions { max_attributes: 1, ..good.clone() }), "max_attributes");
    is_limit(tree.check_limits(&ReadOptions { max_nodes: 4, ..good.clone() }), "max_nodes");
    is_limit(tree.check_limits(&ReadOptions { max_attributes: 1, ..good }), "max_attributes");
}

#[test]
fn many_attributes_still_reject_duplicate_expanded_names() {
    let attrs = (0..5000).map(|i| format!(" a{i}='x'")).collect::<String>();
    let options = ReadOptions { max_attributes: 5001, ..ReadOptions::source() };
    XmlTree::parse(&format!("<root{attrs}/>"), &options).unwrap();
    let error = XmlTree::parse(&format!("<root{attrs} a0='y'/>"), &options).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Xml);
    assert!(error.message().contains("duplicate"));
}

#[test]
fn producer_limits_apply_before_adding_or_building_parts() {
    let mut root = Element::new("Document");
    root.set_attr("Self", "doc");
    root.children.extend((0..20).map(|_| Element::new("Empty")));
    let options = ReadOptions { max_nodes: 21, ..ReadOptions::source() };
    // Generated XML declaration + root + 20 children.
    is_limit(Package::from_elements(&root, &BTreeMap::new(), DomVersion { major: 21, minor: 5 }, &options), "max_nodes");
    let options = ReadOptions { max_nodes: 22, max_attributes: 2, ..options };
    // Self plus two default attributes (version and namespace).
    is_limit(Package::from_elements(&root, &BTreeMap::new(), DomVersion { major: 21, minor: 5 }, &options), "max_attributes");
    let options = ReadOptions { max_entries: 2, ..ReadOptions::source() };
    is_limit(Package::from_elements(&root, &BTreeMap::new(), DomVersion { major: 21, minor: 5 }, &options), "max_entries");
    let mut doc = Package::new(DomVersion { major: 21, minor: 5 }, "d").unwrap();
    let before = doc.write().unwrap();
    let part = Element::new("idPkg:Story");
    let options = ReadOptions { max_entries: 3, ..ReadOptions::source() };
    is_limit(doc.add_part("Stories/Story_u1.xml", &part, &options), "max_entries");
    assert_eq!(doc.write().unwrap(), before);
}

#[test]
fn edited_size_and_package_total_are_rechecked_atomically() {
    let mut tree = XmlTree::parse("<root a='x'/>", &ReadOptions::source()).unwrap();
    tree.set_attribute(tree.root(), "a", &"&".repeat(100)).unwrap();
    let options = ReadOptions { max_entry_size: 100, ..ReadOptions::source() };
    is_limit(tree.check_limits(&options), "size budget");

    let mut tree = XmlTree::parse("<root>x</root>", &ReadOptions::source()).unwrap();
    tree.set_text(tree.root(), "&&").unwrap();
    // One text arena node serializes to two entity-reference nodes.
    let options = ReadOptions { max_nodes: 2, ..ReadOptions::source() };
    is_limit(tree.check_limits(&options), "max_nodes");

    let mut package = Package::new(DomVersion { major: 21, minor: 5 }, "d").unwrap();
    let before = package.write().unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&before)).unwrap();
    let original_total: u64 = (0..zip.len()).map(|i| zip.by_index(i).unwrap().size()).sum();
    let options = ReadOptions { max_size: original_total, ..ReadOptions::source() };
    package.check_limits(&options).unwrap();
    is_limit(package.add_part("Stories/Story_u1.xml", &Element::new("idPkg:Story"), &options), "max_size");
    assert_eq!(package.write().unwrap(), before);

    package.other_files_mut().insert("extra.bin".into(), vec![42; 20]);
    is_limit(package.check_limits(&options), "max_size");
}

#[test]
fn producer_sizes_count_escaping_without_serializing() {
    for (text, cdata) in [(None, false), (Some("a&<>\r\u{18}b"), false), (Some("a]]>b"), true)] {
        let mut doc = Element::new("Document");
        doc.set_attr("Self", format!("{}&\"<", "d".repeat(256)));
        if let Some(text) = text {
            let mut child = Element::new("Content");
            child.text = Some(text.to_owned());
            child.cdata = cdata;
            doc.children.push(child);
        }
        let options = ReadOptions::source();
        let package = Package::from_elements(&doc, &BTreeMap::new(), DomVersion { major: 21, minor: 5 }, &options).unwrap();
        let encoded = package.write().unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(encoded)).unwrap();
        let size = zip.by_name("designmap.xml").unwrap().size();
        let total: u64 = (0..zip.len()).map(|i| zip.by_index(i).unwrap().size()).sum();
        let exact = ReadOptions { max_entry_size: size, ..options.clone() };
        Package::from_elements(&doc, &BTreeMap::new(), DomVersion { major: 21, minor: 5 }, &exact).unwrap();
        let too_small = ReadOptions { max_entry_size: size - 1, ..options };
        is_limit(Package::from_elements(&doc, &BTreeMap::new(), DomVersion { major: 21, minor: 5 }, &too_small), "size budget");
        let exact = ReadOptions { max_size: total, ..ReadOptions::source() };
        Package::from_elements(&doc, &BTreeMap::new(), DomVersion { major: 21, minor: 5 }, &exact).unwrap();
        let too_small = ReadOptions { max_size: total - 1, ..ReadOptions::source() };
        is_limit(Package::from_elements(&doc, &BTreeMap::new(), DomVersion { major: 21, minor: 5 }, &too_small), "size budget");
    }
}

#[test]
fn package_parsing_checks_node_and_attribute_counts_in_container_and_parts() {
    use std::io::Write;
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let document = format!("<Document Self='d' DOMVersion='21.5'>{}</Document>", "<X/>".repeat(10));
    for (path, content) in [
        ("mimetype", idml::MIMETYPE),
        ("META-INF/container.xml", "<container><rootfiles><rootfile full-path='designmap.xml'/></rootfiles></container>"),
        ("designmap.xml", document.as_str()),
    ] {
        writer.start_file(path, zip::write::SimpleFileOptions::default()).unwrap();
        writer.write_all(content.as_bytes()).unwrap();
    }
    let bytes = writer.finish().unwrap().into_inner();
    let options = ReadOptions { max_nodes: 5, ..ReadOptions::source() };
    let err = Package::read(&bytes, &options).err().unwrap();
    assert_eq!(err.kind(), ErrorKind::Limit);
    assert_eq!(err.location().unwrap().part, "designmap.xml");
    #[cfg(feature = "owned")]
    {
        let err = idml::OwnedPackage::read(&bytes, &options).err().unwrap();
        assert_eq!(err.kind(), ErrorKind::Limit);
        assert_eq!(err.location().unwrap().part, "designmap.xml");
    }
    let options = ReadOptions { max_attributes: 0, ..ReadOptions::source() };
    let err = Package::read(&bytes, &options).err().unwrap();
    assert_eq!(err.kind(), ErrorKind::Limit);
    assert_eq!(err.location().unwrap().part, "META-INF/container.xml");
    #[cfg(feature = "owned")]
    {
        let err = idml::OwnedPackage::read(&bytes, &options).err().unwrap();
        assert_eq!(err.kind(), ErrorKind::Limit);
        assert_eq!(err.location().unwrap().part, "META-INF/container.xml");
    }
}
