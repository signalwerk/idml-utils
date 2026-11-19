//! Keep source XML authoritative; apply edits made to its typed projection.
use std::collections::BTreeMap;
use std::ops::Range;

use roxmltree::{Document, Node};

use crate::{Error, ErrorKind, Parsed, PartFile, ReadOptions};

/// A standalone part with its source XML and an editable typed projection.
/// Unknown data, lexical forms and the source DOMVersion survive writing.
/// An edit whose placement cannot be determined without losing source data
/// returns a write error. `PartFile` alone is a schema projection.
#[derive(Debug, Clone)]
pub struct XmlPart<T> {
    pub value: T,
    source: SourcePart,
}

impl<T: PartFile> XmlPart<T> {
    pub fn read(xml: &str, options: &ReadOptions) -> Result<Parsed<Self>, Error> {
        let parsed = T::from_xml(xml, options)?;
        let baseline = parsed.value.to_xml()?;
        Ok(Parsed {
            value: Self { value: parsed.value, source: SourcePart { original: xml.into(), baseline, preserve_version: true } },
            warnings: parsed.warnings,
        })
    }

    pub fn write(&self) -> Result<String, Error> {
        self.source.merge(&self.value.to_xml()?)
    }

    pub fn original_xml(&self) -> &str {
        &self.source.original
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SourcePart {
    pub original: String,
    pub baseline: String,
    pub preserve_version: bool,
}

impl SourcePart {
    pub fn merge(&self, edited: &str) -> Result<String, Error> {
        if self.baseline == edited {
            return Ok(self.original.clone());
        }
        let parse_error = |e| error(format!("cannot merge XML: {e}"));
        let source = Document::parse(&self.original).map_err(parse_error)?;
        let baseline = Document::parse(&self.baseline).map_err(parse_error)?;
        let edited = Document::parse(edited).map_err(parse_error)?;
        if self.preserve_version
            && baseline.root_element().attribute("DOMVersion") != edited.root_element().attribute("DOMVersion")
            && source.root_element().attribute("DOMVersion") != edited.root_element().attribute("DOMVersion")
        {
            return Err(error("changing DOMVersion requires an explicit conversion, not a layout label"));
        }
        let root = source.root_element();
        let replacement = merge(root, baseline.root_element(), edited.root_element(), self.preserve_version)?;
        let merged = patch(&self.original, vec![(root.range(), replacement)])?;
        Document::parse(&merged).map_err(|e| error(format!("source edit would produce invalid XML: {e}")))?;
        Ok(merged)
    }
}

fn error(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::Write, message)
}

fn name<'a, 'i>(n: Node<'a, 'i>) -> (Option<&'a str>, &'a str) {
    (n.tag_name().namespace(), n.tag_name().name())
}

fn attrs<'a, 'i>(n: Node<'a, 'i>) -> BTreeMap<(Option<&'a str>, &'a str), &'a str> {
    n.attributes().map(|a| ((a.namespace(), a.name()), a.value())).collect()
}

fn children<'a, 'i>(n: Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    n.children().filter(Node::is_element).collect()
}

fn text(n: Node<'_, '_>) -> String {
    n.children().filter(Node::is_text).filter_map(|n| n.text()).collect()
}

/// Formatting whitespace is insignificant only in element content. Comments
/// and instructions are compared too, so a structural replacement cannot erase them.
fn same(a: Node<'_, '_>, b: Node<'_, '_>) -> bool {
    let namespaces = |n: Node<'_, '_>| n.namespaces().map(|ns| (ns.name().map(str::to_owned), ns.uri().to_owned())).collect::<BTreeMap<_, _>>();
    if name(a) != name(b) || attrs(a) != attrs(b) || namespaces(a) != namespaces(b) {
        return false;
    }
    let ac = children(a);
    let bc = children(b);
    if ac.len() != bc.len() || !ac.iter().zip(&bc).all(|(a, b)| same(*a, *b)) {
        return false;
    }
    let at = text(a);
    let bt = text(b);
    if (ac.is_empty() && at != bt) || (!ac.is_empty() && at.trim_matches([' ', '\t', '\r', '\n']) != bt.trim_matches([' ', '\t', '\r', '\n'])) {
        return false;
    }
    let misc = |n: Node<'_, '_>| {
        n.children().filter(|n| n.is_comment() || n.is_pi()).map(|n| n.document().input_text()[n.range()].to_owned()).collect::<Vec<_>>()
    };
    misc(a) == misc(b)
}

fn raw<'a, 'i>(n: Node<'a, 'i>) -> &'i str {
    &n.document().input_text()[n.range()]
}

fn merge(source: Node<'_, '_>, baseline: Node<'_, '_>, edited: Node<'_, '_>, preserve_version: bool) -> Result<String, Error> {
    if same(baseline, edited) {
        return Ok(raw(source).into());
    }
    let fail = || {
        error(format!(
            "edit to <{}> cannot be mapped to the source without risking data loss; edit the source XML or explicitly build a new typed package",
            source.tag_name().name()
        ))
    };
    if name(source) != name(baseline) || name(baseline) != name(edited) {
        return Err(fail());
    }
    let bc = children(baseline);
    let ec = children(edited);
    let identity = |n: Node<'_, '_>| (name(n).0.map(str::to_owned), name(n).1.to_owned(), n.attribute("Self").map(str::to_owned));
    let foreign_version = preserve_version && source.document().root_element().attribute("DOMVersion") != Some(crate::model::DOM_VERSION);
    let ba = attrs(baseline);
    let ea = attrs(edited);
    let mut edits = Vec::new();
    let insert_at = source.range().start + 1 + raw(source)[1..].find([' ', '\t', '\r', '\n', '/', '>']).ok_or_else(fail)?;
    for key in ba.keys().chain(ea.keys()).collect::<std::collections::BTreeSet<_>>() {
        if ba.get(key) == ea.get(key) {
            continue;
        }
        // A version label is not a conversion. The original version is preserved.
        if preserve_version && *key == (None, "DOMVersion") {
            if source.attribute("DOMVersion") == edited.attribute("DOMVersion") {
                continue;
            }
            return Err(error("changing DOMVersion requires an explicit conversion, not a layout label"));
        }
        let sa = source.attributes().find(|a| (a.namespace(), a.name()) == *key);
        let new = edited.attributes().find(|a| (a.namespace(), a.name()) == *key);
        if foreign_version && new.is_some() && sa.is_none_or(|s| ba.get(key).is_some_and(|b| s.value() != *b)) {
            return Err(error("adding an attribute or changing a migrated value in another DOMVersion requires an explicit validated conversion"));
        }
        match (sa, new) {
            (Some(s), Some(e)) => edits.push((s.range(), edited.document().input_text()[e.range()].to_owned())),
            (Some(s), None) => edits.push((s.range(), String::new())),
            (None, Some(e)) => edits.push((insert_at..insert_at, format!(" {}", &edited.document().input_text()[e.range()]))),
            (None, None) => {}
        }
    }
    if bc.iter().map(|n| identity(*n)).ne(ec.iter().map(|n| identity(*n))) {
        if foreign_version
            && ec.iter().enumerate().any(|(i, e)| {
                ec[..=i].iter().filter(|n| identity(**n) == identity(*e)).count() > bc.iter().filter(|n| identity(**n) == identity(*e)).count()
            })
        {
            return Err(error("adding elements to another DOMVersion requires an explicit validated conversion"));
        }
        return if same(source, baseline) { Ok(raw(edited).into()) } else { Err(fail()) };
    }
    if bc.is_empty() {
        if text(baseline) != text(edited) || baseline.children().filter(Node::is_pi).map(raw).ne(edited.children().filter(Node::is_pi).map(raw)) {
            if source.children().any(|n| n.is_element() || n.is_comment() || n.pi().is_some_and(|p| p.target != "ACE")) {
                return Err(fail());
            }
            let content = inner(edited)?;
            let open = opening_end(source)?;
            if source.document().input_text().get(open - 2..open) == Some("/>") {
                let tag = raw(source)[1..].split([' ', '\t', '\n', '\r', '/', '>']).next().ok_or_else(fail)?;
                edits.push((open - 2..open, format!(">{content}</{tag}>")));
            } else {
                let end = source.range().start + raw(source).rfind("</").ok_or_else(fail)?;
                edits.push((open..end, content.into()));
            }
        }
    } else {
        let sc = children(source);
        let mut occurrences = BTreeMap::new();
        for (b, e) in bc.iter().zip(&ec) {
            if same(*b, *e) {
                continue;
            }
            let key = identity(*b);
            // Count preceding siblings, including unchanged ones.
            let index = bc.iter().take_while(|n| **n != *b).filter(|n| identity(**n) == key).count();
            let candidates: Vec<_> = sc.iter().filter(|n| identity(**n) == key).collect();
            let s = candidates.get(index).ok_or_else(fail)?;
            if key.2.is_some() && candidates.len() != 1 {
                return Err(fail());
            }
            if occurrences.insert(s.range().start, ()).is_some() {
                return Err(fail());
            }
            edits.push((s.range(), merge(**s, *b, *e, preserve_version)?));
        }
    }
    // Patch coordinates are relative to the original complete XML document.
    let start = source.range().start;
    patch(raw(source), edits.into_iter().map(|(r, s)| (r.start - start..r.end - start, s)).collect())
}

fn opening_end(n: Node<'_, '_>) -> Result<usize, Error> {
    let mut quote = None;
    for (i, c) in raw(n).char_indices() {
        match (quote, c) {
            (Some(q), c) if q == c => quote = None,
            (None, '\'' | '"') => quote = Some(c),
            (None, '>') => return Ok(n.range().start + i + 1),
            _ => {}
        }
    }
    Err(error("unterminated start tag"))
}

fn inner<'a, 'i>(n: Node<'a, 'i>) -> Result<&'i str, Error> {
    let xml = n.document().input_text();
    let open = opening_end(n)?;
    if xml.get(open - 2..open) == Some("/>") {
        return Ok("");
    }
    let end = n.range().start + raw(n).rfind("</").ok_or_else(|| error("missing end tag"))?;
    Ok(&xml[open..end])
}

fn patch(xml: &str, mut edits: Vec<(Range<usize>, String)>) -> Result<String, Error> {
    edits.sort_by_key(|(r, _)| r.start);
    let mut out = String::new();
    let mut at = 0;
    for (r, value) in edits {
        if r.start < at || r.end > xml.len() {
            return Err(error("overlapping source edits"));
        }
        out.push_str(&xml[at..r.start]);
        out.push_str(&value);
        at = r.end;
    }
    out.push_str(&xml[at..]);
    Ok(out)
}
