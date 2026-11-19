//! Shared parsing options and diagnostics containers.
use crate::Warning;
/// The content of the IDML mimetype entry.
pub const MIMETYPE: &str = "application/vnd.adobe.indesign-idml-package";

/// Structural strictness for `Package`; schema projection strictness for
/// `OwnedPackage` and `PartFile`. Schema validation of `Package` is explicit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    /// Package/XML structure must be valid. For owned projections, anything
    /// that does not match the effective schema is an error.
    /// Older parts are projected into the current model; their source data
    /// and original versions are retained by `OwnedPackage` and `XmlPart`.
    #[default]
    Strict,
    /// Preserve nonstandard package envelopes with warnings. In an owned
    /// projection, unknown data, invalid optional values and unknown optional choices
    /// are omitted from the typed projection with warnings. `OwnedPackage` and
    /// `XmlPart` retain their source data when writing. Required data must
    /// still be valid.
    Lenient,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOptions {
    pub mode: Mode,
    /// Maximum element nesting in a part. The universal tree is iterative;
    /// the recursive owned projection additionally caps depth at 48.
    pub max_depth: usize,
    /// Maximum total size of decompressed entries, in bytes. Producer
    /// construction checks its input elements and envelope against this budget;
    /// the owned projection may generate different output. This does not bound
    /// the input buffer, ZIP metadata, XML arena or validation work.
    pub max_size: u64,
    /// Maximum ZIP central-directory entries, including directory entries.
    pub max_entries: usize,
    /// Maximum uncompressed size of one input ZIP entry, standalone XML part,
    /// or initially built producer part, in bytes. Later edits and unchecked
    /// writes do not retain this budget; check them separately.
    pub max_entry_size: u64,
    /// Maximum XML nodes per part, excluding the synthetic document node but
    /// including text, comments, declarations and processing instructions.
    /// For producer parts this includes their generated XML declaration.
    pub max_nodes: usize,
    /// Maximum total XML attributes per part, including namespace declarations.
    pub max_attributes: usize,
}

impl Default for ReadOptions {
    fn default() -> Self {
        ReadOptions {
            mode: Mode::Strict,
            max_depth: 4096,
            max_size: 1 << 30,
            max_entries: 65_534,
            max_entry_size: 1 << 30,
            max_nodes: 1_000_000,
            max_attributes: 2_000_000,
        }
    }
}

impl ReadOptions {
    /// Explicit spelling of the universal source-tree defaults.
    pub fn source() -> Self {
        Self::default()
    }
    pub fn projection() -> Self {
        Self { max_depth: 48, ..Self::default() }
    }

    pub fn lenient() -> Self {
        ReadOptions { mode: Mode::Lenient, ..ReadOptions::default() }
    }
}

/// A read result with the warnings collected while reading.
#[derive(Debug, Clone, PartialEq)]
#[must_use]
pub struct Parsed<T> {
    pub value: T,
    pub warnings: Vec<Warning>,
}
