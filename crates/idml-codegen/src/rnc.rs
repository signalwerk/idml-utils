//! Parser for the RELAX NG compact syntax that InDesign's `generateIDMLSchema`
//! emits.
//!
//! Only the constructs that occur in such schemas are accepted; anything else
//! is a parse error rather than a silently dropped pattern. One deviation from
//! the RELAX NG specification is tolerated: InDesign writes multi-line
//! documentation comments with `#` on the first line only. Such continuation
//! lines between definitions are reported in [`File::stray_lines`] and kept as
//! part of the next definition's documentation.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The XML Schema datatype library URI.
pub const XSD: &str = "http://www.w3.org/2001/XMLSchema-datatypes";

/// One parsed `.rnc` file.
#[derive(Debug)]
pub struct File {
    /// The first comment line, e.g. `Adobe InDesign 2026 21.5.0.64`.
    pub header: Option<String>,
    pub start: Option<Pattern>,
    pub includes: Vec<String>,
    pub defines: Vec<Define>,
    /// 1-based line numbers of comment continuation lines without `#`.
    pub stray_lines: Vec<usize>,
}

#[derive(Debug)]
pub struct Define {
    pub name: String,
    pub doc: Vec<String>,
    pub pattern: Pattern,
}

/// A pattern node. `id` is unique across all files parsed with one [`Ids`].
#[derive(Debug, Clone)]
pub struct Pattern {
    pub id: u32,
    pub kind: Kind,
}

#[derive(Debug, Clone)]
pub enum Kind {
    Element(NameClass, Box<Pattern>),
    Attribute(NameClass, Box<Pattern>),
    Group(Vec<Pattern>),
    Interleave(Vec<Pattern>),
    Choice(Vec<Pattern>),
    Optional(Box<Pattern>),
    ZeroOrMore(Box<Pattern>),
    OneOrMore(Box<Pattern>),
    List(Box<Pattern>),
    Ref(String),
    Text,
    Empty,
    NotAllowed,
    /// A literal. `token` is true for the `token` datatype (the default for a
    /// bare literal), false for `string`.
    Value {
        token: bool,
        value: String,
    },
    /// An XML Schema datatype with its facets.
    Data {
        xsd: String,
        params: Vec<(String, String)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NameClass {
    Name(QName),
    Any,
}

/// A namespace-qualified name. `prefix` is the prefix the schema uses, kept
/// for writing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QName {
    pub ns: String,
    pub prefix: Option<String>,
    pub local: String,
}

impl QName {
    /// The name as written in XML, `prefix:local` or `local`.
    pub fn qualified(&self) -> String {
        match &self.prefix {
            Some(p) => format!("{p}:{}", self.local),
            None => self.local.clone(),
        }
    }
}

/// Allocates pattern ids.
#[derive(Debug, Default)]
pub struct Ids(u32);

impl Ids {
    pub(crate) fn next(&mut self) -> u32 {
        self.0 += 1;
        self.0
    }
}

impl Pattern {
    /// A canonical text form that ignores node ids: two patterns are
    /// structurally equal exactly when their keys are equal.
    pub fn key(&self) -> String {
        let mut s = String::new();
        self.write_key(&mut s);
        s
    }

    fn write_key(&self, s: &mut String) {
        let list = |s: &mut String, tag: &str, ps: &[Pattern]| {
            s.push_str(tag);
            s.push('(');
            for (i, p) in ps.iter().enumerate() {
                if i > 0 {
                    s.push(' ');
                }
                p.write_key(s);
            }
            s.push(')');
        };
        match &self.kind {
            Kind::Element(n, p) | Kind::Attribute(n, p) => {
                s.push_str(if matches!(self.kind, Kind::Element(..)) { "element " } else { "attribute " });
                match n {
                    NameClass::Name(q) => {
                        let _ = write!(s, "{{{}}}{}", q.ns, q.local);
                    }
                    NameClass::Any => s.push('*'),
                }
                s.push('{');
                p.write_key(s);
                s.push('}');
            }
            Kind::Group(ps) => list(s, "group", ps),
            Kind::Interleave(ps) => list(s, "interleave", ps),
            Kind::Choice(ps) => list(s, "choice", ps),
            Kind::Optional(p) => list(s, "optional", std::slice::from_ref(p)),
            Kind::ZeroOrMore(p) => list(s, "zeroOrMore", std::slice::from_ref(p)),
            Kind::OneOrMore(p) => list(s, "oneOrMore", std::slice::from_ref(p)),
            Kind::List(p) => list(s, "list", std::slice::from_ref(p)),
            Kind::Ref(n) => {
                let _ = write!(s, "ref:{n}");
            }
            Kind::Text => s.push_str("text"),
            Kind::Empty => s.push_str("empty"),
            Kind::NotAllowed => s.push_str("notAllowed"),
            Kind::Value { token, value } => {
                let _ = write!(s, "value:{}:{value:?}", if *token { "token" } else { "string" });
            }
            Kind::Data { xsd, params } => {
                let _ = write!(s, "data:{xsd}");
                for (k, v) in params {
                    let _ = write!(s, ":{k}={v:?}");
                }
            }
        }
    }

    /// Calls `f` for this node and every node below it (not following refs).
    pub fn walk<'p>(&'p self, f: &mut impl FnMut(&'p Pattern)) {
        f(self);
        match &self.kind {
            Kind::Element(_, p) | Kind::Attribute(_, p) | Kind::Optional(p) | Kind::ZeroOrMore(p) | Kind::OneOrMore(p) | Kind::List(p) => p.walk(f),
            Kind::Group(ps) | Kind::Interleave(ps) | Kind::Choice(ps) => {
                for p in ps {
                    p.walk(f);
                }
            }
            Kind::Ref(_) | Kind::Text | Kind::Empty | Kind::NotAllowed | Kind::Value { .. } | Kind::Data { .. } => {}
        }
    }
}

/// Parses one file.
pub fn parse(text: &str, ids: &mut Ids) -> Result<File, String> {
    let lines = scan_lines(text);
    let tokens = lex(&lines.code)?;
    // The compact syntax predeclares the `xsd` datatype prefix.
    let datatypes = BTreeMap::from([("xsd".to_owned(), XSD.to_owned())]);
    let mut p = Parser { tokens, pos: 0, ids, namespaces: BTreeMap::new(), datatypes };
    let mut file = File { header: lines.header, start: None, includes: Vec::new(), defines: Vec::new(), stray_lines: lines.stray };
    p.top(&mut file, &lines.docs)?;
    Ok(file)
}

// ---------------------------------------------------------------------------
// Line scan: header, documentation comments and stray comment lines.

struct Lines {
    /// The source with stray lines blanked out (line numbers unchanged).
    code: String,
    header: Option<String>,
    stray: Vec<usize>,
    /// Documentation by the 1-based line on which a definition starts.
    docs: BTreeMap<usize, Vec<String>>,
}

fn scan_lines(text: &str) -> Lines {
    let mut out = Lines { code: String::with_capacity(text.len()), header: None, stray: Vec::new(), docs: BTreeMap::new() };
    // Brace depth outside definitions (1 = grammar body), whether a
    // definition is open, and its bracket balance.
    let (mut depth, mut inside, mut balance) = (0i32, false, 0i32);
    let mut doc: Vec<String> = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        let number = i + 1;
        let code = code_part(line);
        let trimmed = code.trim();
        let comment = line.trim_start().strip_prefix('#').map(|c| c.strip_prefix(' ').unwrap_or(c).trim_end());
        if out.header.is_none() && number == 1 {
            out.header = comment.map(str::to_owned);
        }
        if depth == 1 && !inside {
            if let Some(c) = comment.filter(|_| trimmed.is_empty()) {
                doc.push(c.to_owned());
            } else if trimmed.is_empty() {
                if !doc.is_empty() {
                    doc.push(String::new());
                }
            } else if !starts_top_level(trimmed) {
                out.stray.push(number);
                doc.push(line.trim_end().to_owned());
                out.code.push('\n');
                continue;
            } else {
                if is_definition(trimmed) {
                    while doc.last().is_some_and(String::is_empty) {
                        doc.pop();
                    }
                    if !doc.is_empty() {
                        out.docs.insert(number, std::mem::take(&mut doc));
                    }
                    inside = true;
                    balance = 0;
                }
                doc.clear();
            }
        }
        out.code.push_str(line);
        out.code.push('\n');
        let delta = count(code, &['{', '(']) - count(code, &['}', ')']);
        if inside {
            balance += delta;
            if balance == 0 && !trimmed.is_empty() && !trimmed.ends_with([',', '|', '&', '=', '(']) {
                inside = false;
            }
        } else {
            depth += count(code, &['{']) - count(code, &['}']);
        }
    }
    out
}

/// The part of a line before any comment, with string literals blanked.
fn code_part(line: &str) -> &str {
    let mut in_string = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_string = !in_string,
            '#' if !in_string => return &line[..i],
            _ => {}
        }
    }
    line
}

fn count(code: &str, chars: &[char]) -> i32 {
    let mut in_string = false;
    let mut n = 0;
    for c in code.chars() {
        if c == '"' {
            in_string = !in_string;
        } else if !in_string && chars.contains(&c) {
            n += 1;
        }
    }
    n
}

fn is_definition(s: &str) -> bool {
    let name_end = s.find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '.' | '-'))).unwrap_or(s.len());
    name_end > 0 && s[name_end..].trim_start().starts_with('=')
}

fn starts_top_level(s: &str) -> bool {
    is_definition(s) || s.starts_with('}') || s.starts_with("include ") || s.starts_with("div ")
}

// ---------------------------------------------------------------------------
// Lexer

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// An identifier or keyword.
    Ident(String),
    /// `prefix:local` or `prefix:*`.
    CName(String, String),
    Literal(String),
    Op(char),
}

struct Token {
    tok: Tok,
    line: usize,
}

fn lex(text: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    let mut line = 1;
    while let Some(&(start, c)) = chars.peek() {
        match c {
            '\n' => {
                line += 1;
                chars.next();
            }
            c if c.is_whitespace() => {
                chars.next();
            }
            '#' => {
                while chars.peek().is_some_and(|&(_, c)| c != '\n') {
                    chars.next();
                }
            }
            '"' => {
                chars.next();
                let mut value = String::new();
                loop {
                    match chars.next() {
                        Some((_, '"')) => break,
                        Some((_, '\n')) | None => return Err(format!("line {line}: unterminated literal")),
                        Some((_, c)) => value.push(c),
                    }
                }
                out.push(Token { tok: Tok::Literal(value), line });
            }
            '{' | '}' | '(' | ')' | '|' | ',' | '&' | '?' | '*' | '+' | '=' | '-' | '~' => {
                chars.next();
                out.push(Token { tok: Tok::Op(c), line });
            }
            c if c.is_alphabetic() || c == '_' => {
                let mut end = start;
                while let Some(&(i, c)) = chars.peek() {
                    if c.is_alphanumeric() || matches!(c, '_' | '.' | '-') {
                        end = i + c.len_utf8();
                        chars.next();
                    } else {
                        break;
                    }
                }
                let ident = text[start..end].to_owned();
                if chars.peek().is_some_and(|&(_, c)| c == ':') {
                    chars.next();
                    let local = match chars.peek() {
                        Some(&(_, '*')) => {
                            chars.next();
                            "*".to_owned()
                        }
                        _ => {
                            let s = chars.peek().map_or(text.len(), |&(i, _)| i);
                            let mut e = s;
                            while let Some(&(i, c)) = chars.peek() {
                                if c.is_alphanumeric() || matches!(c, '_' | '.' | '-') {
                                    e = i + c.len_utf8();
                                    chars.next();
                                } else {
                                    break;
                                }
                            }
                            if e == s {
                                return Err(format!("line {line}: empty local name after `{ident}:`"));
                            }
                            text[s..e].to_owned()
                        }
                    };
                    out.push(Token { tok: Tok::CName(ident, local), line });
                } else {
                    out.push(Token { tok: Tok::Ident(ident), line });
                }
            }
            c => return Err(format!("line {line}: unexpected character {c:?}")),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Parser

struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    ids: &'a mut Ids,
    namespaces: BTreeMap<String, String>,
    datatypes: BTreeMap<String, String>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos).map(|t| &t.tok)
    }

    fn line(&self) -> usize {
        self.tokens.get(self.pos).or(self.tokens.last()).map_or(0, |t| t.line)
    }

    fn next(&mut self) -> Result<Tok, String> {
        let t = self.tokens.get(self.pos).map(|t| t.tok.clone()).ok_or("unexpected end of file")?;
        self.pos += 1;
        Ok(t)
    }

    fn error<T>(&self, msg: impl std::fmt::Display) -> Result<T, String> {
        Err(format!("line {}: {msg}", self.line()))
    }

    fn expect_op(&mut self, op: char) -> Result<(), String> {
        match self.next()? {
            Tok::Op(c) if c == op => Ok(()),
            t => {
                self.pos -= 1;
                self.error(format!("expected `{op}`, found {t:?}"))
            }
        }
    }

    fn is_op(&self, op: char) -> bool {
        self.peek() == Some(&Tok::Op(op))
    }

    fn is_ident(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(w)) if w == word)
    }

    fn literal(&mut self) -> Result<String, String> {
        match self.next()? {
            Tok::Literal(s) => Ok(s),
            t => self.error(format!("expected a literal, found {t:?}")),
        }
    }

    fn pattern(&mut self, kind: Kind) -> Pattern {
        Pattern { id: self.ids.next(), kind }
    }

    fn top(&mut self, file: &mut File, docs: &BTreeMap<usize, Vec<String>>) -> Result<(), String> {
        loop {
            if self.is_ident("namespace") || self.is_ident("datatypes") {
                let keyword = self.next()?;
                let Tok::Ident(prefix) = self.next()? else { return self.error("expected a prefix") };
                self.expect_op('=')?;
                let uri = self.literal()?;
                if keyword == Tok::Ident("namespace".into()) {
                    self.namespaces.insert(prefix, uri);
                } else {
                    self.datatypes.insert(prefix, uri);
                }
            } else if self.is_ident("default") {
                return self.error("`default namespace` is not supported");
            } else {
                break;
            }
        }
        if self.is_ident("grammar") {
            self.next()?;
            self.expect_op('{')?;
            self.grammar_body(file, docs)?;
            self.expect_op('}')?;
        } else {
            self.grammar_body(file, docs)?;
        }
        if self.pos < self.tokens.len() {
            return self.error("unexpected content after the grammar");
        }
        Ok(())
    }

    fn grammar_body(&mut self, file: &mut File, docs: &BTreeMap<usize, Vec<String>>) -> Result<(), String> {
        while let Some(tok) = self.peek() {
            match tok {
                Tok::Op('}') => return Ok(()),
                Tok::Ident(w) if w == "include" => {
                    self.next()?;
                    file.includes.push(self.literal()?);
                    if self.is_op('{') {
                        return self.error("include with overrides is not supported");
                    }
                }
                Tok::Ident(_) => {
                    let line = self.line();
                    let Tok::Ident(name) = self.next()? else { return self.error("expected a name") };
                    match self.next()? {
                        Tok::Op('=') => {}
                        Tok::Op(c @ ('|' | '&')) => return self.error(format!("combine `{c}=` is not supported")),
                        t => return self.error(format!("expected `=` after `{name}`, found {t:?}")),
                    }
                    let pattern = self.alternatives()?;
                    if name == "start" {
                        if file.start.is_some() {
                            return self.error("duplicate start");
                        }
                        file.start = Some(pattern);
                    } else {
                        let doc = docs.get(&line).cloned().unwrap_or_default();
                        file.defines.push(Define { name, doc, pattern });
                    }
                }
                t => return self.error(format!("unexpected {t:?} in grammar")),
            }
        }
        Ok(())
    }

    /// `particle ((| particle)+ | (, particle)+ | (& particle)+)?`
    fn alternatives(&mut self) -> Result<Pattern, String> {
        let first = self.particle()?;
        let op = match self.peek() {
            Some(Tok::Op(c @ ('|' | ',' | '&'))) => *c,
            _ => return Ok(first),
        };
        let mut items = vec![first];
        while self.is_op(op) {
            self.next()?;
            items.push(self.particle()?);
        }
        if let Some(Tok::Op(c @ ('|' | ',' | '&'))) = self.peek() {
            return self.error(format!("mixing `{op}` and `{c}` needs parentheses"));
        }
        let kind = match op {
            '|' => Kind::Choice(items),
            ',' => Kind::Group(items),
            _ => Kind::Interleave(items),
        };
        Ok(self.pattern(kind))
    }

    fn particle(&mut self) -> Result<Pattern, String> {
        let primary = self.primary()?;
        let kind = match self.peek() {
            Some(Tok::Op('?')) => Kind::Optional(Box::new(primary)),
            Some(Tok::Op('*')) => Kind::ZeroOrMore(Box::new(primary)),
            Some(Tok::Op('+')) => Kind::OneOrMore(Box::new(primary)),
            _ => return Ok(primary),
        };
        self.next()?;
        if matches!(self.peek(), Some(Tok::Op('?' | '*' | '+'))) {
            return self.error("repeated quantifier");
        }
        Ok(self.pattern(kind))
    }

    fn braced(&mut self) -> Result<Pattern, String> {
        self.expect_op('{')?;
        let p = if self.is_op('}') { self.pattern(Kind::Empty) } else { self.alternatives()? };
        self.expect_op('}')?;
        Ok(p)
    }

    fn primary(&mut self) -> Result<Pattern, String> {
        match self.next()? {
            Tok::Op('(') => {
                let p = self.alternatives()?;
                self.expect_op(')')?;
                Ok(p)
            }
            Tok::Ident(w) if w == "element" || w == "attribute" => {
                let name = self.name_class(w == "attribute")?;
                let content = self.braced()?;
                let kind = if w == "element" { Kind::Element(name, Box::new(content)) } else { Kind::Attribute(name, Box::new(content)) };
                Ok(self.pattern(kind))
            }
            Tok::Ident(w) if w == "list" => {
                let content = self.braced()?;
                Ok(self.pattern(Kind::List(Box::new(content))))
            }
            Tok::Ident(w) if w == "text" => Ok(self.pattern(Kind::Text)),
            Tok::Ident(w) if w == "empty" => Ok(self.pattern(Kind::Empty)),
            Tok::Ident(w) if w == "notAllowed" => Ok(self.pattern(Kind::NotAllowed)),
            Tok::Ident(w) if (w == "string" || w == "token") && matches!(self.peek(), Some(Tok::Literal(_))) => {
                let value = self.literal()?;
                Ok(self.pattern(Kind::Value { token: w == "token", value }))
            }
            Tok::Ident(w) if w == "string" || w == "token" => self.error(format!("builtin datatype `{w}` is not supported")),
            Tok::Ident(w) if matches!(w.as_str(), "mixed" | "parent" | "external" | "grammar") => self.error(format!("`{w}` is not supported")),
            Tok::Ident(name) => Ok(self.pattern(Kind::Ref(name))),
            Tok::Literal(value) => Ok(self.pattern(Kind::Value { token: true, value })),
            Tok::CName(prefix, local) => {
                if self.datatypes.get(&prefix).map(String::as_str) != Some(XSD) {
                    return self.error(format!("unknown datatype library `{prefix}`"));
                }
                if matches!(self.peek(), Some(Tok::Literal(_))) {
                    return self.error("typed literals other than string/token are not supported");
                }
                let mut params = Vec::new();
                if self.is_op('{') {
                    self.next()?;
                    while !self.is_op('}') {
                        let Tok::Ident(param) = self.next()? else { return self.error("expected a facet name") };
                        self.expect_op('=')?;
                        params.push((param, self.literal()?));
                    }
                    self.next()?;
                }
                if self.is_op('-') {
                    return self.error("datatype exceptions are not supported");
                }
                Ok(self.pattern(Kind::Data { xsd: local, params }))
            }
            t => self.error(format!("unexpected {t:?}")),
        }
    }

    fn name_class(&mut self, attribute: bool) -> Result<NameClass, String> {
        match self.next()? {
            Tok::Op('*') => Ok(NameClass::Any),
            Tok::Ident(local) => Ok(NameClass::Name(QName { ns: String::new(), prefix: None, local })),
            Tok::CName(prefix, local) if local != "*" => {
                let Some(ns) = self.namespaces.get(&prefix) else {
                    return self.error(format!("undeclared namespace prefix `{prefix}`"));
                };
                if attribute && ns.is_empty() {
                    return self.error("prefixed attribute without namespace");
                }
                Ok(NameClass::Name(QName { ns: ns.clone(), prefix: Some(prefix), local }))
            }
            t => self.error(format!("unsupported name class {t:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(text: &str) -> File {
        parse(text, &mut Ids::default()).unwrap()
    }

    #[test]
    fn parses_definitions_and_documentation() {
        let f = parse_ok(
            "# Adobe InDesign 2026 1.2\n\ndatatypes xsd = \"http://www.w3.org/2001/XMLSchema-datatypes\"\n\
             namespace p=\"urn:p\"\ngrammar {\nstart = A\ninclude \"d.rnc\"\n\n\
             # A thing.\nA = element p:A {\n  attribute X { xsd:double {minInclusive=\"0\" maxInclusive=\"1\"} }?,\n  (B* & C?)\n}\n\
             B = element B { text }\nC = element C { string \"x\" | string \"y\" }\n}",
        );
        assert_eq!(f.header.as_deref(), Some("Adobe InDesign 2026 1.2"));
        assert_eq!(f.includes, ["d.rnc"]);
        assert_eq!(f.defines.len(), 3);
        assert_eq!(f.defines[0].doc, ["A thing."]);
        assert_eq!(
            f.defines[0].pattern.key(),
            "element {urn:p}A{group(optional(attribute {}X{data:double:minInclusive=\"0\":maxInclusive=\"1\"}) \
             interleave(zeroOrMore(ref:B) optional(ref:C)))}"
        );
        assert_eq!(f.defines[2].pattern.key(), "element {}C{choice(value:string:\"x\" value:string:\"y\")}");
        assert!(f.stray_lines.is_empty());
    }

    #[test]
    fn keeps_stray_comment_lines_as_documentation() {
        let f = parse_ok("grammar {\n# First.\n\nSecond line\n| table |\nA = text\n}");
        assert_eq!(f.stray_lines, [4, 5]);
        assert_eq!(f.defines[0].doc, ["First.", "", "Second line", "| table |"]);
    }

    #[test]
    fn rejects_unsupported_constructs() {
        for text in ["grammar { A = mixed { text } }", "grammar { A |= text }", "grammar { A = element * - x { text } }"] {
            assert!(parse(text, &mut Ids::default()).is_err(), "{text}");
        }
    }
}
