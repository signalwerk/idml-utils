//! Schema names to Rust identifiers.

/// Splits an XML or schema name into words: `XMLElement` → `XML`, `Element`;
/// `JIS78Form` → `JIS78`, `Form`; `Story_File` → `Story`, `File`.
pub fn words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut words = Vec::new();
    let mut word = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            continue;
        }
        let prev = i.checked_sub(1).and_then(|p| chars.get(p)).copied();
        let next = chars.get(i + 1).copied();
        let boundary = match prev {
            Some(p) if c.is_uppercase() => p.is_lowercase() || p.is_ascii_digit() || (p.is_uppercase() && next.is_some_and(char::is_lowercase)),
            _ => false,
        };
        if boundary && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        word.push(c);
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// `UpperCamelCase`, with acronyms as words: `XMLElement` → `XmlElement`.
pub fn camel(name: &str) -> String {
    let mut out = String::new();
    for w in words(name) {
        let mut chars = w.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(&chars.as_str().to_lowercase());
        }
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, 'N');
    }
    out
}

/// `snake_case`, with a trailing `_` for Rust keywords: `Self` → `self_`.
pub fn snake(name: &str) -> String {
    let mut out = words(name).iter().map(|w| w.to_lowercase()).collect::<Vec<_>>().join("_");
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, 'n');
    }
    if KEYWORDS.contains(&out.as_str()) {
        out.push('_');
    }
    out
}

const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate", "do", "dyn", "else", "enum", "extern", "false",
    "final", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof", "unsafe", "unsized", "use", "virtual", "where", "while",
    "yield",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_names() {
        assert_eq!(camel("XMLElement"), "XmlElement");
        assert_eq!(camel("HTMLFXLExportPreference"), "HtmlfxlExportPreference");
        assert_eq!(camel("InDesignUIColorType"), "InDesignUiColorType");
        assert_eq!(camel("JIS78Form"), "Jis78Form");
        assert_eq!(camel("Ppi72"), "Ppi72");
        assert_eq!(camel("object"), "Object");
        assert_eq!(snake("Self"), "self_");
        assert_eq!(snake("type"), "type_");
        assert_eq!(snake("CMYKProfileList"), "cmyk_profile_list");
        assert_eq!(snake("AppliedMathMLRgbColor"), "applied_math_ml_rgb_color");
        assert_eq!(snake("DOMVersion"), "dom_version");
    }
}
