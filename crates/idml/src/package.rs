//! IDML packages: the ZIP container around the designmap and its parts.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Seek, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::error::{Error, ErrorKind, Location, Warning};
use crate::model::{Document, INDESIGN_VERSION};
use crate::value::PartPath;
use crate::xml::{Content, Ctx, Writer, read_part};

/// The content of the `mimetype` entry.
pub const MIMETYPE: &str = "application/vnd.adobe.indesign-idml-package";

const DESIGNMAP: &str = "designmap.xml";
const CONTAINER: &str = "META-INF/container.xml";
const METADATA: &str = "META-INF/metadata.xml";

/// An IDML package.
///
/// The designmap [`Document`] holds the other parts where it references
/// them, so a package cannot reference a part it does not contain.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub document: Document,
    /// `META-INF/metadata.xml`, an XMP packet. The IDML schema does not
    /// cover it; it is kept verbatim.
    pub metadata: Option<String>,
    /// Entries that the designmap does not reference, kept verbatim.
    pub other_files: BTreeMap<String, Vec<u8>>,
}

/// A package part with a path chosen by the document (spreads, master
/// spreads, stories).
#[derive(Debug, Clone, PartialEq)]
pub struct Part<T> {
    pub src: PartPath,
    pub content: T,
}

impl<T> Part<T> {
    pub fn new(src: PartPath, content: T) -> Self {
        Part { src, content }
    }
}

/// How strictly to read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    /// Anything that does not match the schema is an error. Parts of older
    /// `DOMVersion`s are upgraded first (see the crate documentation).
    #[default]
    Strict,
    /// Unknown attributes and elements, invalid optional values and newer
    /// `DOMVersion`s are skipped with a warning. Required data must still be
    /// valid.
    Lenient,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOptions {
    pub mode: Mode,
    /// Maximum element nesting in a part. Reading needs about 1 KiB of stack
    /// per level in optimized builds and about 32 KiB in unoptimized ones; the
    /// default fits the 2 MiB of a test thread. Realistic documents nest
    /// fewer than 25 levels.
    pub max_depth: usize,
    /// Maximum total size of the decompressed entries, in bytes.
    pub max_size: u64,
}

impl Default for ReadOptions {
    fn default() -> Self {
        ReadOptions { mode: Mode::Strict, max_depth: 48, max_size: 1 << 30 }
    }
}

impl ReadOptions {
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

/// The root of a package part (`StoryFile`, `SpreadFile`, …): readable and
/// writable as a standalone XML document.
pub trait PartFile: Sized + crate::sealed::Sealed {
    /// The root element, such as `idPkg:Story`.
    const ELEMENT: &'static str;
    /// The schema file, such as `Stories/Story.rnc`.
    const SCHEMA: &'static str;

    fn from_xml(xml: &str, options: &ReadOptions) -> Result<Parsed<Self>, Error>;

    fn to_xml(&self) -> Result<String, Error>;
}

impl Package {
    pub fn new(document: Document) -> Self {
        Package { document, metadata: None, other_files: BTreeMap::new() }
    }

    /// Reads a package from the bytes of an `.idml` file.
    pub fn read(bytes: &[u8], options: &ReadOptions) -> Result<Parsed<Package>, Error> {
        let mut ctx = Ctx::new(options, "");
        ctx.files = unzip(bytes, options)?;
        let package_error =
            |message: String, part: &str| Error::new(ErrorKind::Package, message).at(Location { part: part.to_owned(), ..Location::default() });
        let notice = |ctx: &mut Ctx, message: String, part: &str| -> Result<(), Error> {
            match options.mode {
                Mode::Strict => Err(package_error(message, part)),
                Mode::Lenient => {
                    ctx.warn(message, Location { part: part.to_owned(), ..Location::default() });
                    Ok(())
                }
            }
        };

        match ctx.files.remove("mimetype") {
            Some(m) if m == MIMETYPE.as_bytes() => {}
            Some(_) => notice(&mut ctx, format!("mimetype is not {MIMETYPE}"), "mimetype")?,
            None => notice(&mut ctx, "missing mimetype".to_owned(), "mimetype")?,
        }
        let designmap = match ctx.files.remove(CONTAINER) {
            Some(c) => root_file(&c).map_err(|m| package_error(m, CONTAINER))?,
            None => {
                notice(&mut ctx, "missing container; assuming designmap.xml".to_owned(), CONTAINER)?;
                DESIGNMAP.to_owned()
            }
        };
        let metadata = match ctx.files.remove(METADATA) {
            Some(m) => Some(String::from_utf8(m).map_err(|_| package_error("not UTF-8".to_owned(), METADATA))?),
            None => None,
        };
        let xml = ctx.files.remove(&designmap).ok_or_else(|| package_error("missing designmap".to_owned(), &designmap))?;
        let xml = String::from_utf8(xml).map_err(|_| package_error("not UTF-8".to_owned(), &designmap))?;
        ctx.part = designmap;
        let document: Document = read_part(&mut ctx, &xml, "Document")?;

        let mut other_files = std::mem::take(&mut ctx.files);
        other_files.retain(|path, _| !ctx.used.contains(path));
        for path in other_files.keys() {
            ctx.warn("not referenced by the designmap; kept in other_files".to_owned(), Location { part: path.clone(), ..Location::default() });
        }
        Ok(Parsed { value: Package { document, metadata, other_files }, warnings: ctx.into_warnings() })
    }

    /// Writes the package as the bytes of an `.idml` file.
    pub fn write(&self) -> Result<Vec<u8>, Error> {
        Ok(self.write_to(Cursor::new(Vec::new()))?.into_inner())
    }

    /// Writes the package to `out` and returns it.
    pub fn write_to<W: Write + Seek>(&self, out: W) -> Result<W, Error> {
        let pi = format!("aid style=\"50\" type=\"document\" readerVersion=\"6.0\" featureSet=\"257\" product=\"{}\" ", product());
        let designmap = Writer::document(DESIGNMAP, "Document", Some(&pi), |w| self.document.write(w))?;

        let mut paths: BTreeSet<&str> = [DESIGNMAP, CONTAINER, METADATA, "mimetype"].into();
        let parts = designmap.parts.iter().map(|(p, x)| (p.as_str(), x.as_bytes()));
        let others = self.other_files.iter().map(|(p, b)| (p.as_str(), b.as_slice()));
        let entries: Vec<(&str, &[u8])> = parts.chain(others).collect();
        for (path, _) in &entries {
            if !paths.insert(path) {
                return Err(Error::new(ErrorKind::Package, format!("two package entries share the path {path}")));
            }
        }

        let mut zip = ZipWriter::new(out);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("mimetype", stored)?;
        zip.write_all(MIMETYPE.as_bytes())?;
        zip.start_file(CONTAINER, deflated)?;
        zip.write_all(CONTAINER_XML.as_bytes())?;
        if let Some(m) = &self.metadata {
            zip.start_file(METADATA, deflated)?;
            zip.write_all(m.as_bytes())?;
        }
        zip.start_file(DESIGNMAP, deflated)?;
        zip.write_all(designmap.xml.as_bytes())?;
        for (path, bytes) in entries {
            zip.start_file(path, deflated)?;
            zip.write_all(bytes)?;
        }
        Ok(zip.finish()?)
    }
}

const CONTAINER_XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n\
\t<rootfiles>\n\t\t<rootfile full-path=\"designmap.xml\" media-type=\"text/xml\"/>\n\t</rootfiles>\n</container>\n";

/// `21.5(64)` for InDesign 21.5.0.64, as the designmap's `aid` instruction
/// names the product.
fn product() -> String {
    let parts: Vec<&str> = INDESIGN_VERSION.split('.').collect();
    match parts.as_slice() {
        [major, minor, .., build] => format!("{major}.{minor}({build})"),
        _ => INDESIGN_VERSION.to_owned(),
    }
}

fn unzip(bytes: &[u8], options: &crate::ReadOptions) -> Result<BTreeMap<String, Vec<u8>>, Error> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut files = BTreeMap::new();
    let mut total: u64 = 0;
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name()?.into_owned();
        let budget = options.max_size.saturating_sub(total);
        let mut data = Vec::new();
        entry.take(budget.saturating_add(1)).read_to_end(&mut data)?;
        total = total.saturating_add(data.len() as u64);
        if total > options.max_size {
            return Err(Error::new(ErrorKind::Limit, format!("package expands beyond {} bytes", options.max_size)));
        }
        if files.insert(name.clone(), data).is_some() {
            return Err(Error::new(ErrorKind::Package, format!("duplicate entry {name}")));
        }
    }
    Ok(files)
}

/// The designmap path named by `META-INF/container.xml`.
fn root_file(container: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(container).map_err(|_| "not UTF-8".to_owned())?;
    if crate::xml::nesting_exceeds(text, 8) {
        return Err("nested too deeply".to_owned());
    }
    let doc = roxmltree::Document::parse(text).map_err(|e| e.to_string())?;
    doc.descendants()
        .find(|n| n.tag_name().name() == "rootfile")
        .and_then(|n| n.attribute("full-path"))
        .map(str::to_owned)
        .ok_or_else(|| "no rootfile".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_name() {
        assert!(product().contains('('));
    }
}
