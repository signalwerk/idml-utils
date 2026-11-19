//! IDML packages: the ZIP container around the designmap and its parts.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;

use crate::archive;
use crate::element::Element;
use crate::error::{Error, ErrorKind, Location};
use crate::layout::{Compression, Entry, Layout};
use crate::model::{DOM_VERSION, Document, INDESIGN_VERSION};
use crate::package::root_file;
use crate::preserve::SourcePart;
use crate::value::PartPath;
use crate::xml::{Content, Ctx, DECLARATION, PACKAGING_NS, Writer, read_part};
use crate::{MIMETYPE, Mode, Parsed, ReadOptions};

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
pub struct OwnedPackage {
    pub document: Document,
    /// `META-INF/metadata.xml`, an XMP packet. The IDML schema does not
    /// cover it; it is kept verbatim.
    pub metadata: Option<String>,
    /// Entries that the designmap does not reference, kept verbatim.
    pub other_files: BTreeMap<String, Vec<u8>>,
    /// How the package is written: recorded when it is read, empty for a new
    /// package (written as InDesign writes packages).
    pub layout: Layout,
    source: BTreeMap<String, SourcePart>,
    envelope: Option<Envelope>,
}

#[derive(Debug, Clone)]
struct Envelope {
    mimetype: Option<Vec<u8>>,
    container_present: bool,
}

impl PartialEq for OwnedPackage {
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

/// The root of a package part (`StoryFile`, `SpreadFile`, …): readable and
/// writable as a standalone XML document. These methods operate on the
/// typed projection; use `XmlPart<Self>` to retain source XML and unknown data.
pub trait PartFile: Sized + crate::sealed::Sealed {
    /// The root element, such as `idPkg:Story`.
    const ELEMENT: &'static str;
    /// The schema file, such as `Stories/Story.rnc`.
    const SCHEMA: &'static str;

    fn from_xml(xml: &str, options: &ReadOptions) -> Result<Parsed<Self>, Error>;

    fn to_xml(&self) -> Result<String, Error>;
}

impl OwnedPackage {
    pub fn new(document: Document) -> Self {
        OwnedPackage { document, metadata: None, other_files: BTreeMap::new(), layout: Layout::default(), source: BTreeMap::new(), envelope: None }
    }

    /// Reads a package from the bytes of an `.idml` file, with its layout.
    pub fn read(bytes: &[u8], options: &ReadOptions) -> Result<Parsed<OwnedPackage>, Error> {
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

        let original_mimetype = ctx.files.remove("mimetype");
        let container_present = ctx.files.contains_key(CONTAINER);
        match &original_mimetype {
            Some(m) if m.as_slice() == MIMETYPE.as_bytes() => {}
            Some(_) => notice(&mut ctx, format!("mimetype is not {MIMETYPE}"), "mimetype")?,
            None => notice(&mut ctx, "missing mimetype".to_owned(), "mimetype")?,
        }
        let mut layout = Layout { entries, ..Layout::default() };
        let designmap = match ctx.files.remove(CONTAINER) {
            Some(c) => {
                if let Ok(xml) = std::str::from_utf8(&c) {
                    crate::tree::check_xml_counts(xml, options).map_err(|e| e.in_part(CONTAINER))?;
                }
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
        layout.designmap_path = Some(designmap);
        let envelope = Some(Envelope { mimetype: original_mimetype, container_present });
        let mut package = OwnedPackage { document, metadata, other_files, layout, source: BTreeMap::new(), envelope };
        package.record_source(std::mem::take(&mut ctx.sources), true)?;
        Ok(Parsed { value: package, warnings: ctx.into_warnings() })
    }

    /// Reads a package from elements: the designmap's root element
    /// (`Document`) and the root element of each part it references, by path.
    ///
    /// The elements are read as content of this library's `DOMVersion`, so
    /// part roots leave out `DOMVersion` and the `xmlns:idPkg` declaration.
    /// For any other version, use `Package`, not this projection. The order of
    /// attributes and children becomes the package's layout.
    pub fn from_elements(designmap: &Element, parts: &BTreeMap<String, Element>, options: &ReadOptions) -> Result<Parsed<OwnedPackage>, Error> {
        if parts.len().saturating_add(3) > options.max_entries {
            return Err(Error::new(ErrorKind::Limit, "package exceeds max_entries"));
        }
        crate::package::check_producer_envelope(options, CONTAINER_XML)?;
        let mut total_size = (MIMETYPE.len() + CONTAINER_XML.len()) as u64;
        let mut to_tree = |path: &str, root: &Element| -> Result<crate::XmlTree, Error> {
            if root.attributes.iter().any(|(n, _)| n == "DOMVersion" || n == "xmlns:idPkg") {
                return Err(Error::new(ErrorKind::Package, "the library supplies DOMVersion and xmlns:idPkg for typed producers").in_part(path));
            }
            let remaining = ReadOptions { max_size: options.max_size.saturating_sub(total_size), ..options.clone() };
            let (tree, size) = crate::XmlTree::from_element_with(root, &[("xmlns:idPkg", PACKAGING_NS), ("DOMVersion", DOM_VERSION)], &remaining)?;
            total_size = total_size.saturating_add(size);
            if total_size > options.max_size {
                return Err(Error::new(ErrorKind::Limit, "producer package exceeds max_size"));
            }
            tree.check_limits(options)?;
            Ok(tree)
        };
        let mut ctx = Ctx::new(options, DESIGNMAP);
        ctx.capture_source = false;
        for (path, root) in parts {
            ctx.element_parts.insert(path.clone(), to_tree(path, root)?);
        }
        let root = to_tree(DESIGNMAP, designmap)?;
        let document: Document = crate::xml::read_element_part(&mut ctx, &root, "Document")?;
        if let Some(path) = ctx.element_parts.keys().find(|path| !ctx.used.contains(*path)) {
            return Err(Error::new(ErrorKind::Package, "not referenced by the designmap").in_part(path));
        }
        let layout = Layout { parts: std::mem::take(&mut ctx.layouts), ..Layout::default() };
        let mut package = OwnedPackage { document, metadata: None, other_files: BTreeMap::new(), layout, source: BTreeMap::new(), envelope: None };
        let element_parts = std::mem::take(&mut ctx.element_parts);
        let warnings = ctx.into_warnings();
        if !warnings.is_empty() {
            let mut sources = BTreeMap::new();
            sources.insert(DESIGNMAP.into(), format!("{DECLARATION}{}", root.to_xml()?));
            for (path, tree) in element_parts {
                sources.insert(path, format!("{DECLARATION}{}", tree.to_xml()?));
            }
            package.record_source(sources, false)?;
        }
        Ok(Parsed { value: package, warnings })
    }

    /// Original XML before schema projection or migration, including unknown data.
    pub fn original_xml(&self, path: &str) -> Option<&str> {
        self.source.get(path).map(|s| s.original.as_str())
    }

    /// Explicitly discard the source and serialize only the typed projection.
    /// This can lose unknown data and fields removed by migration. Clear the
    /// layout as well to normalize serialization to the current InDesign version.
    pub fn discard_source(&mut self) {
        self.source.clear();
        self.envelope = None;
    }

    fn record_source(&mut self, mut originals: BTreeMap<String, String>, preserve_version: bool) -> Result<(), Error> {
        let written = self.written()?;
        let path = self.layout.designmap_path.as_deref().unwrap_or(DESIGNMAP);
        for (path, baseline) in std::iter::once((path.to_owned(), written.xml)).chain(written.parts) {
            if let Some(original) = originals.remove(&path) {
                self.source.insert(path, SourcePart { original, baseline, preserve_version });
            }
        }
        Ok(())
    }

    fn written(&self) -> Result<crate::xml::Written, Error> {
        let layout = &self.layout;
        let pi = match &layout.aid {
            Some(aid) => aid.clone(),
            None => format!("aid style=\"50\" type=\"document\" readerVersion=\"6.0\" featureSet=\"257\" product=\"{}\" ", product()),
        };
        let path = layout.designmap_path.as_deref().unwrap_or(DESIGNMAP);
        Writer::document(layout, path, "Document", Some(&pi), |w| self.document.write(w))
    }

    /// Writes the package to `out` and returns it.
    pub fn write_to<W: Write>(&self, mut out: W) -> Result<W, Error> {
        out.write_all(&self.write()?)?;
        Ok(out)
    }

    /// Writes the package as the bytes of an `.idml` file. Source XML and
    /// versions are retained; edits that cannot be mapped safely return
    /// `ErrorKind::Write`. ZIP metadata such as timestamps is regenerated.
    pub fn write(&self) -> Result<Vec<u8>, Error> {
        let layout = &self.layout;
        let mut designmap = self.written()?;
        let path = layout.designmap_path.as_deref().unwrap_or(DESIGNMAP);
        if let Some(source) = self.source.get(path) {
            designmap.xml = source.merge(&designmap.xml).map_err(|e| e.in_part(path))?;
        }
        for (path, xml) in &mut designmap.parts {
            if let Some(source) = self.source.get(path) {
                *xml = source.merge(xml).map_err(|e| e.in_part(path))?;
            }
        }

        let mut entries: Vec<(&str, &[u8])> = Vec::new();
        let mimetype = self.envelope.as_ref().map_or(Some(MIMETYPE.as_bytes()), |e| e.mimetype.as_deref());
        if let Some(mimetype) = mimetype {
            entries.push(("mimetype", mimetype));
        }
        if self.envelope.as_ref().is_none_or(|e| e.container_present) {
            entries.push((CONTAINER, layout.container.as_deref().unwrap_or(CONTAINER_XML).as_bytes()));
        }
        if let Some(m) = &self.metadata {
            entries.push((METADATA, m.as_bytes()));
        }
        entries.push((path, designmap.xml.as_bytes()));
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
    for f in archive::read(bytes, options)? {
        if files.insert(f.name.clone(), f.data).is_some() {
            return Err(Error::new(ErrorKind::Package, format!("duplicate entry {}", f.name)));
        }
        entries.push(Entry { path: f.name, compression: f.compression });
    }
    Ok((files, entries))
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
