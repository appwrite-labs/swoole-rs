use super::{Token, token};

#[test]
fn sqlite_placeholder_boundaries() {
    for (sql, kind, length) in [
        (b":x9_!".as_slice(), Token::Bind, 4),
        (b"?", Token::Positional, 1),
        (b"???x", Token::Text, 3),
        (b"::x", Token::Text, 2),
        (b"'a':x'b':y", Token::Text, 8),
        (b"[x]:a[y]:b", Token::Text, 8),
        (b"`a\0b`", Token::Text, 1),
        (b"--a\nb", Token::Text, 3),
        (b"/*a*/:x", Token::Text, 5),
        (b"/*a", Token::End, 4),
        (b"", Token::End, 0),
    ] {
        let (actual, consumed) = token(sql);
        assert_eq!((actual as i32, consumed), (kind as i32, length), "{sql:?}");
    }
}
