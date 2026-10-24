//! Lowers the grammar to the [`Model`].
//!
//! Lowering is total over the constructs InDesign's schemas use and fails on
//! anything else. It records every pattern node it represents, and
//! [`lower`] fails unless every node of every reachable definition is
//! represented. That check is what makes the generated model complete.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::model::*;
use crate::names::{camel, snake};
use crate::rnc::{Kind, NameClass, Pattern, QName};
use crate::schema::Schema;

/// The IDML packaging namespace, bound to the `idPkg` prefix.
pub const PACKAGING_NS: &str = "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging";

/// Generated modules by defining file. A definition shared by several files
/// goes into the first module listed here.
const MODULES: &[(&str, &str)] = &[
    ("designmap.rnc", "designmap"),
    ("Resources/Graphic.rnc", "graphic"),
    ("Resources/Fonts.rnc", "fonts"),
    ("Resources/Styles.rnc", "styles"),
    ("Resources/Preferences.rnc", "preferences"),
    ("Spreads/Spread.rnc", "spread"),
    ("MasterSpreads/MasterSpread.rnc", "master_spread"),
    ("Stories/Story.rnc", "story"),
    ("XML/BackingStory.rnc", "backing_story"),
    ("XML/Tags.rnc", "tags"),
    ("XML/Mapping.rnc", "mapping"),
    ("datatype.rnc", "datatype"),
];

pub fn lower(schema: &Schema) -> Result<Model, String> {
    let mut l = Lower::new(schema)?;
    let mut roots = Vec::new();
    for e in &schema.entries {
        let ty = l.def_type(&e.start)?;
        roots.push(Root { file: e.file.clone(), element: qualified(&e.root)?, ty });
    }
    l.check_coverage()?;
    l.box_recursion();
    l.expand_traces();

    let mut versions = BTreeSet::new();
    for r in &roots {
        if let TypeKind::Struct(c) = &l.types[r.ty].kind {
            versions.extend(c.fixed.iter().filter(|f| f.name == "DOMVersion").map(|f| f.value.clone()));
        }
    }
    let dom_version = match versions.len() {
        1 => versions.into_iter().next().unwrap_or_default(),
        _ => return Err(format!("expected one fixed DOMVersion on the part roots, found {versions:?}")),
    };
    let unreachable = schema
        .defs
        .iter()
        .filter(|(name, _)| !schema.reachable.contains(*name))
        .map(|(name, def)| (name.clone(), def.defined_in.iter().cloned().collect()))
        .collect();
    Ok(Model {
        schema_dir: schema.dir.to_string_lossy().into_owned(),
        indesign_version: schema.indesign_version.clone(),
        dom_version,
        sha256: schema.sha256.clone(),
        files: schema.files.clone(),
        stray_lines: schema.stray_lines.clone(),
        types: l.types,
        roots,
        unreachable,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefKind {
    /// `attribute name { literal }`
    Fixed,
    Enum,
    /// Text value: data, lists, unions.
    Value,
    /// `element name { … }`
    Element,
    /// Attributes and elements without an element of its own.
    Content,
}

enum Lowered {
    Content(Content),
    Choice(Vec<Variant>),
}

/// Where nested types get their names and module from.
#[derive(Clone)]
struct Owner {
    name: String,
    module: String,
    path: String,
}

impl Owner {
    fn child(&self, name: &str, element: &str) -> Owner {
        Owner { name: name.to_owned(), module: self.module.clone(), path: format!("{}/<{element}>", self.path) }
    }
}

/// An inline element already lowered, and its first occurrence.
#[derive(Clone)]
struct Seen<'s> {
    fixed: Vec<Fixed>,
    ty: Ty,
    node: u32,
    content: &'s Pattern,
}

struct Lower<'s> {
    s: &'s Schema,
    types: Vec<Type>,
    def_types: HashMap<String, TypeId>,
    /// Rust names of the type definitions, reserved up front.
    def_names: HashSet<String>,
    names: HashSet<String>,
    /// Lowered inline elements by name and content.
    inline: HashMap<(String, String), Seen<'s>>,
    /// Pattern nodes equal to an earlier occurrence's node, by that node.
    aliases: HashMap<u32, Vec<u32>>,
    ranged: HashMap<(Scalar, String, String), TypeId>,
    unions: HashMap<Vec<TypeId>, TypeId>,
    /// Local names of inline elements that occur with different contents.
    multi: HashSet<String>,
    /// Part root element (qualified) to its start definition.
    roots: HashMap<String, String>,
    covered: HashSet<u32>,
}

impl<'s> Lower<'s> {
    fn new(s: &'s Schema) -> Result<Self, String> {
        let mut l = Lower {
            s,
            types: Vec::new(),
            def_types: HashMap::new(),
            def_names: HashSet::new(),
            names: HashSet::new(),
            inline: HashMap::new(),
            aliases: HashMap::new(),
            ranged: HashMap::new(),
            unions: HashMap::new(),
            multi: HashSet::new(),
            roots: HashMap::new(),
            covered: HashSet::new(),
        };
        for e in &s.entries {
            l.roots.insert(qualified(&e.root)?, e.start.clone());
        }
        let mut contents: HashMap<String, HashSet<String>> = HashMap::new();
        for name in &s.reachable {
            if l.def_kind(name)? != DefKind::Fixed {
                let rust = def_type_name(name);
                if !l.def_names.insert(rust.clone()) {
                    return Err(format!("two definitions map to the Rust name {rust}"));
                }
            }
            let p = &s.def(name)?.pattern;
            let inner = match &p.kind {
                Kind::Element(_, c) => c,
                _ => p,
            };
            inner.walk(&mut |q| {
                if let Kind::Element(NameClass::Name(n), c) = &q.kind {
                    contents.entry(n.local.clone()).or_default().insert(c.key());
                }
            });
        }
        l.multi = contents.into_iter().filter(|(_, keys)| keys.len() > 1).map(|(n, _)| n).collect();
        Ok(l)
    }

    fn cover(&mut self, p: &'s Pattern) {
        self.covered.insert(p.id);
    }

    fn cover_tree(&mut self, p: &'s Pattern) {
        p.walk(&mut |q| {
            self.covered.insert(q.id);
        });
    }

    fn check_coverage(&self) -> Result<(), String> {
        let mut missing = Vec::new();
        for name in &self.s.reachable {
            self.s.def(name)?.pattern.walk(&mut |p| {
                if !self.covered.contains(&p.id) {
                    missing.push(format!("{name}: {}", describe(p)));
                }
            });
        }
        if missing.is_empty() {
            Ok(())
        } else {
            let n = missing.len();
            missing.truncate(20);
            Err(format!("{n} schema patterns have no representation:\n  {}", missing.join("\n  ")))
        }
    }

    // -- definitions -------------------------------------------------------

    fn def_kind(&self, name: &str) -> Result<DefKind, String> {
        let p = &self.s.def(name)?.pattern;
        Ok(match &p.kind {
            Kind::Attribute(_, v) if matches!(v.kind, Kind::Value { .. }) => DefKind::Fixed,
            Kind::Element(..) => DefKind::Element,
            _ if name.ends_with("_EnumValue") => DefKind::Enum,
            _ if self.is_value(p)? => DefKind::Value,
            _ => DefKind::Content,
        })
    }

    /// True for patterns that describe a text value.
    fn is_value(&self, p: &'s Pattern) -> Result<bool, String> {
        Ok(match &p.kind {
            Kind::Text | Kind::Data { .. } | Kind::Value { .. } | Kind::List(_) => true,
            Kind::Choice(ps) => {
                let mut all = true;
                for q in ps {
                    all &= self.is_value(q)?;
                }
                all
            }
            Kind::Ref(r) => matches!(self.def_kind(r)?, DefKind::Enum | DefKind::Value),
            _ => false,
        })
    }

    fn module_of(&self, name: &str) -> Result<String, String> {
        let def = self.s.def(name)?;
        for (file, module) in MODULES {
            if def.defined_in.contains(*file) {
                return Ok((*module).to_owned());
            }
        }
        let file = def.defined_in.iter().next().ok_or("definition without file")?;
        let stem = file.rsplit('/').next().unwrap_or(file).trim_end_matches(".rnc");
        Ok(snake(stem))
    }

    fn def_element(&self, name: &str) -> Result<String, String> {
        match &self.s.def(name)?.pattern.kind {
            Kind::Element(NameClass::Name(n), _) => qualified(n),
            _ => Err(format!("{name} is not an element definition")),
        }
    }

    fn def_type(&mut self, name: &str) -> Result<TypeId, String> {
        if let Some(&id) = self.def_types.get(name) {
            return Ok(id);
        }
        let s = self.s;
        let def = s.def(name)?;
        let kind = self.def_kind(name)?;
        let rust = def_type_name(name);
        let owner = Owner { name: rust.clone(), module: self.module_of(name)?, path: name.to_owned() };
        let id = self.push_type(Type {
            name: rust.clone(),
            module: owner.module.clone(),
            origin: name.to_owned(),
            files: def.defined_in.iter().cloned().collect(),
            doc: def.doc.clone(),
            element: None,
            kind: TypeKind::Enum(Vec::new()),
            trace: Trace { nodes: vec![def.pattern.id], ..Trace::default() },
        });
        self.names.insert(rust);
        self.def_types.insert(name.to_owned(), id);
        let p = &def.pattern;
        let kind = match kind {
            DefKind::Enum => TypeKind::Enum(self.enum_literals(p)?),
            DefKind::Value => TypeKind::Alias(self.value_type(p, false)?),
            DefKind::Element => {
                let Kind::Element(NameClass::Name(n), content) = &p.kind else {
                    return Err(format!("{name}: expected a named element"));
                };
                self.cover(p);
                self.types[id].element = Some(qualified(n)?);
                self.content_kind(content, &owner)?
            }
            DefKind::Content => self.content_kind(p, &owner)?,
            DefKind::Fixed => return Err(format!("{name} is an attribute, not a type")),
        };
        self.types[id].kind = kind;
        Ok(id)
    }

    fn push_type(&mut self, t: Type) -> TypeId {
        self.types.push(t);
        self.types.len() - 1
    }

    /// Adds a type that has no definition of its own, making its name unique.
    /// `nodes` are the patterns it stands for, if it stands for any.
    fn new_type(&mut self, name: &str, module: &str, origin: String, kind: TypeKind, nodes: Vec<u32>) -> TypeId {
        let mut unique = name.to_owned();
        let mut n = 2;
        while self.names.contains(&unique) || self.def_names.contains(&unique) {
            unique = format!("{name}{n}");
            n += 1;
        }
        self.names.insert(unique.clone());
        let trace = Trace { nodes, ..Trace::default() };
        self.push_type(Type { name: unique, module: module.to_owned(), origin, files: Vec::new(), doc: Vec::new(), element: None, kind, trace })
    }

    fn enum_literals(&mut self, p: &'s Pattern) -> Result<Vec<Literal>, String> {
        self.cover(p);
        let items = match &p.kind {
            Kind::Choice(ps) => ps.iter().collect(),
            _ => vec![p],
        };
        let mut out = Vec::new();
        for q in items {
            let Kind::Value { value, .. } = &q.kind else {
                return Err(format!("enumeration contains {}", describe(q)));
            };
            self.cover(q);
            if out.iter().any(|l: &Literal| &l.value == value) {
                return Err(format!("duplicate literal {value:?}"));
            }
            out.push(Literal { value: value.clone(), trace: Trace { nodes: vec![q.id], ..Trace::default() } });
        }
        Ok(out)
    }

    // -- content -----------------------------------------------------------

    fn content_kind(&mut self, p: &'s Pattern, owner: &Owner) -> Result<TypeKind, String> {
        Ok(match self.lower_content(p, owner)? {
            Lowered::Content(c) => TypeKind::Struct(c),
            Lowered::Choice(v) => TypeKind::Choice(v),
        })
    }

    fn lower_content(&mut self, p: &'s Pattern, owner: &Owner) -> Result<Lowered, String> {
        if let Kind::Choice(branches) = &p.kind
            && !self.is_value(p)?
        {
            self.cover(p);
            return Ok(Lowered::Choice(self.variants(branches, owner)?));
        }
        let mut c = Content::default();
        self.accumulate(p, owner, &mut c, false)?;
        name_fields(&mut c);
        Ok(Lowered::Content(c))
    }

    /// Adds the items of a content pattern to `c`.
    fn accumulate(&mut self, p: &'s Pattern, owner: &Owner, c: &mut Content, optional: bool) -> Result<(), String> {
        match &p.kind {
            Kind::Group(ps) if !optional => {
                self.cover(p);
                for q in ps {
                    self.accumulate(q, owner, c, false)?;
                }
            }
            Kind::Attribute(NameClass::Name(n), v) => {
                if n.prefix.is_some() {
                    return Err(format!("{}: namespaced attribute {}", owner.path, n.qualified()));
                }
                self.cover(p);
                if let Kind::Value { token, value } = &v.kind {
                    if optional {
                        // Its presence would be data; no schema needs this yet.
                        return Err(format!("{}: optional literal attribute {}", owner.path, n.local));
                    }
                    self.cover(v);
                    c.fixed.push(Fixed { name: n.local.clone(), value: value.clone(), token: *token });
                } else {
                    let ty = self.value_type(v, false)?;
                    let schema = describe(v);
                    let trace = Trace { nodes: vec![p.id], ..Trace::default() };
                    c.attrs.push(Attr { field: String::new(), name: n.local.clone(), ty, required: !optional, schema, trace });
                }
            }
            Kind::Optional(q) if !optional => {
                self.cover(p);
                match &q.kind {
                    Kind::Element(..) => self.push_child(q, owner, c, Occurs::Optional)?,
                    Kind::Ref(r) if self.def_kind(r)? == DefKind::Element => {
                        self.push_child(q, owner, c, Occurs::Optional)?;
                    }
                    Kind::Attribute(..) | Kind::Ref(_) => self.accumulate(q, owner, c, true)?,
                    _ => return Err(format!("{}: unsupported optional {}", owner.path, describe(q))),
                }
            }
            Kind::ZeroOrMore(q) if !optional => {
                self.cover(p);
                match &q.kind {
                    Kind::Element(..) => self.push_child(q, owner, c, Occurs::Many)?,
                    Kind::Ref(r) if self.def_kind(r)? == DefKind::Element => self.push_child(q, owner, c, Occurs::Many)?,
                    _ => return Err(format!("{}: unsupported repetition of {}", owner.path, describe(q))),
                }
            }
            Kind::Element(..) if !optional => self.push_child(p, owner, c, Occurs::One)?,
            Kind::Ref(r) => match self.def_kind(r)? {
                DefKind::Fixed => {
                    self.cover(p);
                    let def = &self.s.def(r)?.pattern;
                    self.accumulate(def, owner, c, optional)?;
                }
                DefKind::Element if !optional => self.push_child(p, owner, c, Occurs::One)?,
                DefKind::Content if !optional => {
                    // A content definition inside other content: its items
                    // become part of this content.
                    self.cover(p);
                    let def = &self.s.def(r)?.pattern;
                    self.accumulate(def, owner, c, false)?;
                }
                DefKind::Enum | DefKind::Value if !optional => self.set_text(p, owner, c)?,
                _ => return Err(format!("{}: unsupported optional {r}", owner.path)),
            },
            Kind::Interleave(ms) if !optional => {
                self.cover(p);
                let slots = self.interleave(ms, owner)?;
                children(c, owner)?.push(Item::Interleave(slots));
            }
            Kind::Empty => self.cover(p),
            Kind::Text | Kind::Data { .. } | Kind::List(_) | Kind::Choice(_) if !optional && self.is_value(p)? => {
                self.set_text(p, owner, c)?;
            }
            _ => return Err(format!("{}: unsupported pattern {}", owner.path, describe(p))),
        }
        Ok(())
    }

    fn set_text(&mut self, p: &'s Pattern, owner: &Owner, c: &mut Content) -> Result<(), String> {
        if c.body != Body::Empty {
            return Err(format!("{}: text mixed with other content", owner.path));
        }
        let ty = self.value_type(p, false)?;
        let trace = Trace { nodes: vec![p.id], ..Trace::default() };
        c.body = Body::Text { field: String::new(), ty, schema: describe(p), trace };
        Ok(())
    }

    fn push_child(&mut self, p: &'s Pattern, owner: &Owner, c: &mut Content, occurs: Occurs) -> Result<(), String> {
        let elem = self.elem(p, owner)?;
        let trace = Trace { nodes: vec![p.id], ..Trace::default() };
        children(c, owner)?.push(Item::Elem(ElemField { field: String::new(), elem, occurs, boxed: false, trace }));
        Ok(())
    }

    fn interleave(&mut self, ms: &'s [Pattern], owner: &Owner) -> Result<Vec<Slot>, String> {
        let mut members = Vec::new();
        for m in ms {
            let (occurs, q) = match &m.kind {
                Kind::Optional(q) => (Occurs::Optional, q.as_ref()),
                Kind::ZeroOrMore(q) => (Occurs::Many, q.as_ref()),
                _ => (Occurs::One, m),
            };
            if occurs != Occurs::One {
                self.cover(m);
            }
            let is_elem = match &q.kind {
                Kind::Element(..) => true,
                Kind::Ref(r) => self.def_kind(r)? == DefKind::Element,
                _ => false,
            };
            if !is_elem {
                return Err(format!("{}: interleave member {} is not an element", owner.path, describe(m)));
            }
            members.push((occurs, self.elem(q, owner)?, q.id));
        }
        let repeated = members.iter().filter(|(o, _, _)| *o == Occurs::Many).count();
        let mut slots = Vec::new();
        let trace = |node: u32| Trace { nodes: vec![node], ..Trace::default() };
        if repeated < 2 {
            for (occurs, elem, node) in members {
                slots.push(Slot::Elem(ElemField { field: String::new(), elem, occurs, boxed: false, trace: trace(node) }));
            }
            return Ok(slots);
        }
        let mut variants: Vec<Member> = Vec::new();
        for (occurs, elem, node) in &members {
            if *occurs == Occurs::Many {
                let name = unique_name(&camel(local(&elem.name)), variants.iter().map(|v| v.name.as_str()));
                // Decided in `box_recursion`, when every type is complete.
                variants.push(Member { name, elem: elem.clone(), boxed: false, trace: trace(*node) });
            }
        }
        let names: Vec<String> = variants.iter().map(|v| format!("<{}>*", v.elem.name)).collect();
        let origin = format!("{}/({})", owner.path, names.join(" & "));
        let group = self.new_type(&format!("{}Child", owner.name), &owner.module, origin, TypeKind::Children(variants), Vec::new());
        let mut placed = false;
        for (occurs, elem, node) in members {
            if occurs != Occurs::Many {
                slots.push(Slot::Elem(ElemField { field: String::new(), elem, occurs, boxed: false, trace: trace(node) }));
            } else if !placed {
                placed = true;
                slots.push(Slot::Group { field: String::new(), ty: group });
            }
        }
        Ok(slots)
    }

    fn elem(&mut self, p: &'s Pattern, owner: &Owner) -> Result<Elem, String> {
        match &p.kind {
            Kind::Ref(r) => {
                self.cover(p);
                let name = self.def_element(r)?;
                Ok(Elem { name, fixed: Vec::new(), ty: Ty::Named(self.def_type(r)?), part: None })
            }
            Kind::Element(NameClass::Name(n), content) => {
                self.cover(p);
                let name = qualified(n)?;
                if let Some(e) = self.part_ref(&name, content)? {
                    return Ok(e);
                }
                let (fixed, ty) = self.inline(p.id, n, content, owner)?;
                Ok(Elem { name, fixed, ty, part: None })
            }
            _ => Err(format!("{}: expected an element, found {}", owner.path, describe(p))),
        }
    }

    /// `<idPkg:Spread src="…"/>` in the designmap: a reference to a part.
    fn part_ref(&mut self, name: &str, content: &'s Pattern) -> Result<Option<Elem>, String> {
        let Some(start) = self.roots.get(name).cloned() else { return Ok(None) };
        let Kind::Attribute(NameClass::Name(a), v) = &content.kind else { return Ok(None) };
        let part = match &v.kind {
            Kind::Value { value, .. } if a.local == "src" => PartRef::Fixed(value.clone()),
            Kind::Data { xsd, params } if a.local == "src" && xsd == "string" && is_part_path(params) => PartRef::Path,
            _ => return Ok(None),
        };
        self.cover_tree(content);
        Ok(Some(Elem { name: name.to_owned(), fixed: Vec::new(), ty: Ty::Named(self.def_type(&start)?), part: Some(part) }))
    }

    /// The representation of an inline element's content: collapsed to an
    /// existing type when it carries a single kind of data, else a new type.
    fn inline(&mut self, node: u32, n: &QName, content: &'s Pattern, owner: &Owner) -> Result<(Vec<Fixed>, Ty), String> {
        // Elements with one content structure share one type; elements whose
        // content differs by context are named after their owner.
        let base = camel(&n.local);
        let name = if self.multi.contains(&n.local) {
            format!("{}{base}", owner.name)
        } else if self.def_names.contains(&base) {
            format!("{base}Element")
        } else {
            base
        };
        let key = (name.clone(), content.key());
        if let Some(seen) = self.inline.get(&key).cloned() {
            // The same content again: it is represented by the first
            // occurrence's types, which now stand for this one too.
            self.cover_tree(content);
            self.aliases.entry(seen.node).or_default().push(node);
            self.alias_tree(seen.content, content);
            return Ok((seen.fixed, seen.ty));
        }
        let inner = owner.child(&name, &n.qualified());
        let result = if let Some((fixed, id)) = self.fixed_and_ref(content, &inner)? {
            (fixed, Ty::Named(id))
        } else {
            match self.lower_content(content, &inner)? {
                Lowered::Choice(vs) => {
                    let origin = inner.path.clone();
                    (Vec::new(), Ty::Named(self.new_type(&name, &owner.module, origin, TypeKind::Choice(vs), vec![node])))
                }
                Lowered::Content(c) => match collapse(&c) {
                    Some(ty) => (c.fixed, ty),
                    None => {
                        let origin = inner.path.clone();
                        (Vec::new(), Ty::Named(self.new_type(&name, &owner.module, origin, TypeKind::Struct(c), vec![node])))
                    }
                },
            }
        };
        self.inline.insert(key, Seen { fixed: result.0.clone(), ty: result.1.clone(), node, content });
        Ok(result)
    }

    /// Records that the nodes of `again` stand for the same items as the
    /// corresponding nodes of `first`, a structurally equal pattern.
    fn alias_tree(&mut self, first: &'s Pattern, again: &Pattern) {
        let mut a = Vec::new();
        let mut b = Vec::new();
        first.walk(&mut |p| a.push(p.id));
        again.walk(&mut |p| b.push(p.id));
        for (x, y) in a.into_iter().zip(b) {
            self.aliases.entry(x).or_default().push(y);
        }
    }

    /// Every node a trace stands for: its own and their aliases.
    fn expand(&self, nodes: &[u32]) -> Vec<u32> {
        let mut out: Vec<u32> = Vec::new();
        let mut stack: Vec<u32> = nodes.to_vec();
        while let Some(n) = stack.pop() {
            if !out.contains(&n) {
                out.push(n);
                stack.extend(self.aliases.get(&n).into_iter().flatten());
            }
        }
        out.sort_unstable();
        out
    }

    fn expand_traces(&mut self) {
        let mut types = std::mem::take(&mut self.types);
        for t in &mut types {
            for_each_trace(t, &mut |trace| trace.nodes = self.expand(&trace.nodes));
        }
        self.types = types;
    }

    /// Content that is literal attributes plus one content definition: the
    /// definition's type with those attributes.
    fn fixed_and_ref(&mut self, p: &'s Pattern, owner: &Owner) -> Result<Option<(Vec<Fixed>, TypeId)>, String> {
        let mut items = Vec::new();
        let mut groups = Vec::new();
        flatten(p, &mut items, &mut groups);
        let mut fixed = Vec::new();
        let mut target = None;
        for q in &items {
            match &q.kind {
                Kind::Ref(r) if self.def_kind(r)? == DefKind::Fixed => fixed.push(*q),
                Kind::Attribute(_, v) if matches!(v.kind, Kind::Value { .. }) => fixed.push(*q),
                Kind::Ref(r) if self.def_kind(r)? == DefKind::Content && target.is_none() => target = Some((*q, r)),
                _ => return Ok(None),
            }
        }
        let Some((reference, name)) = target else { return Ok(None) };
        for g in groups {
            self.cover(g);
        }
        self.cover(reference);
        let mut c = Content::default();
        for q in fixed {
            self.accumulate(q, owner, &mut c, false)?;
        }
        Ok(Some((c.fixed, self.def_type(name)?)))
    }

    fn variants(&mut self, branches: &'s [Pattern], owner: &Owner) -> Result<Vec<Variant>, String> {
        let mut out: Vec<Variant> = Vec::new();
        for b in branches {
            let schema = describe(b);
            let (fixed, ty, base) = if let Some((fixed, id)) = self.fixed_and_ref(b, owner)? {
                let base = type_tag(&fixed).map_or_else(|| self.types[id].name.clone(), camel);
                (fixed, Ty::Named(id), base)
            } else {
                let mut c = Content::default();
                self.accumulate(b, owner, &mut c, false)?;
                name_fields(&mut c);
                let base = match type_tag(&c.fixed) {
                    Some(tag) => camel(tag),
                    None => first_name(&c).unwrap_or_else(|| "Value".to_owned()),
                };
                match collapse(&c) {
                    Some(ty) => (c.fixed, ty, base),
                    None => {
                        let origin = format!("{}/({schema})", owner.path);
                        let id = self.new_type(&format!("{}{base}", owner.name), &owner.module, origin, TypeKind::Struct(c), vec![b.id]);
                        (Vec::new(), Ty::Named(id), base)
                    }
                }
            };
            // An enumeration branch is named after its enumeration.
            let base = match (&ty, type_tag(&fixed)) {
                (Ty::Value(ValueType::Named(id)), Some("enumeration")) => self.types[*id].name.clone(),
                _ => base,
            };
            let name = unique_name(&base, out.iter().map(|v| v.name.as_str()));
            out.push(Variant { name, fixed, ty, boxed: false, schema, trace: Trace { nodes: vec![b.id], ..Trace::default() } });
        }
        Ok(out)
    }

    // -- values ------------------------------------------------------------

    fn value_type(&mut self, p: &'s Pattern, in_list: bool) -> Result<ValueType, String> {
        self.cover(p);
        Ok(match &p.kind {
            Kind::Text if !in_list => ValueType::String,
            Kind::Data { xsd, params } => match (xsd.as_str(), params.as_slice()) {
                ("string", []) if in_list => ValueType::Token,
                ("string", []) => ValueType::String,
                ("string", ps) if !in_list && is_part_path(ps) => ValueType::PartPath,
                ("boolean", []) => ValueType::Bool,
                ("int", []) => ValueType::Int,
                ("short", []) => ValueType::Short,
                ("double", []) => ValueType::Double,
                ("dateTime", []) => ValueType::DateTime,
                (t @ ("int" | "short" | "double"), [(a, min), (b, max)]) if a == "minInclusive" && b == "maxInclusive" => {
                    let base = match t {
                        "int" => Scalar::Int,
                        "short" => Scalar::Short,
                        _ => Scalar::Double,
                    };
                    ValueType::Named(self.ranged(base, min, max)?)
                }
                _ => return Err(format!("unsupported datatype {}", describe(p))),
            },
            Kind::Ref(r) => match self.def_kind(r)? {
                DefKind::Enum | DefKind::Value => ValueType::Named(self.def_type(r)?),
                _ => return Err(format!("{r} is not a value")),
            },
            Kind::Choice(ps) => {
                let mut members = Vec::new();
                for q in ps {
                    match self.value_type(q, in_list)? {
                        ValueType::Named(id) if matches!(self.types[id].kind, TypeKind::Enum(_)) => members.push(id),
                        _ => return Err(format!("unsupported value choice {}", describe(p))),
                    }
                }
                ValueType::Named(self.union(members))
            }
            Kind::List(inner) if !in_list => self.list_type(inner)?,
            _ => return Err(format!("unsupported value {}", describe(p))),
        })
    }

    fn list_type(&mut self, p: &'s Pattern) -> Result<ValueType, String> {
        Ok(match &p.kind {
            Kind::ZeroOrMore(q) => {
                self.cover(p);
                ValueType::List(Box::new(self.value_type(q, true)?))
            }
            Kind::Group(qs) => {
                self.cover(p);
                let mut items = Vec::new();
                for q in qs {
                    items.push(self.value_type(q, true)?);
                }
                match items.first() {
                    Some(first) if items.iter().all(|i| i == first) => ValueType::Array(Box::new(first.clone()), items.len()),
                    _ => return Err(format!("unsupported list of different types {}", describe(p))),
                }
            }
            _ => ValueType::Array(Box::new(self.value_type(p, true)?), 1),
        })
    }

    fn ranged(&mut self, base: Scalar, min: &str, max: &str) -> Result<TypeId, String> {
        let key = (base, min.to_owned(), max.to_owned());
        if let Some(&id) = self.ranged.get(&key) {
            return Ok(id);
        }
        let valid = match base {
            Scalar::Double => min.parse::<f64>().is_ok() && max.parse::<f64>().is_ok(),
            Scalar::Int => min.parse::<i32>().is_ok() && max.parse::<i32>().is_ok(),
            Scalar::Short => min.parse::<i16>().is_ok() && max.parse::<i16>().is_ok(),
        };
        if !valid {
            return Err(format!("invalid range {min}..{max}"));
        }
        let prefix = match base {
            Scalar::Int => "Int",
            Scalar::Short => "Short",
            Scalar::Double => "Double",
        };
        let bound = |s: &str| s.replace('-', "Minus").replace('.', "p");
        let name = format!("{prefix}{}To{}", bound(min), bound(max));
        let origin = format!("xsd:{} {{minInclusive=\"{min}\" maxInclusive=\"{max}\"}}", prefix.to_lowercase());
        let kind = TypeKind::Ranged { base, min: min.to_owned(), max: max.to_owned() };
        let id = self.new_type(&name, "datatype", origin, kind, Vec::new());
        self.ranged.insert(key, id);
        Ok(id)
    }

    fn union(&mut self, members: Vec<TypeId>) -> TypeId {
        if let Some(&id) = self.unions.get(&members) {
            return id;
        }
        let names: Vec<&str> = members.iter().map(|&m| self.types[m].name.as_str()).collect();
        let name = names.join("Or");
        let origin = members.iter().map(|&m| self.types[m].origin.as_str()).collect::<Vec<_>>().join(" | ");
        let id = self.new_type(&name, "datatype", origin, TypeKind::Union(members.clone()), Vec::new());
        self.unions.insert(members, id);
        id
    }

    // -- recursion ---------------------------------------------------------

    /// Boxes the fields and variants through which a type contains itself,
    /// and the element payloads of interleaved-children enums (so that a
    /// list item costs a pointer, not the largest member).
    fn box_recursion(&mut self) {
        let named_content: Vec<bool> = self.types.iter().map(|t| matches!(t.kind, TypeKind::Struct(_) | TypeKind::Choice(_))).collect();
        for t in &mut self.types {
            if let TypeKind::Children(members) = &mut t.kind {
                for m in members {
                    m.boxed = matches!(m.elem.ty, Ty::Named(id) if named_content.get(id) == Some(&true));
                }
            }
        }
        let n = self.types.len();
        let mut edges: Vec<Vec<TypeId>> = vec![Vec::new(); n];
        for (id, t) in self.types.iter().enumerate() {
            match &t.kind {
                TypeKind::Struct(c) => each_field(c, &mut |f| {
                    if let Some(target) = inline_target(&f.elem.ty, f.occurs) {
                        edges[id].push(target);
                    }
                }),
                TypeKind::Choice(vs) => {
                    for v in vs {
                        if let Some(target) = inline_target(&v.ty, Occurs::One) {
                            edges[id].push(target);
                        }
                    }
                }
                _ => {}
            }
        }
        let scc = strongly_connected(&edges);
        for id in 0..n {
            let same = |target: Option<TypeId>| target.is_some_and(|t| scc[t] == scc[id]);
            match &mut self.types[id].kind {
                TypeKind::Struct(c) => each_field_mut(c, &mut |f| {
                    if same(inline_target(&f.elem.ty, f.occurs)) {
                        f.boxed = true;
                    }
                }),
                TypeKind::Choice(vs) => {
                    for v in vs {
                        if same(inline_target(&v.ty, Occurs::One)) {
                            v.boxed = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Calls `f` with the trace of the type and of each of its fields and variants.
pub fn for_each_trace(t: &mut Type, f: &mut impl FnMut(&mut Trace)) {
    f(&mut t.trace);
    match &mut t.kind {
        TypeKind::Struct(c) => {
            c.attrs.iter_mut().for_each(|a| f(&mut a.trace));
            if let Body::Text { trace, .. } = &mut c.body {
                f(trace);
            }
            each_field_mut(c, &mut |field| f(&mut field.trace));
        }
        TypeKind::Choice(vs) => vs.iter_mut().for_each(|v| f(&mut v.trace)),
        TypeKind::Enum(ls) => ls.iter_mut().for_each(|l| f(&mut l.trace)),
        TypeKind::Children(ms) => ms.iter_mut().for_each(|m| f(&mut m.trace)),
        TypeKind::Union(_) | TypeKind::Ranged { .. } | TypeKind::Alias(_) => {}
    }
}

/// The named type a field holds without indirection, if any.
fn inline_target(ty: &Ty, occurs: Occurs) -> Option<TypeId> {
    if occurs == Occurs::Many {
        return None;
    }
    match ty {
        Ty::Named(id) => Some(*id),
        Ty::Wrap(e) => inline_target(&e.ty, Occurs::One),
        _ => None,
    }
}

pub fn each_field(c: &Content, f: &mut impl FnMut(&ElemField)) {
    if let Body::Children(items) = &c.body {
        for i in items {
            match i {
                Item::Elem(e) => f(e),
                Item::Interleave(slots) => {
                    for s in slots {
                        if let Slot::Elem(e) = s {
                            f(e);
                        }
                    }
                }
            }
        }
    }
}

pub fn each_field_mut(c: &mut Content, f: &mut impl FnMut(&mut ElemField)) {
    if let Body::Children(items) = &mut c.body {
        for i in items {
            match i {
                Item::Elem(e) => f(e),
                Item::Interleave(slots) => {
                    for s in slots {
                        if let Slot::Elem(e) = s {
                            f(e);
                        }
                    }
                }
            }
        }
    }
}

/// Tarjan's algorithm; returns the component of each node.
fn strongly_connected(edges: &[Vec<usize>]) -> Vec<usize> {
    struct State<'a> {
        edges: &'a [Vec<usize>],
        index: Vec<Option<usize>>,
        low: Vec<usize>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        comp: Vec<usize>,
        next: usize,
        comps: usize,
    }
    fn visit(s: &mut State, v: usize) {
        s.index[v] = Some(s.next);
        s.low[v] = s.next;
        s.next += 1;
        s.stack.push(v);
        s.on_stack[v] = true;
        for &w in &s.edges[v] {
            match s.index[w] {
                None => {
                    visit(s, w);
                    s.low[v] = s.low[v].min(s.low[w]);
                }
                Some(i) if s.on_stack[w] => s.low[v] = s.low[v].min(i),
                Some(_) => {}
            }
        }
        if Some(s.low[v]) == s.index[v] {
            while let Some(w) = s.stack.pop() {
                s.on_stack[w] = false;
                s.comp[w] = s.comps;
                if w == v {
                    break;
                }
            }
            s.comps += 1;
        }
    }
    let n = edges.len();
    let mut s =
        State { edges, index: vec![None; n], low: vec![0; n], on_stack: vec![false; n], stack: Vec::new(), comp: vec![0; n], next: 0, comps: 0 };
    for v in 0..n {
        if s.index[v].is_none() {
            visit(&mut s, v);
        }
    }
    // A single node is only recursive with a self edge.
    let mut comp = s.comp;
    let mut sizes = vec![0usize; s.comps];
    for &c in &comp {
        sizes[c] += 1;
    }
    let mut next = s.comps;
    for v in 0..n {
        if sizes[comp[v]] == 1 && !edges[v].contains(&v) {
            comp[v] = next;
            next += 1;
        }
    }
    comp
}

// -- helpers ----------------------------------------------------------------

fn children<'c>(c: &'c mut Content, owner: &Owner) -> Result<&'c mut Vec<Item>, String> {
    if c.body == Body::Empty {
        c.body = Body::Children(Vec::new());
    }
    match &mut c.body {
        Body::Children(items) => Ok(items),
        _ => Err(format!("{}: elements mixed with text", owner.path)),
    }
}

/// Collapses content with a single kind of data into that data's type.
fn collapse(c: &Content) -> Option<Ty> {
    if !c.attrs.is_empty() {
        return None;
    }
    match &c.body {
        Body::Empty => Some(Ty::Unit),
        Body::Text { ty, .. } => Some(Ty::Value(ty.clone())),
        Body::Children(items) => {
            let elems: Vec<&ElemField> = items
                .iter()
                .map(|i| match i {
                    Item::Elem(f) => Some(f),
                    Item::Interleave(_) => None,
                })
                .collect::<Option<_>>()?;
            match elems.as_slice() {
                [f] if f.occurs == Occurs::Many => Some(Ty::Vec(Box::new(f.elem.clone()))),
                [f] if f.occurs == Occurs::One => Some(Ty::Wrap(Box::new(f.elem.clone()))),
                [f, rest @ ..] if !rest.is_empty() && elems.iter().all(|g| g.occurs == Occurs::One && g.elem == f.elem) => {
                    Some(Ty::Array(Box::new(f.elem.clone()), elems.len()))
                }
                _ => None,
            }
        }
    }
}

/// Assigns unique snake_case field names.
fn name_fields(c: &mut Content) {
    let mut used: Vec<String> = Vec::new();
    let mut take = |base: String| {
        let name = unique_snake(&base, &used);
        used.push(name.clone());
        name
    };
    for a in &mut c.attrs {
        a.field = take(snake(&a.name));
    }
    match &mut c.body {
        Body::Text { field, .. } => *field = take("value".to_owned()),
        Body::Children(items) => {
            for i in items {
                match i {
                    Item::Elem(f) => f.field = take(snake(local(&f.elem.name))),
                    Item::Interleave(slots) => {
                        for s in slots {
                            match s {
                                Slot::Elem(f) => f.field = take(snake(local(&f.elem.name))),
                                Slot::Group { field, .. } => *field = take("children".to_owned()),
                            }
                        }
                    }
                }
            }
        }
        Body::Empty => {}
    }
}

fn unique_snake(base: &str, used: &[String]) -> String {
    let mut name = base.to_owned();
    let mut n = 2;
    while used.contains(&name) {
        name = format!("{}_{n}", base.trim_end_matches('_'));
        n += 1;
    }
    name
}

fn unique_name<'a>(base: &str, used: impl Iterator<Item = &'a str> + Clone) -> String {
    let mut name = base.to_owned();
    let mut n = 2;
    while used.clone().any(|u| u == name) {
        name = format!("{base}{n}");
        n += 1;
    }
    name
}

fn type_tag(fixed: &[Fixed]) -> Option<&str> {
    fixed.iter().find(|f| f.name == "type").map(|f| f.value.as_str())
}

/// The name of an untagged choice branch: its only attribute or element, or
/// `Record` for several.
fn first_name(c: &Content) -> Option<String> {
    let items = match &c.body {
        Body::Children(items) => items.as_slice(),
        _ => &[],
    };
    match (c.attrs.as_slice(), items) {
        ([a], []) => Some(camel(&a.name)),
        ([], [Item::Elem(f)]) => Some(camel(local(&f.elem.name))),
        ([], []) => None,
        _ => Some("Record".to_owned()),
    }
}

fn flatten<'p>(p: &'p Pattern, items: &mut Vec<&'p Pattern>, groups: &mut Vec<&'p Pattern>) {
    if let Kind::Group(ps) = &p.kind {
        groups.push(p);
        for q in ps {
            flatten(q, items, groups);
        }
    } else {
        items.push(p);
    }
}

fn is_part_path(params: &[(String, String)]) -> bool {
    matches!(params, [(k, v)] if k == "pattern" && v == r".*\.xml")
}

pub fn local(qualified: &str) -> &str {
    qualified.rsplit(':').next().unwrap_or(qualified)
}

fn qualified(n: &QName) -> Result<String, String> {
    match (&n.prefix, n.ns.as_str()) {
        (None, "") => Ok(n.local.clone()),
        (Some(p), PACKAGING_NS) if p == "idPkg" => Ok(format!("idPkg:{}", n.local)),
        _ => Err(format!("unsupported namespace for {}: {:?}", n.qualified(), n.ns)),
    }
}

/// `TextFrame_Object` → `TextFrame`, `Story_File` → `StoryFile`,
/// `XMLInstruction_Object1` → `XmlInstruction1`.
fn def_type_name(def: &str) -> String {
    match def.rsplit_once('_') {
        Some((base, "File")) => format!("{}File", camel(base)),
        Some((base, suffix)) if suffix.strip_prefix("Object").is_some_and(|d| d.chars().all(|c| c.is_ascii_digit())) => {
            format!("{}{}", camel(base), &suffix["Object".len()..])
        }
        Some((base, "TypeDef" | "EnumValue")) => camel(base),
        _ => camel(def),
    }
}

/// A pattern in compact syntax, for documentation and errors.
pub fn describe(p: &Pattern) -> String {
    let join = |ps: &[Pattern], sep: &str| ps.iter().map(describe_inner).collect::<Vec<_>>().join(sep);
    match &p.kind {
        Kind::Element(n, _) => format!("element {}", name_class(n)),
        Kind::Attribute(n, _) => format!("attribute {}", name_class(n)),
        Kind::Group(ps) => join(ps, ", "),
        Kind::Interleave(ps) => join(ps, " & "),
        Kind::Choice(ps) => join(ps, " | "),
        Kind::Optional(q) => format!("{}?", describe_inner(q)),
        Kind::ZeroOrMore(q) => format!("{}*", describe_inner(q)),
        Kind::OneOrMore(q) => format!("{}+", describe_inner(q)),
        Kind::List(q) => format!("list {{ {} }}", describe(q)),
        Kind::Ref(r) => r.clone(),
        Kind::Text => "text".to_owned(),
        Kind::Empty => "empty".to_owned(),
        Kind::NotAllowed => "notAllowed".to_owned(),
        Kind::Value { token, value } => format!("{}\"{value}\"", if *token { "" } else { "string " }),
        Kind::Data { xsd, params } if params.is_empty() => format!("xsd:{xsd}"),
        Kind::Data { xsd, params } => {
            let ps: Vec<String> = params.iter().map(|(k, v)| format!("{k}=\"{v}\"")).collect();
            format!("xsd:{xsd} {{{}}}", ps.join(" "))
        }
    }
}

fn describe_inner(p: &Pattern) -> String {
    match &p.kind {
        Kind::Group(_) | Kind::Interleave(_) | Kind::Choice(_) => format!("({})", describe(p)),
        _ => describe(p),
    }
}

fn name_class(n: &NameClass) -> String {
    match n {
        NameClass::Name(q) => q.qualified(),
        NameClass::Any => "*".to_owned(),
    }
}
