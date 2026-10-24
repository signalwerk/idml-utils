//! Random schema-valid XML, generated from the grammar alone.
//!
//! This is the test oracle for the generated reader and writer: it walks the
//! RELAX NG patterns, not the model, so a pattern the model got wrong shows
//! up as a round-trip difference. It also records which pattern nodes it
//! exercised, so tests can require that every attribute, element, choice
//! branch and enumeration literal was exercised.
//!
//! In [`Style::Full`], the first occurrence of each definition in a document
//! has every optional attribute and child and one of each repeated child;
//! later occurrences have only what is required. Choice branches and
//! enumeration literals not exercised yet are preferred. [`Style::Random`]
//! makes smaller documents with random choices; [`Style::Minimal`] only what
//! is required.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use crate::rnc::{Kind, NameClass, Pattern, QName};
use crate::schema::{Entry, Schema};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Full,
    Random,
    /// Only what is required.
    Minimal,
}

/// How lexical values are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lexical {
    /// The forms the `idml` writer produces, and interleaves in the order it
    /// writes them, so a round trip must reproduce the document.
    Canonical,
    /// Any valid form (`1.0`, ` +7 `, `1` for true) and any interleave order.
    Any,
}

pub struct Generator<'s> {
    schema: &'s Schema,
    rng: u64,
    style: Style,
    lexical: Lexical,
    max_depth: usize,
    cost: HashMap<String, u64>,
    rotation: HashMap<u32, usize>,
    /// Definitions and inline elements already generated in full.
    visited: HashSet<String>,
    covered: HashSet<u32>,
    paths: usize,
    in_attribute: bool,
    /// Elements in the current document, to bound random documents.
    elements: usize,
    /// The tracked pattern ids below each choice branch, following
    /// references.
    below: HashMap<u32, Vec<u32>>,
}

/// Random documents switch to minimal content beyond this many elements;
/// without a bound they grow exponentially with depth.
const RANDOM_ELEMENTS: usize = 2000;

/// A generated element.
#[derive(Debug, Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Child>,
}

#[derive(Debug)]
enum Child {
    Elem(Node),
    Text(String),
}

impl<'s> Generator<'s> {
    pub fn new(schema: &'s Schema, seed: u64, style: Style, lexical: Lexical) -> Self {
        let mut g = Generator {
            schema,
            rng: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            style,
            lexical,
            max_depth: if style == Style::Full { 30 } else { 9 },
            cost: HashMap::new(),
            rotation: HashMap::new(),
            visited: HashSet::new(),
            covered: HashSet::new(),
            paths: 0,
            in_attribute: false,
            elements: 0,
            below: HashMap::new(),
        };
        g.compute_costs();
        g
    }

    pub fn style(&self) -> Style {
        self.style
    }

    pub fn set_style(&mut self, style: Style, lexical: Lexical) {
        self.style = style;
        self.lexical = lexical;
        self.max_depth = if style == Style::Full { 30 } else { 9 };
    }

    /// A complete XML document for a part grammar.
    pub fn document(&mut self, entry: &Entry) -> Result<String, String> {
        self.visited.clear();
        self.elements = 0;
        let def = self.schema.def(&entry.start)?;
        let mut root = Node::default();
        self.covered.insert(def.pattern.id);
        self.generate(&def.pattern, &mut root, 0, true)?;
        let Some(Child::Elem(mut node)) = root.children.pop() else {
            return Err(format!("{}: start is not an element", entry.file));
        };
        node.attrs.insert(0, ("xmlns:idPkg".to_owned(), crate::lower::PACKAGING_NS.to_owned()));
        let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
        write_node(&node, &mut out);
        Ok(out)
    }

    /// The attribute, element, value and choice-branch patterns reachable
    /// from the part grammars that no document exercised so far, as
    /// (definition, pattern).
    pub fn uncovered(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for name in &self.schema.reachable {
            let Ok(def) = self.schema.def(name) else { continue };
            def.pattern.walk(&mut |p| {
                let tracked = match &p.kind {
                    Kind::Attribute(..) | Kind::Element(..) | Kind::Value { .. } => true,
                    Kind::Choice(branches) => {
                        for b in branches.iter().filter(|b| !self.covered.contains(&b.id)) {
                            out.push((name.clone(), format!("branch {}", crate::lower::describe(b))));
                        }
                        false
                    }
                    _ => false,
                };
                if tracked && !self.covered.contains(&p.id) {
                    out.push((name.clone(), crate::lower::describe(p)));
                }
            });
        }
        out
    }

    // -- randomness ------------------------------------------------------------

    fn next(&mut self) -> u64 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// A branch not exercised yet, else one leading to something not
    /// exercised yet, else the next one in rotation.
    fn pick_branch(&mut self, id: u32, branches: &[Pattern]) -> usize {
        if let Some(i) = branches.iter().position(|b| !self.covered.contains(&b.id)) {
            return i;
        }
        for (i, b) in branches.iter().enumerate() {
            if !self.below.contains_key(&b.id) {
                let ids = self.tracked_below(b);
                self.below.insert(b.id, ids);
            }
            if self.below.get(&b.id).is_some_and(|ids| ids.iter().any(|t| !self.covered.contains(t))) {
                return i;
            }
        }
        let i = self.rotation.entry(id).or_insert(0);
        let v = *i % branches.len().max(1);
        *i += 1;
        v
    }

    /// The attribute, element, value and branch ids in `p`, following
    /// references.
    fn tracked_below(&self, p: &Pattern) -> Vec<u32> {
        let mut ids = Vec::new();
        let mut defs: HashSet<&str> = HashSet::new();
        let mut stack: Vec<&Pattern> = vec![p];
        while let Some(q) = stack.pop() {
            q.walk(&mut |n| match &n.kind {
                Kind::Attribute(..) | Kind::Element(..) | Kind::Value { .. } => ids.push(n.id),
                Kind::Choice(bs) => ids.extend(bs.iter().map(|b| b.id)),
                Kind::Ref(r) => {
                    if let Some(def) = self.schema.defs.get(r)
                        && defs.insert(r.as_str())
                    {
                        stack.push(&def.pattern);
                    }
                }
                _ => {}
            });
        }
        ids
    }

    // -- costs -------------------------------------------------------------------

    /// The number of elements in the smallest instance of each definition.
    fn compute_costs(&mut self) {
        let names: Vec<String> = self.schema.defs.keys().cloned().collect();
        for n in &names {
            self.cost.insert(n.clone(), u64::MAX);
        }
        loop {
            let mut changed = false;
            for n in &names {
                let Ok(def) = self.schema.def(n) else { continue };
                let c = self.pattern_cost(&def.pattern);
                if self.cost.get(n) != Some(&c) {
                    self.cost.insert(n.clone(), c);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn pattern_cost(&self, p: &Pattern) -> u64 {
        match &p.kind {
            Kind::Element(_, c) => self.pattern_cost(c).saturating_add(1),
            Kind::Group(ps) | Kind::Interleave(ps) => ps.iter().fold(0u64, |a, q| a.saturating_add(self.pattern_cost(q))),
            Kind::Choice(ps) => ps.iter().map(|q| self.pattern_cost(q)).min().unwrap_or(u64::MAX),
            Kind::Optional(_) | Kind::ZeroOrMore(_) => 0,
            Kind::OneOrMore(q) => self.pattern_cost(q),
            Kind::Ref(r) => self.cost.get(r).copied().unwrap_or(u64::MAX),
            Kind::NotAllowed => u64::MAX,
            _ => 0,
        }
    }

    // -- generation ----------------------------------------------------------------

    /// Adds `p`'s attributes, children and text to `out`. `full` asks for
    /// every optional item.
    fn generate(&mut self, p: &Pattern, out: &mut Node, depth: usize, full: bool) -> Result<(), String> {
        let full = full && depth < self.max_depth;
        let random = self.style == Style::Random && self.elements < RANDOM_ELEMENTS;
        match &p.kind {
            Kind::Element(name, content) => {
                self.covered.insert(p.id);
                self.elements += 1;
                let mut node = Node { name: qualified(name)?, ..Node::default() };
                // Only a full occurrence counts as the first.
                let first = full && self.visited.insert(format!("inline:{}", p.id));
                self.generate(content, &mut node, depth + 1, first)?;
                out.children.push(Child::Elem(node));
            }
            Kind::Attribute(name, v) => {
                self.covered.insert(p.id);
                let NameClass::Name(n) = name else { return Err("attribute name class".to_owned()) };
                self.in_attribute = true;
                let value = self.value(v, false);
                self.in_attribute = false;
                let value = value?;
                out.attrs.push((n.local.clone(), value));
            }
            Kind::Group(ps) => {
                for q in ps {
                    self.generate(q, out, depth, full)?;
                }
            }
            Kind::Interleave(ps) => self.interleave(ps, out, depth, full)?,
            Kind::Choice(ps) => {
                let i = if self.is_value(p) {
                    let text = self.value(p, false)?;
                    out.children.push(Child::Text(text));
                    return Ok(());
                } else if full {
                    self.pick_branch(p.id, ps)
                } else if random && depth < self.max_depth {
                    self.below(ps.len())
                } else {
                    self.cheapest(ps)
                };
                self.covered.insert(ps[i].id);
                self.generate(&ps[i], out, depth, full)?;
            }
            Kind::Optional(q) => {
                let attribute = matches!(q.kind, Kind::Attribute(..));
                let include = if full {
                    true
                } else if random && depth < self.max_depth {
                    self.chance(if attribute { 40 } else { 55u64.saturating_sub(depth as u64 * 6) })
                } else {
                    false
                };
                if include {
                    self.generate(q, out, depth, full)?;
                }
            }
            Kind::ZeroOrMore(q) | Kind::OneOrMore(q) => {
                let min = usize::from(matches!(p.kind, Kind::OneOrMore(_)));
                let n = if full {
                    1
                } else if random && depth < self.max_depth {
                    self.below(if depth < 4 { 3 } else { 2 })
                } else {
                    0
                };
                for _ in 0..n.max(min) {
                    self.generate(q, out, depth, full)?;
                }
            }
            Kind::Ref(r) => {
                let def = self.schema.def(r)?;
                if matches!(def.pattern.kind, Kind::Element(..)) {
                    self.covered.insert(def.pattern.id);
                    self.elements += 1;
                    let Kind::Element(name, content) = &def.pattern.kind else { return Ok(()) };
                    let mut node = Node { name: qualified(name)?, ..Node::default() };
                    let first = full && self.visited.insert(r.clone());
                    self.generate(content, &mut node, depth + 1, first)?;
                    out.children.push(Child::Elem(node));
                } else if self.is_value(&def.pattern) {
                    let text = self.value(&def.pattern, false)?;
                    out.children.push(Child::Text(text));
                } else {
                    self.generate(&def.pattern, out, depth, full)?;
                }
            }
            Kind::Text | Kind::Data { .. } | Kind::List(_) | Kind::Value { .. } => {
                let text = self.value(p, false)?;
                out.children.push(Child::Text(text));
            }
            Kind::Empty => {}
            Kind::NotAllowed => return Err("notAllowed".to_owned()),
        }
        Ok(())
    }

    fn cheapest(&self, ps: &[Pattern]) -> usize {
        (0..ps.len()).min_by_key(|&i| self.pattern_cost(&ps[i])).unwrap_or(0)
    }

    /// Interleave members in the order the writer uses (members in schema
    /// order; all repeated members' items, in their own order, at the first
    /// repeated member when there are several), or in any order.
    fn interleave(&mut self, ps: &[Pattern], out: &mut Node, depth: usize, full: bool) -> Result<(), String> {
        let repeated = ps.iter().filter(|m| matches!(m.kind, Kind::ZeroOrMore(_) | Kind::OneOrMore(_))).count();
        let mut groups: Vec<(bool, Vec<Child>)> = Vec::new();
        for m in ps {
            let mut tmp = Node::default();
            self.generate(m, &mut tmp, depth, full)?;
            out.attrs.append(&mut tmp.attrs);
            groups.push((matches!(m.kind, Kind::ZeroOrMore(_) | Kind::OneOrMore(_)), tmp.children));
        }
        if self.lexical == Lexical::Any {
            let mut all: Vec<Child> = groups.into_iter().flat_map(|(_, c)| c).collect();
            self.shuffle(&mut all);
            out.children.extend(all);
            return Ok(());
        }
        if repeated < 2 {
            out.children.extend(groups.into_iter().flat_map(|(_, c)| c));
            return Ok(());
        }
        let first_repeated = groups.iter().position(|(r, _)| *r);
        let mut pool: Vec<Child> = Vec::new();
        let mut rest: Vec<(bool, Vec<Child>)> = Vec::new();
        for (is_repeated, children) in groups {
            if is_repeated {
                pool.extend(children);
                rest.push((true, Vec::new()));
            } else {
                rest.push((false, children));
            }
        }
        self.shuffle(&mut pool);
        let mut pool = Some(pool);
        for (i, (_, children)) in rest.into_iter().enumerate() {
            if Some(i) == first_repeated {
                out.children.extend(pool.take().unwrap_or_default());
            }
            out.children.extend(children);
        }
        Ok(())
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i + 1);
            items.swap(i, j);
        }
    }

    fn is_value(&self, p: &Pattern) -> bool {
        match &p.kind {
            Kind::Text | Kind::Data { .. } | Kind::Value { .. } | Kind::List(_) => true,
            Kind::Choice(ps) => ps.iter().all(|q| self.is_value(q)),
            Kind::Ref(r) => self.schema.def(r).is_ok_and(|d| self.is_value(&d.pattern)),
            _ => false,
        }
    }

    // -- values ------------------------------------------------------------------

    fn value(&mut self, p: &Pattern, in_list: bool) -> Result<String, String> {
        Ok(match &p.kind {
            Kind::Value { value, .. } => {
                self.covered.insert(p.id);
                value.clone()
            }
            Kind::Text => self.string(in_list, !self.in_attribute),
            Kind::Data { xsd, params } => self.data(xsd, params, in_list)?,
            Kind::Ref(r) => {
                let def = self.schema.def(r)?;
                self.value(&def.pattern, in_list)?
            }
            Kind::Choice(ps) => {
                // Enumeration literals not exercised yet come first.
                let i = self.pick_branch(p.id, ps);
                self.covered.insert(ps[i].id);
                self.value(&ps[i], in_list)?
            }
            Kind::List(inner) => {
                let items = self.list(inner)?;
                let sep = if self.lexical == Lexical::Any && self.chance(30) { "\t " } else { " " };
                items.join(sep)
            }
            _ => return Err(format!("not a value: {}", crate::lower::describe(p))),
        })
    }

    fn list(&mut self, p: &Pattern) -> Result<Vec<String>, String> {
        Ok(match &p.kind {
            Kind::ZeroOrMore(q) => {
                let n = self.below(4);
                (0..n).map(|_| self.value(q, true)).collect::<Result<_, _>>()?
            }
            Kind::Group(qs) => qs.iter().map(|q| self.value(q, true)).collect::<Result<_, _>>()?,
            _ => vec![self.value(p, true)?],
        })
    }

    fn data(&mut self, xsd: &str, params: &[(String, String)], in_list: bool) -> Result<String, String> {
        let any = self.lexical == Lexical::Any;
        let range = match params {
            [(a, min), (b, max)] if a == "minInclusive" && b == "maxInclusive" => Some((min.as_str(), max.as_str())),
            _ => None,
        };
        Ok(match xsd {
            "string" if params.iter().any(|(k, _)| k == "pattern") => {
                self.paths += 1;
                format!("Parts/Part_{}.xml", self.paths)
            }
            "string" => self.string(in_list, false),
            "boolean" => {
                let b = self.chance(50);
                match (any && self.chance(50), b) {
                    (true, true) => "1".to_owned(),
                    (true, false) => "0".to_owned(),
                    (false, b) => b.to_string(),
                }
            }
            "int" | "short" => {
                let (lo, hi): (i64, i64) = match range {
                    Some((min, max)) => (min.parse().map_err(|_| "range")?, max.parse().map_err(|_| "range")?),
                    None if xsd == "int" => (i32::MIN.into(), i32::MAX.into()),
                    None => (i16::MIN.into(), i16::MAX.into()),
                };
                let v = match self.below(4) {
                    0 => lo,
                    1 => hi,
                    _ => lo + (self.next() % ((hi - lo) as u64 + 1)) as i64,
                };
                if any && v >= 0 && self.chance(30) { format!(" +0{v} ") } else { v.to_string() }
            }
            "double" => {
                let v = match range {
                    Some((min, max)) => {
                        let (lo, hi): (f64, f64) = (min.parse().map_err(|_| "range")?, max.parse().map_err(|_| "range")?);
                        match self.below(4) {
                            0 => lo,
                            1 => hi,
                            _ => lo + (hi - lo) * (self.below(1000) as f64 / 999.0),
                        }
                    }
                    None => *self.pick(&[
                        0.0,
                        -0.0,
                        1.0,
                        -1.5,
                        12.345,
                        1e-7,
                        123_456_789.125,
                        f64::MAX,
                        f64::MIN_POSITIVE,
                        5e-324,
                        f64::INFINITY,
                        f64::NEG_INFINITY,
                        f64::NAN,
                    ]),
                };
                let mut s = String::new();
                if v.is_nan() {
                    s.push_str("NaN");
                } else if v.is_infinite() {
                    s.push_str(if v > 0.0 { "INF" } else { "-INF" });
                } else if any && v.fract() == 0.0 && v.abs() < 1e15 && self.chance(50) {
                    let _ = write!(s, "{v:.1}");
                } else if any && v != 0.0 && v.abs() < 1e15 && self.chance(30) {
                    let _ = write!(s, "{v:E}");
                } else {
                    let _ = write!(s, "{v}");
                }
                s
            }
            "dateTime" => self
                .pick(&["2026-10-23T09:30:00Z", "2024-02-29T23:59:59.125+14:00", "1999-12-31T23:59:59.5Z", "2026-01-02T03:04:05-07:30"])
                .to_string(),
            other => return Err(format!("no generator for xsd:{other}")),
        })
    }

    /// A string; `text` strings may contain characters that IDML writes as
    /// `<?ACE n?>`.
    fn string(&mut self, token: bool, text: bool) -> String {
        if token {
            return self.pick(&["u1d8", "Color/Black", "ParagraphStyle/$ID/NormalParagraphStyle", "ü€", "a&b<c>\"d'"]).to_string();
        }
        let mut pool = vec![
            "",
            "a",
            "Hello, World",
            "  leading and trailing  ",
            "tab\there",
            "line\nbreak\r\nand\rreturns",
            "<&>\"'",
            "Ünïcödé € 中文 😀",
            "$ID/[No paragraph style]",
            "]]>",
        ];
        if text {
            pool.extend(["indent\u{7}here", "page \u{18} number", "\u{3}\u{4}\u{1f}"]);
        }
        self.pick(&pool).to_string()
    }
}

fn qualified(name: &NameClass) -> Result<String, String> {
    match name {
        NameClass::Name(QName { prefix: Some(p), local, .. }) => Ok(format!("{p}:{local}")),
        NameClass::Name(QName { local, .. }) => Ok(local.clone()),
        NameClass::Any => Err("wildcard element".to_owned()),
    }
}

/// Writes a node without indentation (so whitespace handling is tested).
fn write_node(node: &Node, out: &mut String) {
    out.push('<');
    out.push_str(&node.name);
    for (k, v) in &node.attrs {
        let _ = write!(out, " {k}=\"");
        for c in v.chars() {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '"' => out.push_str("&quot;"),
                '\t' => out.push_str("&#x9;"),
                '\n' => out.push_str("&#xA;"),
                '\r' => out.push_str("&#xD;"),
                c => out.push(c),
            }
        }
        out.push('"');
    }
    if node.children.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for c in &node.children {
        match c {
            Child::Elem(n) => write_node(n, out),
            Child::Text(t) => {
                for c in t.chars() {
                    match c {
                        '&' => out.push_str("&amp;"),
                        '<' => out.push_str("&lt;"),
                        '>' => out.push_str("&gt;"),
                        '\r' => out.push_str("&#xD;"),
                        c if c < ' ' && !matches!(c, '\t' | '\n') => {
                            let _ = write!(out, "<?ACE {:X}?>", u32::from(c));
                        }
                        c => out.push(c),
                    }
                }
            }
        }
    }
    let _ = write!(out, "</{}>", node.name);
}
