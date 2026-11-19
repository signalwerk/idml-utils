//! One typed vocabulary across snapshots; views borrow lossless arena nodes.
use crate::{
    history, names,
    rnc::{Ids, Kind, NameClass, Pattern},
    schema::{Files, Grammar},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    path::Path,
};
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Spec {
    Scalar(String),
    Literal(BTreeSet<String>),
    Enumeration(String),
    List(Box<Spec>),
    Union(Vec<Spec>),
}
fn specs(p: &Pattern, defs: &BTreeMap<&str, (&str, &crate::rnc::Define)>, seen: &mut BTreeSet<String>) -> Spec {
    match &p.kind {
        Kind::Data { xsd, .. } => Spec::Scalar(xsd.clone()),
        Kind::Text => Spec::Scalar("string".into()),
        Kind::Value { value, .. } => Spec::Literal(BTreeSet::from([value.clone()])),
        Kind::Ref(n) if n.ends_with("_EnumValue") => Spec::Enumeration(n.clone()),
        Kind::Ref(n) if seen.insert(n.clone()) => {
            let s = defs.get(n.as_str()).map_or(Spec::Scalar("string".into()), |(_, d)| specs(&d.pattern, defs, seen));
            seen.remove(n);
            s
        }
        Kind::List(p) => {
            let mut members = Vec::new();
            let mut pending = vec![&**p];
            while let Some(p) = pending.pop() {
                match &p.kind {
                    Kind::Group(ps) | Kind::Choice(ps) => pending.extend(ps),
                    Kind::Optional(p) | Kind::ZeroOrMore(p) | Kind::OneOrMore(p) => pending.push(p),
                    _ => members.push(specs(p, defs, seen)),
                }
            }
            Spec::List(Box::new(merge(members)))
        }
        Kind::Choice(ps) => merge(ps.iter().map(|p| specs(p, defs, seen)).collect()),
        _ => Spec::Scalar("string".into()),
    }
}
fn merge(items: Vec<Spec>) -> Spec {
    let mut set = BTreeSet::new();
    let mut literals = BTreeSet::new();
    for i in items {
        match i {
            Spec::Union(xs) => set.extend(xs),
            Spec::Literal(xs) => literals.extend(xs),
            x => {
                set.insert(x);
            }
        }
    }
    if set.contains(&Spec::Scalar("string".into())) {
        return Spec::Scalar("string".into());
    }
    if set.contains(&Spec::Scalar("double".into())) {
        set.remove(&Spec::Scalar("int".into()));
        set.remove(&Spec::Scalar("short".into()));
    } else if set.contains(&Spec::Scalar("int".into())) {
        set.remove(&Spec::Scalar("short".into()));
    }
    if !literals.is_empty() {
        set.insert(Spec::Literal(literals));
    }
    match set.len() {
        0 => Spec::Scalar("string".into()),
        1 => set.into_iter().next().unwrap_or(Spec::Scalar("string".into())),
        _ => Spec::Union(set.into_iter().collect()),
    }
}
type Vocabulary = BTreeMap<(String, String), BTreeMap<String, Vec<Spec>>>;
fn attributes(p: &Pattern, g: &Grammar<'_>, out: &mut BTreeMap<String, Vec<Spec>>, seen: &mut BTreeSet<String>) {
    match &p.kind {
        Kind::Attribute(NameClass::Name(n), v) => {
            out.entry(n.qualified()).or_default().push(specs(v, &g.defs, &mut BTreeSet::new()));
        }
        Kind::Group(ps) | Kind::Interleave(ps) | Kind::Choice(ps) => {
            for p in ps {
                attributes(p, g, out, seen)
            }
        }
        Kind::Optional(p) | Kind::ZeroOrMore(p) | Kind::OneOrMore(p) => attributes(p, g, out, seen),
        Kind::Ref(n) if seen.insert(n.clone()) => {
            if let Some((_, d)) = g.defs.get(n.as_str()) {
                attributes(&d.pattern, g, out, seen)
            }
            seen.remove(n);
        }
        _ => {}
    }
}
struct Types {
    out: String,
    enums: BTreeMap<String, BTreeSet<String>>,
    emitted: BTreeMap<Spec, String>,
    names: BTreeSet<String>,
}
impl Types {
    fn ty(&mut self, s: &Spec, hint: &str) -> String {
        match s {
            Spec::Scalar(t) => {
                return match t.as_str() {
                    "boolean" => "bool",
                    "double" => "f64",
                    "int" => "i32",
                    "short" => "i16",
                    "dateTime" => "crate::DateTime",
                    "token" => "crate::Token",
                    _ => "std::string::String",
                }
                .into();
            }
            Spec::List(t) => {
                return format!(
                    "Vec<{}>",
                    self.ty(&if **t == Spec::Scalar("string".into()) { Spec::Scalar("token".into()) } else { *t.clone() }, hint)
                );
            }
            _ => {}
        }
        if let Some(t) = self.emitted.get(s) {
            return format!("values::{t}");
        }
        let base = match s {
            Spec::Enumeration(n) => names::camel(n.trim_end_matches("_EnumValue")),
            _ => hint.into(),
        };
        let mut name = base.clone();
        let mut suffix = 2;
        while !self.names.insert(name.clone()) {
            name = format!("{base}{suffix}");
            suffix += 1;
        }
        self.emitted.insert(s.clone(), name.clone());
        let literals = match s {
            Spec::Literal(xs) => Some(xs.clone()),
            Spec::Enumeration(n) => Some(self.enums.get(n).cloned().unwrap_or_default()),
            _ => None,
        };
        if let Some(literals) = literals {
            let _ = writeln!(self.out, "value_enum! {{ {name} {{");
            let mut variants = BTreeSet::new();
            for v in literals {
                let base = names::camel(&v);
                let mut variant = if base.is_empty() { "Empty".into() } else { base };
                while !variants.insert(variant.clone()) {
                    variant.push('_');
                }
                let _ = writeln!(self.out, "{variant} = {v:?},");
            }
            self.out.push_str("} }\n");
        } else if let Spec::Union(members) = s {
            let mut types = Vec::new();
            for (i, m) in members.iter().enumerate() {
                types.push(self.ty(m, &format!("{name}Value{}", i + 1)));
            }
            let _ = writeln!(self.out, "#[derive(Debug, Clone, PartialEq)] pub enum {name} {{");
            for (i, t) in types.iter().enumerate() {
                let _ = writeln!(self.out, "Value{}({}),", i + 1, t.replace("values::", ""));
            }
            self.out.push_str("}\n");
            let _ = writeln!(self.out, "impl crate::value::Value for {name} {{ fn parse(text: &str) -> Result<Self, crate::ValueError> {{");
            for (i, t) in types.iter().enumerate() {
                let _ = writeln!(
                    self.out,
                    "if let Ok(v) = <{} as crate::value::Value>::parse(text) {{ return Ok(Self::Value{}(v)); }}",
                    t.replace("values::", ""),
                    i + 1
                );
            }
            let _ = writeln!(
                self.out,
                "Err(crate::ValueError::new(format!(\"unrecognized {name} value: {{text:?}}\"))) }} fn format(&self, out: &mut String) {{ match self {{"
            );
            for (i, _) in types.iter().enumerate() {
                let _ = writeln!(self.out, "Self::Value{}(v) => crate::value::Value::format(v,out),", i + 1);
            }
            self.out.push_str("} } }\n");
        }
        format!("values::{name}")
    }
}
pub fn rust(root: &Path) -> Result<String, String> {
    let mut vocabulary = Vocabulary::new();
    let mut enums = BTreeMap::<String, BTreeSet<String>>::new();
    for s in history::snapshots(root)? {
        let mut ids = Ids::default();
        let mut files = Files::read(&root.join(&s.version.dir), &mut ids)?;
        files.apply_amendments(&mut ids)?;
        for g in files.grammars()? {
            for (name, (_, d)) in &g.defs {
                if name.ends_with("_EnumValue") {
                    let mut pending = vec![&d.pattern];
                    let mut seen = BTreeSet::new();
                    while let Some(pattern) = pending.pop() {
                        pattern.walk(&mut |p| match &p.kind {
                            Kind::Value { value, .. } => {
                                enums.entry((*name).into()).or_default().insert(value.clone());
                            }
                            Kind::Ref(reference) if seen.insert(reference.clone()) => {
                                if let Some((_, definition)) = g.defs.get(reference.as_str()) {
                                    pending.push(&definition.pattern);
                                }
                            }
                            _ => {}
                        });
                    }
                }
                d.pattern.walk(&mut |p| {
                    if let Kind::Element(NameClass::Name(n), body) = &p.kind {
                        attributes(body, &g, vocabulary.entry((n.ns.clone(), n.local.clone())).or_default(), &mut BTreeSet::new());
                    }
                });
            }
        }
    }
    let mut types = Types { out: String::new(), enums, emitted: BTreeMap::new(), names: BTreeSet::new() };
    let mut out = String::from(crate::emit::HEADER);
    out.push_str("//! Shared typed views/builders from the union of all available snapshots.\nuse crate::view::{ElementType, View, ViewMut, Builder};\nuse crate::{Field, Error};\n");
    let mut marker_names = BTreeSet::new();
    for ((ns, local), attrs) in vocabulary {
        let base = format!("{}{}", names::camel(&local), if ns == crate::lower::PACKAGING_NS { "File" } else { "" });
        let mut marker = base.clone();
        while !marker_names.insert(marker.clone()) {
            marker.push('_');
        }
        let empty = attrs.is_empty();
        let text_arg = if empty { "_text" } else { "text" };
        let match_open = if empty { "" } else { "match name {" };
        let _ = writeln!(
            out,
            "#[derive(Debug, Clone, Copy)] pub struct {marker};\nimpl ElementType for {marker} {{ const NAMESPACE: &'static str = {ns:?}; const LOCAL: &'static str = {local:?}; fn parse_attribute(name: &str, {text_arg}: &str) -> Result<(), crate::ValueError> {{\n{match_open}"
        );
        let mut fields = Vec::new();
        let mut used: BTreeSet<String> = [
            "node_id",
            "xml_tree",
            "attribute",
            "children",
            "descendants",
            "text",
            "as_view",
            "set_checked",
            "set_attribute",
            "remove_attribute",
            "set_text",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        for (name, specs) in attrs {
            let spec = merge(specs);
            let ty = types.ty(&spec, &format!("{marker}{}Value", names::camel(&name)));
            let mut field = names::snake(&name);
            while used.contains(&field) || used.contains(&format!("set_{field}")) {
                field.push('_');
            }
            used.insert(field.clone());
            used.insert(format!("set_{field}"));
            let _ = writeln!(out, "{name:?} => <{ty} as crate::value::Value>::parse(text).map(|_|()),");
            fields.push((name, field, ty));
        }
        if empty {
            out.push_str("Err(crate::ValueError::new(format!(\"unknown attribute {name}\"))) } }\n");
        } else {
            out.push_str("_ => Err(crate::ValueError::new(format!(\"unknown attribute {name}\"))), } } }\n");
        }
        let _ = writeln!(out, "impl<'a> View<'a,{marker}> {{");
        for (name, field, ty) in &fields {
            let _ = writeln!(out, "pub fn {field}(&self) -> Field<'a,{ty}> {{ self.attribute({name:?}) }}");
        }
        out.push_str("}\n");
        let _ = writeln!(out, "impl ViewMut<'_,{marker}> {{");
        for (name, field, ty) in &fields {
            let _ = writeln!(out, "pub fn set_{field}(&mut self, value: &{ty}) -> Result<(),Error> {{ self.set_attribute({name:?},value) }}");
        }
        out.push_str("}\n");
        let _ = writeln!(out, "impl Builder<{marker}> {{");
        for (name, field, ty) in &fields {
            let _ = writeln!(out, "pub fn set_{field}(&mut self, value: &{ty}) -> Result<(),Error> {{ self.assign_value({name:?},value) }}");
        }
        out.push_str("}\n");
    }
    out.push_str("pub mod values { use crate::value::value_enum;\n");
    out.push_str(&types.out);
    out.push_str("}\n");
    Ok(out)
}
