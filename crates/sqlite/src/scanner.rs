//! PHP 8.4+ PDO SQLite placeholder tokens, matching the bundled re2c scanner.
use std::io::IoSlice;

#[repr(i32)]
enum Token {
    Text = 1,
    Bind = 2,
    Positional = 3,
    End = 6,
}

fn token(sql: &[u8]) -> (Token, usize) {
    use Token::{Bind, End, Positional, Text};
    let Some((&first, tail)) = sql.split_first() else {
        return (End, 0);
    };
    match first {
        b':' if tail.first() != Some(&b':') => {
            let name = tail
                .iter()
                .take_while(|&&b| b == b'_' || b.is_ascii_alphanumeric())
                .count();
            (if name == 0 { Text } else { Bind }, name + 1)
        }
        b'?' | b':' => {
            let len = 1 + tail.iter().take_while(|&&b| b == first).count();
            (
                if first == b'?' && len == 1 {
                    Positional
                } else {
                    Text
                },
                len,
            )
        }
        b'\'' | b'"' | b'`' | b'[' => {
            // The generated scanner remembers the last closing delimiter before NUL.
            let close = if first == b'[' { b']' } else { first };
            let quoted = tail.split(|&b| b == 0).next().unwrap_or_default();
            (
                Text,
                quoted
                    .iter()
                    .rposition(|&b| b == close)
                    .map_or(1, |i| i + 2),
            )
        }
        b'-' if tail.starts_with(b"-") => sql
            .iter()
            .position(|&b| b == b'\n')
            .map_or((End, sql.len() + 1), |i| (Text, i)),
        b'/' if tail.starts_with(b"*") => tail
            .get(1..)
            .unwrap_or_default()
            .windows(2)
            .position(|w| w == b"*/")
            .map_or((End, sql.len() + 1), |i| (Text, i + 4)),
        0 | b'-' | b'/' => (Text, 1),
        _ => (
            Text,
            1 + tail
                .iter()
                .take_while(|&&b| !b"\0\"'-/:[?`".contains(&b))
                .count(),
        ),
    }
}

#[unsafe(no_mangle)]
extern "C" fn pdo_sqlite_scan_token(sql: IoSlice<'_>, consumed: &mut usize) -> Token {
    let (kind, length) = token(&sql);
    *consumed = length;
    kind
}

#[cfg(test)]
#[path = "../../../tests/modules/sqlite/scanner.rs"]
mod tests;
