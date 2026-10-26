//! IDML packages: the ZIP container around the designmap and its parts.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;

use crate::archive;
use crate::element::Element;
use crate::error::{Error, ErrorKind, Location, Warning};
use crate::layout::{Compression, Entry, Layout};
use crate::model::{DOM_VERSION, Document, INDESIGN_VERSION};
use crate::value::PartPath;
use crate::xml::{Content, Ctx, DECLARATION, PACKAGING_NS, Writer, read_part, serialize};

/// The content of the `mimetype` entry.
pub const MIMETYPE: &str = "application/vnd.adobe.indesign-idml-package";

const DESIGNMAP: &str = "designmap.xml";
const CONTAINER: &str = "META-INF/container.xml";
const METADATA: &str = "META-INF/metadata.xml";

/// An IDML package.
///
/// The designmap [`Document`] holds the other parts where it references
/// them, so a package cannot reference a part it does not contain.
///
/// Packages compare by content: the [`Layout`] is not compared.
#[derive(Debug, Clone)]
pub struct Package {
    pub document: Document,
    /// `META-INF/metadata.xml`, an XMP packet. The IDML schema does not
    /// cover it; it is kept verbatim.
    pub metadata: Option<String>,
    /// Entries that the designmap does not reference, kept verbatim.
    pub other_files: BTreeMap<String, Vec<u8>>,
    /// How the package is written: recorded when it is read, empty for a new
    /// package (written as InDesign writes packages).
    pub layout: Layout,
}

impl PartialEq for Package {
    fn eq(&self, other: &Self) -> bool {
        self.document == other.document && self.metadata == other.metadata && self.other_files == other.other_files
    }
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
        Package { document, metadata: None, other_files: BTreeMap::new(), layout: Layout::default() }
    }

    /// Reads a package from the bytes of an `.idml` file, with its layout.
    pub fn read(bytes: &[u8], options: &ReadOptions) -> Result<Parsed<Package>, Error> {
        let mut ctx = Ctx::new(options, "");
        let (files, entries) = unzip(bytes, options)?;
        ctx.files = files;
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
        let mut layout = Layout { entries, ..Layout::default() };
        let designmap = match ctx.files.remove(CONTAINER) {
            Some(c) => {
                let designmap = root_file(&c).map_err(|m| package_error(m, CONTAINER))?;
                layout.container = String::from_utf8(c).ok();
                designmap
            }
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
        ctx.part = designmap.clone();
        let document: Document = read_part(&mut ctx, &xml, "Document")?;
        // The `aid` instruction of an older version would name it.
        if !ctx.upgraded.contains(&designmap) {
            layout.aid = aid(&xml);
        }

        let mut other_files = std::mem::take(&mut ctx.files);
        other_files.retain(|path, _| !ctx.used.contains(path));
        for path in other_files.keys() {
            ctx.warn("not referenced by the designmap; kept in other_files".to_owned(), Location { part: path.clone(), ..Location::default() });
        }
        layout.parts = std::mem::take(&mut ctx.layouts);
        // The designmap is written at its usual path.
        if let Some(l) = layout.parts.remove(&designmap) {
            layout.parts.insert(DESIGNMAP.to_owned(), l);
        }
        Ok(Parsed { value: Package { document, metadata, other_files, layout }, warnings: ctx.into_warnings() })
    }

    /// Reads a package from elements: the designmap's root element
    /// (`Document`) and the root element of each part it references, by path.
    ///
    /// The elements are read as content of this library's `DOMVersion`, so
    /// part roots leave out `DOMVersion` and the `xmlns:idPkg` declaration;
    /// [`Layout::dom_version`] sets the version written. The order of
    /// attributes and children becomes the package's layout.
    pub fn from_elements(designmap: &Element, parts: &BTreeMap<String, Element>, options: &ReadOptions) -> Result<Parsed<Package>, Error> {
        let to_xml = |path: &str, root: &Element| -> Result<Vec<u8>, Error> {
            if root.attributes.iter().any(|(n, _)| n == "DOMVersion" || n == "xmlns:idPkg") {
                let message = "the library writes DOMVersion and xmlns:idPkg; set Layout::dom_version for another version";
                return Err(Error::new(ErrorKind::Package, message).in_part(path));
            }
            let mut out = String::from(DECLARATION);
            serialize(&mut out, root, &[("xmlns:idPkg", PACKAGING_NS), ("DOMVersion", DOM_VERSION)], None).map_err(|e| e.in_part(path))?;
            Ok(out.into_bytes())
        };
        let mut ctx = Ctx::new(options, DESIGNMAP);
        for (path, root) in parts {
            ctx.files.insert(path.clone(), to_xml(path, root)?);
        }
        let xml = String::from_utf8(to_xml(DESIGNMAP, designmap)?).map_err(|_| Error::new(ErrorKind::Package, "not UTF-8"))?;
        let document: Document = read_part(&mut ctx, &xml, "Document")?;
        if let Some(path) = ctx.files.keys().find(|path| !ctx.used.contains(*path)) {
            return Err(Error::new(ErrorKind::Package, "not referenced by the designmap").in_part(path));
        }
        let layout = Layout { parts: std::mem::take(&mut ctx.layouts), ..Layout::default() };
        let package = Package { document, metadata: None, other_files: BTreeMap::new(), layout };
        Ok(Parsed { value: package, warnings: ctx.into_warnings() })
    }

    /// Writes the package to `out` and returns it.
    pub fn write_to<W: Write>(&self, mut out: W) -> Result<W, Error> {
        out.write_all(&self.write()?)?;
        Ok(out)
    }

    /// Writes the package as the bytes of an `.idml` file.
    pub fn write(&self) -> Result<Vec<u8>, Error> {
        let layout = &self.layout;
        let pi = match &layout.aid {
            Some(aid) => aid.clone(),
            None => format!("aid style=\"50\" type=\"document\" readerVersion=\"6.0\" featureSet=\"257\" product=\"{}\" ", product()),
        };
        let designmap = Writer::document(layout, DESIGNMAP, "Document", Some(&pi), |w| self.document.write(w))?;

        let mut entries: Vec<(&str, &[u8])> = vec![("mimetype", MIMETYPE.as_bytes())];
        entries.push((CONTAINER, layout.container.as_deref().unwrap_or(CONTAINER_XML).as_bytes()));
        if let Some(m) = &self.metadata {
            entries.push((METADATA, m.as_bytes()));
        }
        entries.push((DESIGNMAP, designmap.xml.as_bytes()));
        entries.extend(designmap.parts.iter().map(|(p, x)| (p.as_str(), x.as_bytes())));
        entries.extend(self.other_files.iter().map(|(p, b)| (p.as_str(), b.as_slice())));
        let mut paths = BTreeSet::new();
        for (path, _) in &entries {
            if !paths.insert(*path) {
                return Err(Error::new(ErrorKind::Package, format!("two package entries share the path {path}")));
            }
        }
        // The layout's order, then InDesign's.
        let listed: HashMap<&str, (usize, Compression)> =
            layout.entries.iter().enumerate().map(|(i, e)| (e.path.as_str(), (i, e.compression))).collect();
        let position = |path: &str| match listed.get(path) {
            Some((i, _)) => (0, *i),
            None => (1, indesign_rank(path)),
        };
        // Stable: parts of one kind stay in designmap order.
        entries.sort_by_key(|(path, _)| position(path));

        let files: Vec<(&str, &[u8], Compression)> = entries
            .into_iter()
            .map(|(path, bytes)| {
                let compression = match listed.get(path) {
                    Some((_, c)) => *c,
                    None if path == "mimetype" => Compression::Stored,
                    None => Compression::Deflated,
                };
                (path, bytes, compression)
            })
            .collect();
        archive::write(&files)
    }
}

/// The position of an entry in InDesign's packages.
fn indesign_rank(path: &str) -> usize {
    const ORDER: &[&str] = &[
        "mimetype",
        DESIGNMAP,
        CONTAINER,
        METADATA,
        "Resources/Graphic.xml",
        "Resources/Fonts.xml",
        "Resources/Styles.xml",
        "Resources/Preferences.xml",
        "XML/Tags.xml",
        "MasterSpreads/",
        "Spreads/",
        "XML/BackingStory.xml",
        "Stories/",
        "XML/Mapping.xml",
    ];
    ORDER.iter().position(|p| if p.ends_with('/') { path.starts_with(p) } else { path == *p }).unwrap_or(ORDER.len())
}

/// The `aid` processing instruction before the root element of a designmap.
fn aid(xml: &str) -> Option<String> {
    let root = xml.find("<Document")?;
    let start = xml.get(..root)?.find("<?aid ")?;
    let end = xml.get(start..root)?.find("?>")?;
    xml.get(start + 2..start + end).map(str::to_owned)
}

const CONTAINER_XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n\
\t<rootfiles>\n\t\t<rootfile full-path=\"designmap.xml\" media-type=\"text/xml\">\n\t\t</rootfile>\n\t</rootfiles>\n</container>\n";

/// `21.5(64)` for InDesign 21.5.0.64, as the designmap's `aid` instruction
/// names the product.
fn product() -> String {
    let parts: Vec<&str> = INDESIGN_VERSION.split('.').collect();
    match parts.as_slice() {
        [major, minor, .., build] => format!("{major}.{minor}({build})"),
        _ => INDESIGN_VERSION.to_owned(),
    }
}

type Files = BTreeMap<String, Vec<u8>>;

/// The entries of a ZIP archive, and their order and compression.
fn unzip(bytes: &[u8], options: &crate::ReadOptions) -> Result<(Files, Vec<Entry>), Error> {
    let mut files = BTreeMap::new();
    let mut entries = Vec::new();
    for f in archive::read(bytes, options.max_size)? {
        if files.insert(f.name.clone(), f.data).is_some() {
            return Err(Error::new(ErrorKind::Package, format!("duplicate entry {}", f.name)));
        }
        entries.push(Entry { path: f.name, compression: f.compression });
    }
    Ok((files, entries))
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

    #[test]
    fn finds_the_aid_instruction() {
        let xml = "<?xml version=\"1.0\"?>\n<?aid style=\"50\" product=\"20.4(52)\" ?>\n<Document/>";
        assert_eq!(aid(xml).as_deref(), Some("aid style=\"50\" product=\"20.4(52)\" "));
        assert_eq!(aid("<Document/><?aid x?>"), None);
    }

    #[test]
    fn entries_follow_indesign_order() {
        let mut paths = ["Stories/Story_u1.xml", "Resources/Fonts.xml", "designmap.xml", "Spreads/Spread_u2.xml", "mimetype", "other.txt"];
        paths.sort_by_key(|p| indesign_rank(p));
        assert_eq!(paths, ["mimetype", "designmap.xml", "Resources/Fonts.xml", "Spreads/Spread_u2.xml", "Stories/Story_u1.xml", "other.txt"]);
    }
}
