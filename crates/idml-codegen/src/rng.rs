//! Reader for the RELAX NG XML syntax, producing the same patterns as
//! [`crate::rnc`]. Some published InDesign schemas exist only in this form
//! (converted from the compact syntax by Trang).

use roxmltree::{Document, Node};

use crate::rnc::{Define, File, Ids, Kind, NameClass, Pattern, QName, XSD};

const RNG: &str = "http://relaxng.org/ns/structure/1.0";

pub fn parse(text: &str, ids: &mut Ids) -> Result<File, String> {
    let doc = Document::parse(text).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    if !is(root, "grammar") {
        return Err("the root element is not a RELAX NG grammar".to_owned());
    }
    // The first comment, e.g. `Adobe InDesign CS6 8.0.0.370`.
    let header = doc.root().children().find(|n| n.is_comment()).and_then(|n| n.text()).map(|t| t.trim().to_owned());
    let mut file = File { header, start: None, includes: Vec::new(), defines: Vec::new(), stray_lines: Vec::new() };
    grammar(root, ids, &mut file)?;
    Ok(file)
}

fn grammar(node: Node, ids: &mut Ids, file: &mut File) -> Result<(), String> {
    for child in elements(node) {
        match child.tag_name().name() {
            "start" => file.start = Some(group(child, ids)?),
            "define" => {
                if child.attribute("combine").is_some() {
                    return Err("combine is not supported".to_owned());
                }
                let name = child.attribute("name").ok_or("define without name")?.to_owned();
                file.defines.push(Define { name, doc: Vec::new(), pattern: group(child, ids)? });
            }
            "include" if elements(child).next().is_none() => {
                file.includes.push(child.attribute("href").ok_or("include without href")?.to_owned());
            }
            "div" => grammar(child, ids, file)?,
            other => return Err(format!("unsupported <{other}> in grammar")),
        }
    }
    Ok(())
}

fn is(node: Node, name: &str) -> bool {
    node.tag_name().namespace() == Some(RNG) && node.tag_name().name() == name
}

fn elements<'a, 'i>(node: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    node.children().filter(Node::is_element)
}

fn new(ids: &mut Ids, kind: Kind) -> Pattern {
    Pattern { id: ids.next(), kind }
}

/// The child patterns of `node` as one pattern (several form a group).
fn group(node: Node, ids: &mut Ids) -> Result<Pattern, String> {
    let mut items = Vec::new();
    for child in elements(node).filter(|c| !is(*c, "name") && !is(*c, "anyName") && !is(*c, "param")) {
        items.push(pattern(child, ids)?);
    }
    Ok(match items.len() {
        0 => new(ids, Kind::Empty),
        1 => items.pop().ok_or("no pattern")?,
        _ => new(ids, Kind::Group(items)),
    })
}

fn list(node: Node, ids: &mut Ids) -> Result<Vec<Pattern>, String> {
    elements(node).map(|c| pattern(c, ids)).collect()
}

fn pattern(node: Node, ids: &mut Ids) -> Result<Pattern, String> {
    if node.tag_name().namespace() != Some(RNG) {
        return Err(format!("unexpected element {:?}", node.tag_name().name()));
    }
    let kind = match node.tag_name().name() {
        name @ ("element" | "attribute") => {
            let class = name_class(node, name == "attribute")?;
            let content = if name == "attribute" && elements(node).all(|c| is(c, "name") || is(c, "anyName")) {
                new(ids, Kind::Text)
            } else {
                group(node, ids)?
            };
            if name == "element" { Kind::Element(class, Box::new(content)) } else { Kind::Attribute(class, Box::new(content)) }
        }
        "group" => Kind::Group(list(node, ids)?),
        "interleave" => Kind::Interleave(list(node, ids)?),
        "choice" => Kind::Choice(list(node, ids)?),
        "optional" => Kind::Optional(Box::new(group(node, ids)?)),
        "zeroOrMore" => Kind::ZeroOrMore(Box::new(group(node, ids)?)),
        "oneOrMore" => Kind::OneOrMore(Box::new(group(node, ids)?)),
        "list" => Kind::List(Box::new(group(node, ids)?)),
        "ref" => Kind::Ref(node.attribute("name").ok_or("ref without name")?.to_owned()),
        "text" => Kind::Text,
        "empty" => Kind::Empty,
        "notAllowed" => Kind::NotAllowed,
        "value" => {
            let builtin = datatype_library(node).is_empty();
            let token = match node.attribute("type") {
                None => true,
                Some("token") if builtin => true,
                Some("string") => false,
                Some(t) => return Err(format!("unsupported value type {t}")),
            };
            Kind::Value { token, value: node.text().unwrap_or_default().to_owned() }
        }
        "data" => {
            if datatype_library(node) != XSD {
                return Err("data outside the XML Schema datatype library".to_owned());
            }
            let params = elements(node)
                .filter(|c| is(*c, "param"))
                .map(|c| Ok((c.attribute("name").ok_or("param without name")?.to_owned(), c.text().unwrap_or_default().to_owned())))
                .collect::<Result<Vec<_>, String>>()?;
            if elements(node).any(|c| is(c, "except")) {
                return Err("data with except is not supported".to_owned());
            }
            Kind::Data { xsd: node.attribute("type").ok_or("data without type")?.to_owned(), params }
        }
        other => return Err(format!("<{other}> is not supported")),
    };
    Ok(new(ids, kind))
}

/// The `datatypeLibrary` in scope.
fn datatype_library<'a>(node: Node<'a, '_>) -> &'a str {
    node.ancestors().find_map(|n| n.attribute("datatypeLibrary")).unwrap_or("")
}

fn name_class(node: Node, attribute: bool) -> Result<NameClass, String> {
    let name = match node.attribute("name") {
        Some(name) => name,
        None => match elements(node).next() {
            Some(c) if is(c, "anyName") => return Ok(NameClass::Any),
            Some(c) if is(c, "name") => c.text().unwrap_or_default(),
            _ => return Err("unsupported name class".to_owned()),
        },
    };
    let name = name.trim();
    Ok(NameClass::Name(match name.split_once(':') {
        Some((prefix, local)) => {
            let ns = node.lookup_namespace_uri(Some(prefix)).ok_or_else(|| format!("undeclared prefix {prefix}"))?;
            QName { ns: ns.to_owned(), prefix: Some(prefix.to_owned()), local: local.to_owned() }
        }
        // Unprefixed attributes have no namespace; elements take `ns`.
        None if attribute => QName { ns: String::new(), prefix: None, local: name.to_owned() },
        None => {
            let ns = node.ancestors().find_map(|n| n.attribute("ns")).unwrap_or("");
            QName { ns: ns.to_owned(), prefix: None, local: name.to_owned() }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rnc;

    #[test]
    fn matches_the_compact_syntax() {
        let xml = r#"<?xml version="1.0"?>
<!-- Adobe InDesign CS6 8.0.0.370 -->
<grammar xmlns:p="urn:p" xmlns="http://relaxng.org/ns/structure/1.0" datatypeLibrary="http://www.w3.org/2001/XMLSchema-datatypes">
  <start><ref name="A"/></start>
  <include href="d.rng"/>
  <define name="A">
    <element name="p:A">
      <optional><attribute name="X"><data type="double"><param name="minInclusive">0</param><param name="maxInclusive">1</param></data></attribute></optional>
      <interleave><zeroOrMore><ref name="B"/></zeroOrMore><optional><ref name="C"/></optional></interleave>
    </element>
  </define>
  <define name="C"><element name="C"><choice><value type="string" datatypeLibrary="">x</value><value type="string" datatypeLibrary="">y</value></choice></element></define>
</grammar>"#;
        let f = parse(xml, &mut Ids::default()).unwrap();
        let c = rnc::parse(
            "namespace p=\"urn:p\"\ngrammar {\nstart = A\ninclude \"d.rnc\"\nA = element p:A {\n  attribute X { xsd:double {minInclusive=\"0\" maxInclusive=\"1\"} }?,\n  (B* & C?)\n}\nC = element C { string \"x\" | string \"y\" }\n}",
            &mut Ids::default(),
        )
        .unwrap();
        assert_eq!(f.header.as_deref(), Some("Adobe InDesign CS6 8.0.0.370"));
        assert_eq!(f.includes, ["d.rng"]);
        let keys = |f: &File| f.defines.iter().map(|d| (d.name.clone(), d.pattern.key())).collect::<Vec<_>>();
        assert_eq!(keys(&f), keys(&c));
    }
}
