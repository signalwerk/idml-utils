//! Version-neutral reading, authoring and writing of Adobe InDesign IDML.
//!
//! [`Package`] preserves the document as XML, including each part's version,
//! unknown extensions and lexical forms. New packages require an explicit
//! [`DomVersion`]; writing never upgrades or substitutes the newest version.
//! Generated [`views`] supply one typed vocabulary across schema snapshots.
//! [`Authoring`] checks staged edits against an exact schema profile and lets
//! callers explicitly select a compatible newer target when needed.
//!
//! ```no_run
//! # fn main() -> Result<(), idml::Error> {
//! let version: idml::DomVersion = "8.0".parse()?;
//! let package = idml::Package::new(version, "document")?;
//! let bytes = package.write()?; // DOMVersion remains 8.0.
//! # let _ = bytes;
//! # Ok(())
//! # }
//! ```
//!
//! Parsing and writing preserve data independently of schema acceptance.
//! [`profile::SchemaCatalog`] provides exact validation; unavailable schema
//! releases remain unknown. Conversion is explicit, reviewed and transactional,
//! with loss forbidden by default. ZIP entry contents are preserved; ZIP
//! timestamps and other container metadata are regenerated.
//!
//! `OwnedPackage` and `model` (feature `owned`) are a separate projection into one configured
//! schema for adapters that need owned structs. Their constructors and writers
//! use that schema's version; they are not the universal authoring API.

// rustdoc's auto-trait listing walks the deeply nested generated types.
#![recursion_limit = "512"]

mod archive;
pub mod compose;
pub mod document;
mod element;
mod error;
pub mod evidence;
pub mod geometry;
pub mod layout;
#[rustfmt::skip]
#[cfg(feature = "owned")]
pub mod model;
pub mod authoring;
pub mod migration;
mod options;
#[cfg(feature = "owned")]
mod preserve;
pub mod profile;
#[cfg(feature = "owned")]
mod projection;
pub mod semantic;
#[rustfmt::skip]
mod migration_data;
pub use authoring::{Authoring, AuthoringPlan, UpgradeOption};
pub use profile::DomVersion;
pub mod package;
pub mod tree;
pub mod view;
#[rustfmt::skip]
pub mod views;
pub use package::{ConversionPlan, LossDecision, LossId, LossPolicy, Package};
#[rustfmt::skip]
mod profile_data;
#[cfg(feature = "owned")]
mod upgrade;
pub mod value;
mod xml;

pub use element::Element;
pub use error::{Error, ErrorKind, Location, Warning};
pub use layout::Layout;
pub use options::{MIMETYPE, Mode, Parsed, ReadOptions};
#[cfg(feature = "owned")]
pub use preserve::XmlPart;
#[cfg(feature = "owned")]
pub use projection::{OwnedPackage, Part, PartFile};
pub use tree::{Field, NodeId, XmlTree};
pub use value::{DateTime, PartPath, Token, ValueError};

#[cfg(feature = "owned")]
mod sealed {
    pub trait Sealed {}
}
