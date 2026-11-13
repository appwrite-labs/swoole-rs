//! The JSON the admin server's commands read and write, as upstream's `nlohmann::json`
//! does: objects dump their keys in byte order, compactly, with nlohmann's escapes
//! and number formats. A request that is not JSON, or whose parameter is not a string,
//! made upstream throw out of the handler and abort; it aborts here too.
use std::collections::BTreeMap;
use std::fmt::Write;

pub(crate) enum Json {
    Int(i64),
    Uint(u64),
    Float(f64),
    Str(String),
    Array(Vec<Json>),
    Object(BTreeMap<&'static str, Json>),
}

impl Json {
    pub(crate) fn object<const N: usize>(fields: [(&'static str, Json); N]) -> Self {
        Self::Object(fields.into_iter().collect())
    }

    #[cfg(not(test))]
    /// Text from native code; nlohmann refused to dump invalid UTF-8.
    pub(crate) fn utf8(bytes: &[u8]) -> String {
        match core::str::from_utf8(bytes) {
            Ok(text) => text.to_owned(),
            Err(_) => swoole_rs_corebase::fatal("admin JSON that is not UTF-8"),
        }
    }

    pub(crate) fn dump(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Self::Int(value) => _ = write!(out, "{value}"),
            Self::Uint(value) => _ = write!(out, "{value}"),
            Self::Float(value) => float(out, *value),
            Self::Str(text) => string(out, text),
            Self::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Self::Object(fields) => {
                out.push('{');
                for (index, (key, value)) in fields.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    string(out, key);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => _ = write!(out, "\\u{:04x}", u32::from(c)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// nlohmann's `dump_float`: the shortest digits, in positional notation for
/// decimal exponents in [-4, 15] and as `d.ddde+XX` outside them.
fn float(out: &mut String, value: f64) {
    if !value.is_finite() {
        out.push_str("null");
        return;
    }
    if value == 0.0 {
        out.push_str(if value.is_sign_negative() {
            "-0.0"
        } else {
            "0.0"
        });
        return;
    }
    // `{:e}` is the shortest round-trip form, `[-]d[.ddd]e[-]x`.
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let (sign, mantissa) = mantissa
        .strip_prefix('-')
        .map_or(("", mantissa), |rest| ("-", rest));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let k = i64::try_from(digits.len()).unwrap_or(i64::MAX);
    // The position of the decimal point after the digits' first: value = 0.digits * 10^n.
    let n = exponent.parse::<i64>().unwrap_or(0) + 1;
    out.push_str(sign);
    if k <= n && n <= 15 {
        out.push_str(&digits);
        out.extend(core::iter::repeat_n(
            '0',
            usize::try_from(n - k).unwrap_or(0),
        ));
        out.push_str(".0");
    } else if 0 < n && n <= 15 {
        let (whole, fraction) = digits.split_at(usize::try_from(n).unwrap_or(0));
        _ = write!(out, "{whole}.{fraction}");
    } else if -4 < n && n <= 0 {
        out.push_str("0.");
        out.extend(core::iter::repeat_n('0', usize::try_from(-n).unwrap_or(0)));
        out.push_str(&digits);
    } else {
        let (first, rest) = digits.split_at(1);
        out.push_str(first);
        if !rest.is_empty() {
            _ = write!(out, ".{rest}");
        }
        let e = n - 1;
        _ = write!(
            out,
            "e{}{:02}",
            if e < 0 { '-' } else { '+' },
            e.unsigned_abs()
        );
    }
}

/// The value of top-level key `key` in request `msg`: `None` when `msg` is not an
/// object or has no such key. Aborts, as upstream's uncaught exceptions did, when
/// `msg` is not JSON or the value is not a string.
pub(crate) fn param(msg: &[u8], key: &str) -> Option<String> {
    let mut parser = Parser { text: msg, at: 0 };
    match parser.document(key.as_bytes()) {
        Ok(Found::Text(value)) => Some(value),
        Ok(Found::Other) | Err(()) => {
            swoole_rs_corebase::fatal("admin JSON without the expected text field")
        }
        Ok(Found::Missing) => None,
    }
}

/// A top-level key's value.
#[derive(Debug, PartialEq)]
enum Found {
    /// Not an object, or no such key.
    Missing,
    Text(String),
    /// A value that is not a string.
    Other,
}

struct Parser<'a> {
    text: &'a [u8],
    at: usize,
}

type Parsed<T> = Result<T, ()>;

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        self.space();
        let found = self.peek() == Some(byte);
        if found {
            self.at += 1;
        }
        found
    }

    /// The whole document, and what it holds under `key`.
    fn document(&mut self, key: &[u8]) -> Parsed<Found> {
        self.space();
        let found = if self.peek() == Some(b'{') {
            self.object(key)?
        } else {
            self.value()?;
            Found::Missing
        };
        self.space();
        if self.at == self.text.len() {
            Ok(found)
        } else {
            Err(())
        }
    }

    /// A top-level object: the last value under `key`.
    fn object(&mut self, key: &[u8]) -> Parsed<Found> {
        self.at += 1;
        let mut found = Found::Missing;
        if self.eat(b'}') {
            return Ok(found);
        }
        loop {
            self.space();
            let name = self.string()?;
            if !self.eat(b':') {
                return Err(());
            }
            self.space();
            if name.as_bytes() == key {
                found = if self.peek() == Some(b'"') {
                    Found::Text(self.string()?)
                } else {
                    self.value()?;
                    Found::Other
                };
            } else {
                self.value()?;
            }
            if self.eat(b'}') {
                return Ok(found);
            }
            if !self.eat(b',') {
                return Err(());
            }
        }
    }

    /// Any value, nested without recursion.
    fn value(&mut self) -> Parsed<()> {
        // Each open container: whether it is an object.
        let mut open = swoole_rs_memory::flat::BitSet::new();
        loop {
            self.space();
            match self.peek().ok_or(())? {
                b'{' | b'[' => {
                    let is_object = self.peek() == Some(b'{');
                    self.at += 1;
                    if self.eat(if is_object { b'}' } else { b']' }) {
                        // Empty: fall through to the separator below.
                    } else {
                        open.push(is_object);
                        if is_object {
                            self.key()?;
                        }
                        continue;
                    }
                }
                b'"' => _ = self.string()?,
                b't' => self.literal(b"true")?,
                b'f' => self.literal(b"false")?,
                b'n' => self.literal(b"null")?,
                _ => self.number()?,
            }
            // A value ended: close containers or move to the next element.
            loop {
                let Some(is_object) = open.last() else {
                    return Ok(());
                };
                if self.eat(if is_object { b'}' } else { b']' }) {
                    open.pop();
                    continue;
                }
                if !self.eat(b',') {
                    return Err(());
                }
                if is_object {
                    self.key()?;
                }
                break;
            }
        }
    }

    fn key(&mut self) -> Parsed<()> {
        self.space();
        self.string()?;
        if self.eat(b':') { Ok(()) } else { Err(()) }
    }

    fn literal(&mut self, word: &[u8]) -> Parsed<()> {
        if self.text.get(self.at..self.at + word.len()) == Some(word) {
            self.at += word.len();
            Ok(())
        } else {
            Err(())
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.at += 1;
        }
        self.at - start
    }

    fn number(&mut self) -> Parsed<()> {
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        match self.peek() {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => _ = self.digits(),
            _ => return Err(()),
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            if self.digits() == 0 {
                return Err(());
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if self.digits() == 0 {
                return Err(());
            }
        }
        Ok(())
    }

    fn hex4(&mut self) -> Parsed<u32> {
        let digits = self.text.get(self.at..self.at + 4).ok_or(())?;
        let text = core::str::from_utf8(digits).map_err(|_| ())?;
        if !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(());
        }
        self.at += 4;
        u32::from_str_radix(text, 16).map_err(|_| ())
    }

    /// A string at the cursor, unescaped; UTF-8 and escapes as nlohmann's lexer checks them.
    fn string(&mut self) -> Parsed<String> {
        if self.peek() != Some(b'"') {
            return Err(());
        }
        self.at += 1;
        let mut out = Vec::new();
        loop {
            let byte = self.peek().ok_or(())?;
            self.at += 1;
            match byte {
                b'"' => return String::from_utf8(out).map_err(|_| ()),
                0..0x20 => return Err(()),
                b'\\' => {
                    let escape = self.peek().ok_or(())?;
                    self.at += 1;
                    let c = match escape {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => self.unicode()?,
                        _ => return Err(()),
                    };
                    let mut buffer = [0; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
                }
                byte => out.push(byte),
            }
        }
    }

    fn unicode(&mut self) -> Parsed<char> {
        let high = self.hex4()?;
        let code = if (0xD800..0xDC00).contains(&high) {
            if self.text.get(self.at..self.at + 2) != Some(b"\\u") {
                return Err(());
            }
            self.at += 2;
            let low = self.hex4()?;
            if !(0xDC00..0xE000).contains(&low) {
                return Err(());
            }
            0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
        } else if (0xDC00..0xE000).contains(&high) {
            return Err(());
        } else {
            high
        };
        char::from_u32(code).ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Found, Json, Parser};
    use std::collections::BTreeMap;

    fn found(msg: &str, key: &str) -> Result<Found, ()> {
        Parser {
            text: msg.as_bytes(),
            at: 0,
        }
        .document(key.as_bytes())
    }

    #[test]
    fn objects_dump_sorted_and_compact() {
        let value = Json::object([
            ("data", Json::Str("require parameter type".into())),
            ("code", Json::Int(4003)),
        ]);
        assert_eq!(
            value.dump(),
            r#"{"code":4003,"data":"require parameter type"}"#,
            "nlohmann sorts keys"
        );
        let list = Json::Array(vec![
            Json::Uint(1),
            Json::Object(BTreeMap::default()),
            Json::Array(vec![]),
        ]);
        assert_eq!(list.dump(), "[1,{},[]]", "empty containers");
    }

    #[test]
    fn strings_escape_as_nlohmann() {
        let value = Json::Str("a\"b\\c\u{8}\u{c}\n\r\t\u{1}\u{7f}/é".into());
        assert_eq!(
            value.dump(),
            "\"a\\\"b\\\\c\\b\\f\\n\\r\\t\\u0001\u{7f}/é\"",
            "escapes"
        );
    }

    #[test]
    fn floats_format_as_nlohmann() {
        let cases = [
            (0.0, "0.0"),
            (1.0, "1.0"),
            (1_730_000_000.123_456, "1730000000.123456"),
            (0.5, "0.5"),
            (0.000_12, "0.00012"),
            (0.000_012, "1.2e-05"),
            (1e14, "100000000000000.0"),
            (1e15, "1e+15"),
            (1.5e300, "1.5e+300"),
            (-2.25, "-2.25"),
            (f64::NAN, "null"),
        ];
        for (value, text) in cases {
            assert_eq!(Json::Float(value).dump(), text, "{value}");
        }
    }

    #[test]
    fn params_read_top_level_strings() {
        assert_eq!(
            found(r#"{"fd":"12"}"#, "fd"),
            Ok(Found::Text("12".into())),
            "string"
        );
        assert_eq!(
            found(r#" {"a":[1,{"fd":2}],"fd":"xA"} "#, "fd"),
            Ok(Found::Text("xA".into())),
            "nested ignored"
        );
        assert_eq!(found(r#"{"fd":1}"#, "fd"), Ok(Found::Other), "not a string");
        assert_eq!(
            found(r#"{"fd":"1","fd":"2"}"#, "fd"),
            Ok(Found::Text("2".into())),
            "last wins"
        );
        assert_eq!(
            found(r#"{"type":"event"}"#, "fd"),
            Ok(Found::Missing),
            "missing"
        );
        assert_eq!(found("[1,2]", "fd"), Ok(Found::Missing), "array");
        assert_eq!(found("\"fd\"", "fd"), Ok(Found::Missing), "string document");
        assert_eq!(found("{}", "fd"), Ok(Found::Missing), "empty object");
    }

    #[test]
    fn invalid_documents_fail() {
        for msg in [
            "",
            "{",
            r#"{"fd":}"#,
            "[1,]",
            "01",
            "1.",
            "tru",
            "{\"a\":1}x",
            "\"\u{1}\"",
            r#""\ud800""#,
            "[[[]]",
        ] {
            assert_eq!(found(msg, "fd"), Err(()), "{msg:?}");
        }
        assert_eq!(
            found("[[[{\"a\":[true,false,null,-1.5e3]}]]]", "fd"),
            Ok(Found::Missing),
            "deep values parse"
        );
    }
}
