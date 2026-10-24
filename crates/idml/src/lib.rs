//! Read, build and write Adobe InDesign IDML packages.
//!
//! The document model in [`model`] is generated from the RELAX NG package
//! schemas that InDesign itself generates (see `schemas/` in the repository).
//! Every element, attribute and value those schemas allow has a Rust
//! representation, and only schema-valid values can be represented:
//!
//! - elements are structs; required, optional and repeated children are
//!   `T`, `Option<T>` and `Vec<T>`;
//! - enumerations are enums, `type`-tagged property values are enums;
//! - range-restricted numbers, list tokens, part paths and date-times are
//!   types that check their value on construction.
//!
//! So writing cannot produce XML that violates the schema. The only checks
//! left for write time are about the package and XML itself: unique part
//! paths, and text that XML cannot represent at all (such as U+FFFE).
//!
//! ```no_run
//! # fn main() -> Result<(), idml::Error> {
//! # let bytes = Vec::new();
//! let mut package = idml::Package::read(&bytes, &idml::ReadOptions::default())?.value;
//! for part in &mut package.document.story {
//!     for story in &mut part.content.story {
//!         story.story_title = Some("Edited".into());
//!     }
//! }
//! let bytes = package.write()?;
//! # let _ = bytes;
//! # Ok(())
//! # }
//! ```
//!
//! Documents of older InDesign versions are read into the same model: a
//! part with an older `DOMVersion` is first edited by rules generated from
//! `schema-upgrades.tsv`, which has one for every schema item older versions
//! have and the current one has not, or has differently. Data the current
//! schema cannot hold is left out with a [`Warning`]. Writing always produces
//! the current [`model::DOM_VERSION`].
//!
//! Beyond the schemas, the crate implements the package conventions:
//! the `mimetype` and `META-INF/container.xml` entries, `idPkg:*` part
//! references in the designmap, and `<?ACE n?>` processing instructions for
//! control characters in text.

// rustdoc's auto-trait listing walks the deeply nested generated types.
#![recursion_limit = "512"]

mod error;
#[rustfmt::skip]
pub mod model;
mod package;
mod upgrade;
pub mod value;
mod xml;

pub use error::{Error, ErrorKind, Location, Warning};
pub use package::{MIMETYPE, Mode, Package, Parsed, Part, PartFile, ReadOptions};
pub use value::{DateTime, PartPath, Token, ValueError};

mod sealed {
    pub trait Sealed {}
}
