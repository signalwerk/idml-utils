//! Attribute and text values: XML Schema datatypes and their lexical forms.
//!
//! Every value type parses the lexical space of its schema datatype and
//! writes one canonical form that parses back to the same value.

use std::fmt;

/// A value that is not in a datatype's lexical or value space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueError {
    message: String,
}

impl ValueError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        ValueError { message: message.into() }
    }

    fn invalid(what: &str, text: &str) -> Self {
        Self::new(format!("{text:?} is not a valid {what}"))
    }
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ValueError {}

/// A type with an IDML lexical form.
pub trait Value: Sized {
    /// Parses the lexical form, applying the datatype's whitespace rule.
    fn parse(text: &str) -> Result<Self, ValueError>;

    /// Appends the canonical lexical form.
    fn format(&self, out: &mut String);
}

/// A value whose lexical form contains no whitespace, so it can be an item
/// of a whitespace-separated list.
pub trait ListItem: Value {}

/// XML whitespace: space, tab, line feed and carriage return.
fn is_xml_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// The `collapse` whitespace rule for single-token values.
fn collapsed(text: &str) -> &str {
    text.trim_matches(is_xml_space)
}

impl Value for String {
    fn parse(text: &str) -> Result<Self, ValueError> {
        Ok(text.to_owned())
    }

    fn format(&self, out: &mut String) {
        out.push_str(self);
    }
}

/// `xsd:boolean`: `true`, `false`, `1` or `0`; written as `true`/`false`.
impl Value for bool {
    fn parse(text: &str) -> Result<Self, ValueError> {
        match collapsed(text) {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            _ => Err(ValueError::invalid("xsd:boolean", text)),
        }
    }

    fn format(&self, out: &mut String) {
        out.push_str(if *self { "true" } else { "false" });
    }
}

impl ListItem for bool {}

/// Checks the `xsd:integer` lexical form: an optional sign and digits.
fn integer(text: &str) -> Option<&str> {
    let t = collapsed(text);
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(t)
}

macro_rules! integer_value {
    ($t:ty, $name:literal) => {
        #[doc = concat!("`", $name, "`")]
        impl Value for $t {
            fn parse(text: &str) -> Result<Self, ValueError> {
                integer(text).and_then(|t| t.strip_prefix('+').unwrap_or(t).parse().ok()).ok_or_else(|| ValueError::invalid($name, text))
            }

            fn format(&self, out: &mut String) {
                out.push_str(&self.to_string());
            }
        }

        impl ListItem for $t {}
    };
}

integer_value!(i32, "xsd:int");
integer_value!(i16, "xsd:short");

/// `xsd:double`, including `INF`, `-INF` and `NaN`. Written in the shortest
/// decimal form that reads back as the same value.
impl Value for f64 {
    fn parse(text: &str) -> Result<Self, ValueError> {
        let t = collapsed(text);
        match t {
            "INF" => return Ok(f64::INFINITY),
            "-INF" => return Ok(f64::NEG_INFINITY),
            "NaN" => return Ok(f64::NAN),
            _ => {}
        }
        // (+|-)? (digits (. digits?)? | . digits) ((e|E) (+|-)? digits)?
        let unsigned = t.strip_prefix(['+', '-']).unwrap_or(t);
        let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
            Some((m, e)) => (m, Some(e)),
            None => (unsigned, None),
        };
        let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
        let mantissa_ok = (!int.is_empty() || !frac.is_empty()) && digits(int) && digits(frac);
        let exponent_ok = exponent.is_none_or(|e| {
            let e = e.strip_prefix(['+', '-']).unwrap_or(e);
            !e.is_empty() && digits(e)
        });
        if !(mantissa_ok && exponent_ok) {
            return Err(ValueError::invalid("xsd:double", text));
        }
        t.parse().map_err(|_| ValueError::invalid("xsd:double", text))
    }

    fn format(&self, out: &mut String) {
        if self.is_nan() {
            out.push_str("NaN");
        } else if self.is_infinite() {
            out.push_str(if *self > 0.0 { "INF" } else { "-INF" });
        } else {
            out.push_str(&self.to_string());
        }
    }
}

impl ListItem for f64 {}

/// A string inside an IDML list (`list { xsd:string* }`): list items are
/// separated by whitespace, so a token is non-empty and contains none.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Token(String);

impl Token {
    pub fn new(text: impl Into<String>) -> Result<Self, ValueError> {
        let text = text.into();
        if text.is_empty() || text.contains(is_xml_space) {
            return Err(ValueError::new(format!("{text:?} is not a list token (non-empty, no whitespace)")));
        }
        Ok(Token(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }

    /// For values that are overwritten while reading.
    #[allow(dead_code)]
    pub(crate) fn placeholder() -> Self {
        Token(String::from("-"))
    }
}

impl Value for Token {
    fn parse(text: &str) -> Result<Self, ValueError> {
        Token::new(text)
    }

    fn format(&self, out: &mut String) {
        out.push_str(&self.0);
    }
}

impl ListItem for Token {}

/// A path inside the package, such as `Stories/Story_u1d8.xml`
/// (`xsd:string { pattern = ".*\.xml" }`). It must be relative, use `/`
/// separators, have no empty, `.` or `..` segments, and end in `.xml`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PartPath(String);

impl PartPath {
    pub fn new(path: impl Into<String>) -> Result<Self, ValueError> {
        let path = path.into();
        let segments_ok = path.split('/').all(|s| !s.is_empty() && s != "." && s != "..");
        let chars_ok = !path.contains(['\n', '\r', '\\']);
        if !(path.ends_with(".xml") && segments_ok && chars_ok) {
            return Err(ValueError::new(format!("{path:?} is not a package part path ending in .xml")));
        }
        Ok(PartPath(path))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// For values that are overwritten while reading.
    #[allow(dead_code)]
    pub(crate) fn placeholder() -> Self {
        PartPath(String::from("part.xml"))
    }
}

impl Value for PartPath {
    fn parse(text: &str) -> Result<Self, ValueError> {
        PartPath::new(text)
    }

    fn format(&self, out: &mut String) {
        out.push_str(&self.0);
    }
}

/// An `xsd:dateTime`, kept in its lexical form (for example
/// `2026-10-23T09:30:00Z`). Equality compares the lexical form.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DateTime(String);

impl DateTime {
    pub fn new(text: impl Into<String>) -> Result<Self, ValueError> {
        let text = text.into();
        let t = collapsed(&text);
        if !valid_date_time(t) {
            return Err(ValueError::invalid("xsd:dateTime", &text));
        }
        Ok(DateTime(t.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// For values that are overwritten while reading.
    #[allow(dead_code)]
    pub(crate) fn placeholder() -> Self {
        DateTime(String::from("2000-01-01T00:00:00"))
    }
}

impl Value for DateTime {
    fn parse(text: &str) -> Result<Self, ValueError> {
        DateTime::new(text)
    }

    fn format(&self, out: &mut String) {
        out.push_str(&self.0);
    }
}

impl ListItem for DateTime {}

/// `-?yyyy-mm-ddThh:mm:ss(.s+)?(Z|(+|-)hh:mm)?` with field ranges checked.
fn valid_date_time(t: &str) -> bool {
    let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let two = |s: &str| s.len() == 2 && all_digits(s);
    let t = t.strip_prefix('-').unwrap_or(t);
    let Some((date, time)) = t.split_once('T') else { return false };
    let mut d = date.splitn(3, '-');
    let (Some(y), Some(m), Some(dd)) = (d.next(), d.next(), d.next()) else { return false };
    if y.len() < 4 || (y.len() > 4 && y.starts_with('0')) || !all_digits(y) || !two(m) || !two(dd) {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day)) = (y.parse::<u64>(), m.parse::<u32>(), dd.parse::<u32>()) else { return false };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    if year == 0 || day == 0 || day > days {
        return false;
    }
    let (clock, zone) = match time.find(['Z', '+', '-']) {
        Some(i) => (time.get(..i).unwrap_or(""), time.get(i..)),
        None => (time, None),
    };
    let mut c = clock.splitn(3, ':');
    let (Some(h), Some(mi), Some(s)) = (c.next(), c.next(), c.next()) else { return false };
    let (sec, frac) = match s.split_once('.') {
        Some((sec, frac)) => (sec, Some(frac)),
        None => (s, None),
    };
    if !two(h) || !two(mi) || !two(sec) || frac.is_some_and(|f| !all_digits(f)) {
        return false;
    }
    let (Ok(h), Ok(mi), Ok(sec)) = (h.parse::<u32>(), mi.parse::<u32>(), sec.parse::<u32>()) else { return false };
    // XSD 1.0 errata allow 24:00:00, but validators such as Jing reject it.
    if !(h < 24 && mi < 60 && sec < 60) {
        return false;
    }
    match zone {
        None | Some("Z") => true,
        Some(z) => {
            let Some((zh, zm)) = z.get(1..).and_then(|z| z.split_once(':')) else { return false };
            let (Ok(zh), Ok(zm)) = (zh.parse::<u32>(), zm.parse::<u32>()) else { return false };
            zone_form(z) && zm < 60 && (zh < 14 || (zh == 14 && zm == 0))
        }
    }
}

/// `(+|-)hh:mm`
fn zone_form(zone: &str) -> bool {
    let b = zone.as_bytes();
    b.len() == 6
        && b.iter().enumerate().all(|(i, c)| match i {
            0 => matches!(c, b'+' | b'-'),
            3 => *c == b':',
            _ => c.is_ascii_digit(),
        })
}

/// A whitespace-separated list.
impl<T: ListItem> Value for Vec<T> {
    fn parse(text: &str) -> Result<Self, ValueError> {
        text.split(is_xml_space).filter(|s| !s.is_empty()).map(T::parse).collect()
    }

    fn format(&self, out: &mut String) {
        for (i, item) in self.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            item.format(out);
        }
    }
}

/// A whitespace-separated list of exactly `N` items.
impl<T: ListItem, const N: usize> Value for [T; N] {
    fn parse(text: &str) -> Result<Self, ValueError> {
        let items: Vec<T> = Value::parse(text)?;
        let found = items.len();
        items.try_into().map_err(|_| ValueError::new(format!("expected a list of {N} items, found {found}")))
    }

    fn format(&self, out: &mut String) {
        for (i, item) in self.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            item.format(out);
        }
    }
}

/// Declares an enumeration (`*_EnumValue`) with its IDML literals.
macro_rules! value_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident = $lit:literal,)* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $(#[doc = concat!("`", $lit, "`.")] $(#[$vmeta])* $variant,)*
        }

        impl $name {
            /// Every value, in schema order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)*];

            /// The IDML literal.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $lit,)*
                }
            }
        }

        impl $crate::value::Value for $name {
            fn parse(text: &str) -> Result<Self, $crate::value::ValueError> {
                match text {
                    $($lit => Ok(Self::$variant),)*
                    _ => Err($crate::value::ValueError::new(format!(
                        "{text:?} is not a {} value", stringify!($name)
                    ))),
                }
            }

            fn format(&self, out: &mut String) {
                out.push_str(self.as_str());
            }
        }

        impl $crate::value::ListItem for $name {}

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = $crate::value::ValueError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                $crate::value::Value::parse(text)
            }
        }
    };
}

/// Declares a number type restricted to an inclusive range
/// (`minInclusive`/`maxInclusive` facets).
macro_rules! ranged {
    ($(#[$meta:meta])* $name:ident: $t:ty = $min:literal ..= $max:literal $(, $derive:ident)*) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, PartialOrd $(, $derive)*)]
        pub struct $name($t);

        impl $name {
            pub const MIN: $t = $min;
            pub const MAX: $t = $max;
            /// For values that are overwritten while reading.
            #[allow(dead_code)]
            pub(crate) const PLACEHOLDER: Self = Self($min);

            /// The value, if it is within `MIN..=MAX`.
            pub fn new(value: $t) -> Result<Self, $crate::value::ValueError> {
                if (Self::MIN..=Self::MAX).contains(&value) {
                    Ok(Self(value))
                } else {
                    Err($crate::value::ValueError::new(format!(
                        "{value} is outside {}..={}", Self::MIN, Self::MAX
                    )))
                }
            }

            pub const fn get(self) -> $t {
                self.0
            }
        }

        impl TryFrom<$t> for $name {
            type Error = $crate::value::ValueError;

            fn try_from(value: $t) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for $t {
            fn from(value: $name) -> $t {
                value.0
            }
        }

        impl $crate::value::Value for $name {
            fn parse(text: &str) -> Result<Self, $crate::value::ValueError> {
                Self::new($crate::value::Value::parse(text)?)
            }

            fn format(&self, out: &mut String) {
                $crate::value::Value::format(&self.0, out)
            }
        }

        impl $crate::value::ListItem for $name {}
    };
}

/// Declares a value that is one of several enumerations.
macro_rules! value_union {
    ($(#[$meta:meta])* $name:ident { $($variant:ident,)* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant($variant),)*
        }

        impl $crate::value::Value for $name {
            fn parse(text: &str) -> Result<Self, $crate::value::ValueError> {
                $(if let Ok(v) = $crate::value::Value::parse(text) {
                    return Ok(Self::$variant(v));
                })*
                Err($crate::value::ValueError::new(format!(
                    "{text:?} is not a {} value", stringify!($name)
                )))
            }

            fn format(&self, out: &mut String) {
                match self {
                    $(Self::$variant(v) => $crate::value::Value::format(v, out),)*
                }
            }
        }

        impl $crate::value::ListItem for $name {}

        $(impl From<$variant> for $name {
            fn from(v: $variant) -> Self {
                Self::$variant(v)
            }
        })*
    };
}

pub(crate) use {ranged, value_enum, value_union};

#[cfg(test)]
mod tests {
    use super::*;

    fn round<T: Value>(text: &str) -> String {
        let mut out = String::new();
        T::parse(text).unwrap().format(&mut out);
        out
    }

    #[test]
    fn doubles_follow_xsd_lexical_rules() {
        for (text, canonical) in [
            ("1", "1"),
            ("1.0", "1"),
            (" -2.50 ", "-2.5"),
            ("+.5", "0.5"),
            ("5.", "5"),
            ("1E3", "1000"),
            ("1e-7", "0.0000001"),
            ("-0", "-0"),
            ("INF", "INF"),
            ("-INF", "-INF"),
            ("NaN", "NaN"),
            ("1.7976931348623157e308", &f64::MAX.to_string()),
        ] {
            assert_eq!(round::<f64>(text), canonical, "{text}");
        }
        for bad in ["", ".", "e5", "1e", "inf", "Infinity", "+INF", "nan", "0x10", "1,5", "1 2"] {
            assert!(f64::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn integers_and_booleans() {
        assert_eq!(round::<i32>(" +007 "), "7");
        assert_eq!(round::<i32>("-2147483648"), "-2147483648");
        assert!(i32::parse("2147483648").is_err());
        assert!(i16::parse("32768").is_err());
        assert!(i32::parse("1.0").is_err());
        assert_eq!(round::<bool>("1"), "true");
        assert_eq!(round::<bool>(" false "), "false");
        assert!(bool::parse("True").is_err());
    }

    #[test]
    fn lists() {
        assert_eq!(round::<Vec<f64>>("  1 2.0\t3\n"), "1 2 3");
        assert_eq!(round::<Vec<Token>>(""), "");
        assert_eq!(round::<[f64; 2]>("1 2"), "1 2");
        assert!(<[f64; 2]>::parse("1 2 3").is_err());
        assert!(Token::new("a b").is_err());
        assert!(Token::new("").is_err());
    }

    #[test]
    fn part_paths() {
        assert!(PartPath::new("Stories/Story_u1.xml").is_ok());
        for bad in ["Story.txt", "/Stories/a.xml", "Stories//a.xml", "../a.xml", "a\\b.xml", "a\n.xml"] {
            assert!(PartPath::new(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn date_times() {
        for ok in ["2026-10-23T09:30:00", "2026-10-23T09:30:00Z", "2024-02-29T23:59:59.125+14:00", "-0001-01-01T00:00:00"] {
            assert!(DateTime::new(ok).is_ok(), "{ok}");
        }
        for bad in
            ["2026-10-23", "2025-02-29T00:00:00", "2026-13-01T00:00:00", "2026-10-23T24:00:00", "2026-10-23T09:30:00+15:00", "26-10-23T09:30:00"]
        {
            assert!(DateTime::new(bad).is_err(), "{bad}");
        }
    }
}
