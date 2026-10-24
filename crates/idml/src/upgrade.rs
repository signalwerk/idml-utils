//! Reading documents of older `DOMVersion`s.
//!
//! A part whose root has an older `DOMVersion` is edited into a part of the
//! current version before it is read: the edits are generated from
//! `schema-upgrades.tsv` ([`crate::model::upgrades`]), which has a rule for
//! every schema item older versions have and the current one has not, or
//! has differently. Edits that lose data are reported as warnings. The
//! edits keep line numbers, so locations in errors still point into the
//! original part.

use std::collections::HashMap;
use std::ops::Range;

use roxmltree::{Document, Node};

use crate::error::Location;
use crate::model::DOM_VERSION;
use crate::model::upgrades::RULES;
use crate::xml::{Ctx, qualified};

/// A `DOMVersion`: `(7, 5)` for `7.5`.
pub(crate) type Dom = (u16, u16);

/// An edit for documents after `after` (any older one if `None`) and before
/// `before`, of elements whose path ends with `path`.
pub(crate) struct Rule {
    pub after: Option<Dom>,
    pub before: Dom,
    pub path: &'static [&'static str],
    pub edit: Edit,
    /// Why data is lost, for the warning.
    pub why: &'static str,
}

pub(crate) enum Edit {
    /// Leaves the element out.
    Drop,
    /// Puts the element into new elements, outermost first.
    Wrap(&'static [&'static str]),
    /// Leaves the attribute out.
    DropAttribute(&'static str),
    /// Leaves the attribute out if it has this value.
    DropValue(&'static str, &'static str),
    /// Replaces the attribute's value if it is the first by the second.
    MapValue(&'static str, &'static str, &'static str),
    /// Adds the attribute with this value where it is missing.
    Default(&'static str, &'static str),
}

fn dom(text: &str) -> Option<Dom> {
    let (major, minor) = text.trim().split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// The part edited into the current `DOMVersion`, if its root element has an
/// older one. Parts of the current, a newer or no `DOMVersion` are read as
/// they are.
pub(crate) fn upgrade(ctx: &mut Ctx, doc: &Document, xml: &str) -> Option<String> {
    let root = doc.root_element();
    let attr = root.attributes().find(|a| a.namespace().is_none() && a.name() == "DOMVersion")?;
    let version = dom(attr.value())?;
    if version >= dom(DOM_VERSION)? {
        return None;
    }
    let mut rules: HashMap<&str, Vec<&Rule>> = HashMap::new();
    for r in RULES.iter().filter(|r| r.after.is_none_or(|a| a < version) && version < r.before) {
        if let Some(last) = r.path.last() {
            rules.entry(last).or_default().push(r);
        }
    }
    let mut u = Upgrader { ctx, doc, xml, rules, edits: vec![(attr.range_value(), DOM_VERSION.to_owned())], path: Vec::new() };
    u.element(root);
    let mut edits = u.edits;
    // Stable: insertions at one position stay in document order.
    edits.sort_by_key(|(range, _)| range.start);
    edits.dedup();
    let mut out = String::with_capacity(xml.len());
    let mut at = 0;
    for (range, text) in edits {
        // Two rules for one attribute: the first edit wins.
        if range.start < at {
            continue;
        }
        out.push_str(xml.get(at..range.start)?);
        out.push_str(&text);
        at = range.end;
    }
    out.push_str(xml.get(at..)?);
    Some(out)
}

struct Upgrader<'c, 'a, 'i> {
    ctx: &'c mut Ctx,
    doc: &'a Document<'i>,
    xml: &'i str,
    rules: HashMap<&'static str, Vec<&'static Rule>>,
    /// Replacements of byte ranges of `xml`, in document order.
    edits: Vec<(Range<usize>, String)>,
    path: Vec<String>,
}

impl Upgrader<'_, '_, '_> {
    fn element(&mut self, node: Node) {
        self.path.push(qualified(node).into_owned());
        let name = self.path.last().cloned().unwrap_or_default();
        let rules: Vec<&Rule> = self.rules.get(name.as_str()).into_iter().flatten().copied().filter(|r| ends_with(&self.path, r.path)).collect();
        if let Some(r) = rules.iter().find(|r| matches!(r.edit, Edit::Drop)) {
            let range = node.range();
            self.warn(range.start, format!("<{name}> left out: {}", r.why));
            self.edits.push((range.clone(), newlines(self.xml.get(range).unwrap_or_default())));
            self.path.pop();
            return;
        }
        let mut close = String::new();
        for r in &rules {
            match r.edit {
                Edit::Drop => {}
                Edit::Wrap(names) => {
                    let open: String = names.iter().map(|n| format!("<{n}>")).collect();
                    self.edits.push((node.range().start..node.range().start, open));
                    close = names.iter().rev().map(|n| format!("</{n}>")).collect::<String>() + &close;
                }
                Edit::Default(attr, value) => {
                    if node.attribute(attr).is_none() {
                        let at = self.name_end(node.range().start);
                        self.edits.push((at..at, format!(" {attr}=\"{}\"", escape(value))));
                    }
                }
                Edit::DropAttribute(attr) => self.drop_attribute(node, attr, None, r.why),
                Edit::DropValue(attr, value) => self.drop_attribute(node, attr, Some(value), r.why),
                Edit::MapValue(attr, from, to) => {
                    if let Some(a) = node.attributes().find(|a| a.namespace().is_none() && a.name() == attr && a.value() == from) {
                        self.edits.push((a.range_value(), escape(to)));
                    }
                }
            }
        }
        for child in node.children().filter(Node::is_element) {
            self.element(child);
        }
        if !close.is_empty() {
            self.edits.push((node.range().end..node.range().end, close));
        }
        self.path.pop();
    }

    fn drop_attribute(&mut self, node: Node, attr: &str, value: Option<&str>, why: &str) {
        let Some(a) = node.attributes().find(|a| a.namespace().is_none() && a.name() == attr && value.is_none_or(|v| a.value() == v)) else {
            return;
        };
        let range = a.range();
        // With the whitespace before it.
        let start = self.xml.get(..range.start).unwrap_or_default().trim_end_matches([' ', '\t', '\n', '\r']).len();
        self.warn(range.start, format!("attribute {attr}=\"{}\" left out: {why}", a.value()));
        self.edits.push((start..range.end, newlines(self.xml.get(start..range.end).unwrap_or_default())));
    }

    /// The end of the qualified name of the start tag at `start`.
    fn name_end(&self, start: usize) -> usize {
        let rest = self.xml.get(start + 1..).unwrap_or_default();
        start + 1 + rest.find([' ', '\t', '\n', '\r', '/', '>']).unwrap_or(rest.len())
    }

    fn warn(&mut self, at: usize, message: String) {
        let pos = self.doc.text_pos_at(at);
        let location = Location { part: self.ctx.part.clone(), line: pos.row, column: pos.col, element: self.path.join("/") };
        self.ctx.warn(message, location);
    }
}

fn ends_with(path: &[String], names: &[&str]) -> bool {
    path.len().checked_sub(names.len()).and_then(|i| path.get(i..)).is_some_and(|tail| tail.iter().zip(names).all(|(a, b)| a == b))
}

/// The line breaks of removed text, so that line numbers stay.
fn newlines(removed: &str) -> String {
    "\n".repeat(removed.matches('\n').count())
}

fn escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReadOptions;

    fn run(xml: &str) -> (Option<String>, Vec<String>) {
        let mut ctx = Ctx::new(&ReadOptions::default(), "part.xml");
        let doc = Document::parse(xml).unwrap();
        let out = upgrade(&mut ctx, &doc, xml);
        (out, ctx.into_warnings().into_iter().map(|w| format!("{}: {}", w.location, w.message)).collect())
    }

    const NS: &str = r#"xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging""#;

    #[test]
    fn wraps_and_drops_keeping_lines() {
        let xml = format!(
            "<idPkg:Story {NS} DOMVersion=\"6.0\">\n<Story Self=\"s\"><TextFrame Self=\"a\"/><Oval Self=\"b\"/></Story>\n\
             <XmlStory><StoryPreference\n/><Table/></XmlStory>\n</idPkg:Story>"
        );
        let (out, warnings) = run(&xml);
        assert_eq!(
            out.unwrap(),
            format!(
                "<idPkg:Story {NS} DOMVersion=\"21.5\">\n<Story Self=\"s\"><CharacterStyleRange><TextFrame Self=\"a\"/></CharacterStyleRange>\
                 <CharacterStyleRange><Oval Self=\"b\"/></CharacterStyleRange></Story>\n\
                 <XmlStory>\n<CharacterStyleRange><Table/></CharacterStyleRange></XmlStory>\n</idPkg:Story>"
            )
        );
        assert_eq!(
            warnings,
            [
                "part.xml:3:11 (idPkg:Story/XmlStory/StoryPreference): <StoryPreference> left out: the XML backing story has no story settings since 7.0"
            ]
        );
    }

    #[test]
    fn maps_values_and_adds_defaults() {
        let xml = format!(
            r#"<idPkg:Preferences {NS} DOMVersion="7.5"><HTMLExportPreference NumberedListExportOption="StaticOrderedList"/></idPkg:Preferences>"#
        );
        assert_eq!(
            run(&xml).0.unwrap(),
            format!(
                r#"<idPkg:Preferences {NS} DOMVersion="21.5"><HTMLExportPreference NumberedListExportOption="OrderedList"/></idPkg:Preferences>"#
            )
        );
        let xml = format!(r#"<idPkg:Graphic {NS} DOMVersion="10.0"><ColorGroup Self="g"/></idPkg:Graphic>"#);
        assert_eq!(
            run(&xml).0.unwrap(),
            format!(r#"<idPkg:Graphic {NS} DOMVersion="21.5"><ColorGroup IsRootColorGroup="false" Name="" Self="g"/></idPkg:Graphic>"#)
        );
    }

    #[test]
    fn current_and_newer_versions_are_read_as_they_are() {
        for dom in ["21.5", "22.0", "x"] {
            let xml = format!(r#"<idPkg:Story {NS} DOMVersion="{dom}"><Story><TextFrame/></Story></idPkg:Story>"#);
            assert_eq!(run(&xml), (None, Vec::new()));
        }
    }
}
