//! PostgreSQL placeholder tokens used by PDO's PHP 8.4+ SQL rewriter.
use std::io::IoSlice;

#[repr(i32)]
#[derive(Clone, Copy)]
enum Token {
    Text = 1,
    Bind,
    Positional,
    EscapedQuestion,
    CustomQuote,
    End,
}

/// A doubled quote can also end the token at its first quote if the longer
/// quoted string never closes. This is the scanner's longest-match fallback.
fn quoted(sql: &[u8], start: usize, quote: u8, escapes: bool) -> usize {
    let mut pos = start + 1;
    let mut accepted = 1;
    while let Some(&byte) = sql.get(pos) {
        pos += 1;
        if byte == 0 {
            break;
        }
        if byte == quote {
            accepted = pos;
            if sql.get(pos) != Some(&quote) {
                break;
            }
            pos += 1;
        } else if escapes && byte == b'\\' {
            if sql.get(pos).is_none_or(|&b| b == 0) {
                break;
            }
            pos += 1;
        }
    }
    accepted
}

fn name(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

fn token(sql: &[u8]) -> (Token, usize) {
    use Token::{Bind, CustomQuote, End, EscapedQuestion, Positional, Text};
    let Some((&first, tail)) = sql.split_first() else {
        return (End, 0);
    };
    match first {
        b'?' if tail.starts_with(b"?") => (EscapedQuestion, 2),
        b'?' => (Positional, 1),
        b':' if tail.starts_with(b":") => {
            (Text, 1 + tail.iter().take_while(|&&b| b == b':').count())
        }
        b':' => {
            let len = tail.iter().take_while(|&&b| name(b)).count();
            (if len == 0 { Text } else { Bind }, 1 + len)
        }
        b'$' => {
            if !tail.first().is_some_and(u8::is_ascii_digit) {
                let len = tail.iter().take_while(|&&b| name(b) || b >= 128).count();
                if tail.get(len) == Some(&b'$') {
                    return (CustomQuote, len + 2);
                }
            }
            (Text, 1)
        }
        b'e' | b'E' if tail.starts_with(b"'") => (Text, quoted(sql, 1, b'\'', true)),
        b'"' | b'\'' => (Text, quoted(sql, 0, first, false)),
        b'-' if tail.starts_with(b"-") => sql
            .iter()
            .position(|&b| b == b'\n')
            .map_or((End, sql.len() + 1), |len| (Text, len)),
        b'/' if tail.starts_with(b"*") => tail
            .get(1..)
            .unwrap_or_default()
            .windows(2)
            .position(|w| w == b"*/")
            .map_or((End, sql.len() + 1), |len| (Text, len + 4)),
        b'e' | b'E' | b'/' | b'-' => (Text, 1),
        _ => (
            Text,
            1 + tail
                .iter()
                .take_while(|&&b| !b"\0$eE:?\"'/-".contains(&b))
                .count(),
        ),
    }
}

/// PDO supplies the bytes remaining before its terminal NUL. Unterminated
/// comments consume the NUL and return end-of-input, as the bundled scanner does.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_scan_token(sql: IoSlice<'_>, consumed: &mut usize) -> Token {
    let (kind, length) = token(&sql);
    *consumed = length;
    kind
}
