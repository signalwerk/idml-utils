//! Errors and warnings.

use std::fmt;

/// Where in a package a problem is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Location {
    /// The package path, such as `Stories/Story_u1d8.xml`; empty for XML that
    /// was not read from a package.
    pub part: String,
    /// 1-based line and column; 0 when unknown.
    pub line: u32,
    pub column: u32,
    /// The element path from the part root, such as
    /// `idPkg:Story/Story/ParagraphStyleRange[2]`.
    pub element: String,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let part = if self.part.is_empty() { "<xml>" } else { &self.part };
        write!(f, "{part}")?;
        if self.line > 0 {
            write!(f, ":{}:{}", self.line, self.column)?;
        }
        if !self.element.is_empty() {
            write!(f, " ({})", self.element)?;
        }
        Ok(())
    }
}

/// What kind of problem an [`Error`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The ZIP archive cannot be read or written.
    Zip,
    /// A part is not well-formed XML.
    Xml,
    /// The XML does not match the IDML schema.
    Schema,
    /// The package structure is wrong: missing or duplicate parts, an
    /// invalid mimetype, a text that XML cannot represent.
    Package,
    /// A size or nesting limit from `ReadOptions` was exceeded.
    Limit,
}

/// A failure to read or write IDML.
///
/// It is one pointer wide, so the many `Result`s in generated code stay small.
#[derive(Debug, Clone, PartialEq)]
pub struct Error(Box<Inner>);

#[derive(Debug, Clone, PartialEq)]
struct Inner {
    kind: ErrorKind,
    message: String,
    location: Option<Location>,
}

impl Error {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Error(Box::new(Inner { kind, message: message.into(), location: None }))
    }

    pub(crate) fn at(mut self, location: Location) -> Self {
        self.0.location = Some(location);
        self
    }

    pub fn kind(&self) -> ErrorKind {
        self.0.kind
    }

    pub fn message(&self) -> &str {
        &self.0.message
    }

    pub fn location(&self) -> Option<&Location> {
        self.0.location.as_ref()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0.location {
            Some(l) => write!(f, "{l}: {}", self.0.message),
            None => f.write_str(&self.0.message),
        }
    }
}

impl std::error::Error for Error {}

impl From<zip::result::ZipError> for Error {
    fn from(e: zip::result::ZipError) -> Self {
        Error::new(ErrorKind::Zip, e.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::new(ErrorKind::Zip, e.to_string())
    }
}

/// Something a lenient read skipped or a read noticed without failing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub message: String,
    pub location: Location,
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.location, self.message)
    }
}
