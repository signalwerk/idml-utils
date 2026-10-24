//! The intermediate model between the grammar and the generated Rust code.
//!
//! Every reachable schema pattern maps to exactly one construct here:
//!
//! | Schema                                   | Model                                  |
//! |------------------------------------------|----------------------------------------|
//! | `X_Object`, `X_File`, content `X_TypeDef` | [`TypeKind::Struct`] or [`TypeKind::Choice`] |
//! | `X_EnumValue`                            | [`TypeKind::Enum`]                     |
//! | value `X_TypeDef`                        | [`TypeKind::Alias`]                    |
//! | `attribute N { literal }`                | [`Fixed`] (no data)                    |
//! | `attribute N { value }`                  | [`Attr`]                               |
//! | `element N { … }` (inline)               | [`Elem`] whose [`Ty`] is collapsed or a new named type |
//! | `p?`, `p*`, exactly one                  | [`Occurs`]                             |
//! | `p, q`                                   | items in [`Body::Children`] order      |
//! | `p & q`                                  | [`Item::Interleave`]; two or more repeated members share an ordered [`TypeKind::Children`] list |
//! | `p \| q` of contents                     | [`TypeKind::Choice`]                   |
//! | `p \| q` of values                       | [`TypeKind::Union`]                    |
//! | datatype with min/max facets             | [`TypeKind::Ranged`]                   |
//! | `list { … }`                             | [`ValueType::List`], [`ValueType::Array`] |

use std::collections::BTreeMap;

pub type TypeId = usize;

#[derive(Debug)]
pub struct Model {
    pub schema_dir: String,
    pub indesign_version: String,
    pub dom_version: String,
    pub sha256: String,
    /// Schema files and their SHA-256.
    pub files: BTreeMap<String, String>,
    pub stray_lines: BTreeMap<String, Vec<usize>>,
    pub types: Vec<Type>,
    /// The package parts, in schema file order.
    pub roots: Vec<Root>,
    /// Definitions no part grammar reaches, with the files defining them.
    pub unreachable: Vec<(String, Vec<String>)>,
}

#[derive(Debug)]
pub struct Root {
    pub file: String,
    pub element: String,
    pub ty: TypeId,
}

#[derive(Debug, Clone)]
pub struct Type {
    pub name: String,
    pub module: String,
    /// The schema item in plain text: a definition name, or the path to an
    /// inline element such as `TextFrame_Object/<Properties>`.
    pub origin: String,
    /// The files defining it, for definitions.
    pub files: Vec<String>,
    /// The schema's documentation comment, unformatted.
    pub doc: Vec<String>,
    /// For element definitions: the element name.
    pub element: Option<String>,
    pub kind: TypeKind,
    pub trace: Trace,
}

/// The schema items a model item represents, and when they entered IDML.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trace {
    /// Pattern nodes of the target schema: where the item is defined, and
    /// every other occurrence of a shared inline element.
    pub nodes: Vec<u32>,
    /// Their item IDs (see `history`).
    pub items: Vec<String>,
    /// `Since IDML 7.0 or earlier.`, from the schema history.
    pub since: Option<String>,
}

/// An enumeration literal.
#[derive(Debug, Clone, PartialEq)]
pub struct Literal {
    pub value: String,
    pub trace: Trace,
}

#[derive(Debug, Clone)]
pub enum TypeKind {
    Struct(Content),
    Choice(Vec<Variant>),
    /// Literals in schema order.
    Enum(Vec<Literal>),
    /// A choice between value types (each a named value type).
    Union(Vec<TypeId>),
    Ranged {
        base: Scalar,
        min: String,
        max: String,
    },
    Alias(ValueType),
    /// The members of an interleave with two or more repeated elements.
    Children(Vec<Member>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scalar {
    Int,
    Short,
    Double,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Content {
    pub fixed: Vec<Fixed>,
    pub attrs: Vec<Attr>,
    pub body: Body,
}

/// An attribute with a literal value: it carries no data.
#[derive(Debug, Clone, PartialEq)]
pub struct Fixed {
    pub name: String,
    pub value: String,
    /// Compared as `token` (whitespace-normalized) rather than `string`.
    pub token: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attr {
    pub field: String,
    pub name: String,
    pub ty: ValueType,
    pub required: bool,
    /// The schema's value pattern, in compact syntax.
    pub schema: String,
    pub trace: Trace,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum Body {
    #[default]
    Empty,
    /// Text content.
    Text {
        field: String,
        ty: ValueType,
        schema: String,
        trace: Trace,
    },
    Children(Vec<Item>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Elem(ElemField),
    /// Members in any order.
    Interleave(Vec<Slot>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Slot {
    Elem(ElemField),
    /// The repeated members, kept in document order.
    Group {
        field: String,
        ty: TypeId,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ElemField {
    pub field: String,
    pub elem: Elem,
    pub occurs: Occurs,
    pub boxed: bool,
    pub trace: Trace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Occurs {
    One,
    Optional,
    Many,
}

/// An element and the Rust representation of its content.
#[derive(Debug, Clone, PartialEq)]
pub struct Elem {
    /// The qualified name (`idPkg:Story` or `TextFrame`).
    pub name: String,
    /// Literal attributes of this element's content, when [`Elem::ty`] does
    /// not handle them itself.
    pub fixed: Vec<Fixed>,
    pub ty: Ty,
    /// A designmap reference to a package part (`<idPkg:Spread src="…"/>`).
    pub part: Option<PartRef>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PartRef {
    /// The schema fixes the part path.
    Fixed(String),
    /// The path is data (`Part<T>`).
    Path,
}

/// The Rust type of an element's content.
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    /// Empty content: `()`.
    Unit,
    /// Text content.
    Value(ValueType),
    /// A generated struct or choice.
    Named(TypeId),
    /// Content that is just a repeated child: `Vec<T>`.
    Vec(Box<Elem>),
    /// Content that is N identical required children: `[T; N]`.
    Array(Box<Elem>, usize),
    /// Content that is one required child.
    Wrap(Box<Elem>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub name: String,
    pub fixed: Vec<Fixed>,
    pub ty: Ty,
    pub boxed: bool,
    pub schema: String,
    pub trace: Trace,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub name: String,
    pub elem: Elem,
    pub boxed: bool,
    pub trace: Trace,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ValueType {
    String,
    Bool,
    Int,
    Short,
    Double,
    DateTime,
    /// A string in a list: non-empty, without whitespace.
    Token,
    /// A package part path (`xsd:string { pattern = ".*\.xml" }`).
    PartPath,
    /// An enum, union, ranged or alias type.
    Named(TypeId),
    Array(Box<ValueType>, usize),
    List(Box<ValueType>),
}

impl Content {
    /// True when building a value needs data from the caller: a required
    /// attribute, text, or a required child that has no empty form.
    pub fn has_required(&self) -> bool {
        let required = |f: &ElemField| f.occurs == Occurs::One && !matches!(f.elem.ty, Ty::Unit | Ty::Vec(_));
        self.attrs.iter().any(|a| a.required)
            || match &self.body {
                Body::Empty => false,
                Body::Text { .. } => true,
                Body::Children(items) => items.iter().any(|i| match i {
                    Item::Elem(f) => required(f),
                    Item::Interleave(slots) => slots.iter().any(|s| matches!(s, Slot::Elem(f) if required(f))),
                }),
            }
    }
}
