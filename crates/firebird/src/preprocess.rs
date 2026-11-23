//! `pdo_firebird`'s SQL preprocessing (`php_firebird_preprocess` and its
//! tokenizer): Firebird has no named parameters, so in the statements that take
//! parameters each `:name` becomes `?`, its position recorded under its name.
//!
//! As the C did, the SQL is read as `ZSTR_LEN` bytes followed by the string's
//! terminating NUL, and what is copied out stops at a NUL, as `strcpy`,
//! `strncat` and `strncpy` did: the result is the C string Firebird prepares.

/// The character classes of `classes_array`, for bytes up to 127; any other
/// byte is in none.
const LETTER: u8 = 1;
const DIGIT: u8 = 2;
const IDENT: u8 = 4;
const WHITE: u8 = 16;

/// `php_firebird_classes`: `classes_array`'s entry for `byte`.
const fn class(byte: u8) -> u8 {
    const HEX: u8 = 32;
    const QUOTE: u8 = 8;
    const INTRODUCER: u8 = 64;
    match byte {
        b'\t' | b'\n' | b'\r' | b' ' => WHITE,
        b'"' | b'\'' => QUOTE,
        b'$' => IDENT,
        b'0'..=b'9' => DIGIT | IDENT | HEX,
        b'A'..=b'F' | b'a'..=b'f' => LETTER | IDENT | HEX,
        b'G'..=b'Z' | b'g'..=b'z' | b'{' | b'}' => LETTER | IDENT,
        b'_' => IDENT | INTRODUCER,
        _ => 0,
    }
}

/// `FbTokenType`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Token {
    None,
    White,
    Comment,
    BrokenComment,
    String,
    ParamMark,
    Ident,
    Other,
}

/// The SQL as C read it: its bytes, then the terminating NUL at `end`.
struct Sql<'a> {
    bytes: &'a [u8],
}

impl Sql<'_> {
    fn end(&self) -> usize {
        self.bytes.len()
    }

    /// The byte at `at`: the string's NUL at or past its end.
    fn at(&self, at: usize) -> u8 {
        self.bytes.get(at).copied().unwrap_or(0)
    }

    /// The bytes from `from` to `to`, as far as the first NUL among them, as
    /// `strncat` and `strncpy` copy them.
    fn c_segment(&self, from: usize, to: usize) -> &[u8] {
        let segment = self.bytes.get(from..to.min(self.end())).unwrap_or_default();
        let nul = segment.iter().position(|&byte| byte == 0);
        segment
            .get(..nul.unwrap_or(segment.len()))
            .unwrap_or_default()
    }

    /// `php_firebird_get_token`: the token starting at `at`, and where the next
    /// one starts.
    fn token(&self, mut at: usize) -> (Token, usize) {
        let end = self.end();
        let first = self.at(at);
        at += 1;
        let token = match first {
            b':' | b'?' => Token::ParamMark,
            b'\'' | b'"' => {
                let mut token = Token::None;
                while at < end {
                    let byte = self.at(at);
                    at += 1;
                    if byte == first {
                        token = Token::String;
                        break;
                    }
                }
                token
            }
            b'/' if at < end && self.at(at) == b'*' => {
                let mut token = Token::BrokenComment;
                at += 1;
                while at < end {
                    let byte = self.at(at);
                    at += 1;
                    if byte == b'*' && at < end && self.at(at) == b'/' {
                        at += 1;
                        token = Token::Comment;
                        break;
                    }
                }
                token
            }
            b'-' if at < end && self.at(at) == b'-' => {
                // The line comment runs to a CR (and the LF after it), or up
                // to an LF.
                loop {
                    at += 1;
                    if at >= end {
                        break;
                    }
                    match self.at(at) {
                        b'\r' => {
                            at += 1;
                            if at < end && self.at(at) == b'\n' {
                                at += 1;
                            }
                            break;
                        }
                        b'\n' => break,
                        _ => {}
                    }
                }
                Token::Comment
            }
            b'/' | b'-' => Token::Other,
            _ => {
                let run = |at: &mut usize, keep: &dyn Fn(u8) -> bool| {
                    while *at < end && keep(self.at(*at)) {
                        *at += 1;
                    }
                };
                let first_class = class(first);
                if first_class & DIGIT != 0 {
                    run(&mut at, &|byte| class(byte) & DIGIT != 0);
                    Token::Other
                } else if first_class & IDENT != 0 {
                    run(&mut at, &|byte| class(byte) & IDENT != 0);
                    Token::Ident
                } else if first_class & WHITE != 0 {
                    run(&mut at, &|byte| class(byte) & WHITE != 0);
                    Token::White
                } else {
                    run(&mut at, &|byte| {
                        class(byte) & (DIGIT | IDENT | WHITE) == 0
                            && !matches!(byte, b'/' | b'-' | b':' | b'?' | b'\'' | b'"')
                    });
                    Token::Other
                }
            }
        };
        (token, at)
    }

    /// The identifier from `from` to `to` as `strcasecmp` read C's copy of it:
    /// as far as a NUL, compared without case. None when it is longer than C's
    /// buffer allowed (`limit` bytes).
    fn ident(&self, from: usize, to: usize, limit: usize) -> Option<&[u8]> {
        (to - from <= limit).then(|| self.c_segment(from, to))
    }
}

/// The longest identifier C's buffers took: Firebird 4.0's 63 characters fit.
const IDENT_MAX: usize = 252;

/// `php_firebird_preprocess`: `sql` with each `:name` replaced by `?`, as the C
/// string Firebird prepares (without its NUL), each name (with its colon, as
/// `strncpy` padded it) and parameter index given to `name` as it is found; or
/// None where C failed ("Statement expected", "Parameter name expected", an
/// unclosed comment or string, an identifier too long), after the names found
/// before.
pub fn preprocess(sql: &[u8], mut name: impl FnMut(&[u8], i64)) -> Option<Vec<u8>> {
    let sql = Sql { bytes: sql };
    let end = sql.end();
    let skip_blank = |mut start: usize| {
        let (mut token, mut at) = sql.token(start);
        while at < end && matches!(token, Token::Comment | Token::White) {
            start = at;
            (token, at) = sql.token(at);
        }
        (token, start, at)
    };
    let (token, start, mut at) = skip_blank(0);
    if at >= end || token != Token::Ident {
        return None;
    }
    let keyword = sql.ident(start, at, IDENT_MAX)?;
    let mut exec_block = false;
    let pass_as_is = if keyword.eq_ignore_ascii_case(b"EXECUTE") {
        // EXECUTE PROCEDURE and EXECUTE BLOCK name their parameters, the
        // block up to its AS.
        let (token, second, next) = skip_blank(at);
        at = next;
        if at >= end || token != Token::Ident {
            return None;
        }
        exec_block = sql
            .ident(second, at, IDENT_MAX)?
            .eq_ignore_ascii_case(b"BLOCK");
        false
    } else {
        ![
            &b"INSERT"[..],
            b"UPDATE",
            b"DELETE",
            b"MERGE",
            b"SELECT",
            b"WITH",
        ]
        .iter()
        .any(|statement| keyword.eq_ignore_ascii_case(statement))
    };
    if pass_as_is {
        return Some(sql.c_segment(0, end).to_vec());
    }
    let mut out = sql.c_segment(start, at).to_vec();
    let mut index: i64 = -1;
    while at < end {
        let start = at;
        let token;
        (token, at) = sql.token(at);
        match token {
            Token::ParamMark => {
                let named;
                (named, at) = sql.token(at);
                if named == Token::Ident {
                    index += 1;
                    // The colon and the name, which C's 254-byte buffer took.
                    let length = at - start;
                    if length > IDENT_MAX + 1 {
                        return None;
                    }
                    let mut key = sql.c_segment(start, at).to_vec();
                    key.resize(length, 0);
                    name(&key, index);
                    out.push(b'?');
                } else {
                    if sql.at(start) != b'?' {
                        return None;
                    }
                    index += 1;
                    out.extend_from_slice(sql.c_segment(start, at));
                }
            }
            Token::Ident if exec_block => {
                if sql.ident(start, at, IDENT_MAX)?.eq_ignore_ascii_case(b"AS") {
                    out.extend_from_slice(sql.c_segment(start, end));
                    return Some(out);
                }
                out.extend_from_slice(sql.c_segment(start, at));
            }
            Token::Ident | Token::White | Token::Comment | Token::String | Token::Other => {
                out.extend_from_slice(sql.c_segment(start, at));
            }
            Token::BrokenComment | Token::None => return None,
        }
    }
    Some(out)
}

#[cfg(feature = "php")]
pub(crate) use export::named;

#[cfg(feature = "php")]
mod export {
    use std::ffi::c_long;
    use swoole_rs_corebase::lent::{Count, In, lend_in};
    use swoole_rs_zend::{Array, ZStr};

    unsafe extern "C" {
        /// `firebird_driver.c`: `named_params[name] = index`
        /// (`zend_hash_str_update`), over the lent name's bytes.
        safe fn swr_firebird_name_param<'a>(
            named_params: &Array,
            name: In<'a, u8>,
            len: Count<'a>,
            index: c_long,
        );
    }

    /// `php_firebird_preprocess`: `sql` preprocessed into the C string Firebird
    /// prepares (without its NUL), each named parameter's index recorded in
    /// `named_params` when given. None where C returned 0.
    pub(crate) fn named(sql: &ZStr, named_params: Option<&Array>) -> Option<Vec<u8>> {
        let named = |key: &[u8], index: i64| {
            if let Some(table) = named_params {
                lend_in(key, |name, len| {
                    swr_firebird_name_param(table, name, len, index);
                });
            }
        };
        super::preprocess(sql.bytes(), named)
    }
}
