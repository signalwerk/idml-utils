//! The XML layer between the generated model and package parts.

mod read;
mod write;

pub(crate) use read::{Ctx, Reader, nesting_exceeds, qualified, read_part};
pub(crate) use write::Writer;

use crate::error::Error;
use crate::{Parsed, PartFile, ReadOptions};

/// The IDML packaging namespace, written with the `idPkg` prefix.
pub(crate) const PACKAGING_NS: &str = "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging";

/// A generated type that is the content of an element: its attributes and
/// children. The element name comes from where the content occurs.
///
/// Values are read in place: `read_into` fills a `placeholder`, which may sit
/// in its final heap location. The recursive readers' stack frames therefore
/// do not grow with the size of the (large) generated structs.
pub(crate) trait Content: Sized {
    /// A value for `read_into` to overwrite; never visible to users.
    fn placeholder() -> Self;

    fn read_into(r: &mut Reader<'_, '_, '_>, value: &mut Self) -> Result<(), Error>;

    /// Writes the attributes, then the children.
    fn write(&self, w: &mut Writer) -> Result<(), Error>;

    fn read(r: &mut Reader<'_, '_, '_>) -> Result<Self, Error> {
        let mut value = Self::placeholder();
        Self::read_into(r, &mut value)?;
        Ok(value)
    }
}

/// The root content of a package part.
pub(crate) trait Root: Content + PartFile {}

impl<T: Content + PartFile> Root for T {}

/// Characters that XML 1.0 cannot contain but IDML text can, as `<?ACE n?>`.
pub(crate) fn needs_ace(c: char) -> bool {
    c < ' ' && !matches!(c, '\t' | '\n' | '\r')
}

/// [`PartFile::from_xml`] for generated types.
pub(crate) fn from_xml<T: Root>(xml: &str, options: &ReadOptions) -> Result<Parsed<T>, Error> {
    let mut ctx = Ctx::new(options, "");
    let value = read_part(&mut ctx, xml, T::ELEMENT)?;
    Ok(Parsed { value, warnings: ctx.into_warnings() })
}

/// [`PartFile::to_xml`] for generated types.
pub(crate) fn to_xml<T: Root>(value: &T) -> Result<String, Error> {
    Ok(Writer::document("", T::ELEMENT, None, |w| value.write(w))?.xml)
}
