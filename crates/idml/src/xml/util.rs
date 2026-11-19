//! XML resource checks and expanded names.
#[cfg(feature = "owned")]
use super::PACKAGING_NS;
#[cfg(feature = "owned")]
use roxmltree::Node;
#[cfg(feature = "owned")]
use std::borrow::Cow;
/// Whether elements in `xml` nest deeper than `max`. Checked before parsing
/// because the parser recurses once per level: a hostile document could
/// otherwise exhaust the stack.
pub(crate) fn nesting_exceeds(xml: &str, max: usize) -> bool {
    let b = xml.as_bytes();
    let find = |from: usize, pat: &[u8]| b.get(from..).and_then(|rest| rest.windows(pat.len()).position(|w| w == pat)).map(|p| from + p);
    let mut depth = 0usize;
    let mut i = 0;
    while let Some(start) = b.get(i..).and_then(|rest| rest.iter().position(|&c| c == b'<')).map(|p| i + p) {
        let rest = b.get(start..).unwrap_or_default();
        let end = if rest.starts_with(b"<!--") {
            find(start, b"-->").map(|e| e + 3)
        } else if rest.starts_with(b"<![CDATA[") {
            find(start, b"]]>").map(|e| e + 3)
        } else if rest.starts_with(b"<?") {
            find(start, b"?>").map(|e| e + 2)
        } else if rest.starts_with(b"<!") || rest.starts_with(b"</") {
            if rest.starts_with(b"</") {
                depth = depth.saturating_sub(1);
            }
            find(start, b">").map(|e| e + 1)
        } else {
            // A start tag: find its end outside quoted attribute values.
            let mut quote = None;
            let mut end = None;
            for (j, &c) in rest.iter().enumerate().skip(1) {
                match (quote, c) {
                    (Some(q), c) if c == q => quote = None,
                    (Some(_), _) => {}
                    (None, b'"' | b'\'') => quote = Some(c),
                    (None, b'>') => {
                        end = Some(start + j + 1);
                        if rest.get(j - 1) != Some(&b'/') {
                            depth += 1;
                        }
                        break;
                    }
                    (None, _) => {}
                }
            }
            end
        };
        if depth > max {
            return true;
        }
        // Malformed input: the parser reports it.
        let Some(end) = end else { return false };
        i = end;
    }
    false
}

#[cfg(feature = "owned")]
/// `prefix:local` for the packaging namespace, `local` without namespace,
/// `{ns}local` otherwise.
pub(crate) fn qualified<'a>(node: Node<'a, '_>) -> Cow<'a, str> {
    let name = node.tag_name();
    match name.namespace() {
        None => Cow::Borrowed(name.name()),
        Some(PACKAGING_NS) => Cow::Owned(format!("idPkg:{}", name.name())),
        Some(ns) => Cow::Owned(format!("{{{ns}}}{}", name.name())),
    }
}
