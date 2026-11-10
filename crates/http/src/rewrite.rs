//! `url_rewrite_rules`: upstream's `Server::add_rewrite_rule` and `apply_rewrite_rules`.
//!
//! A rule is a byte prefix or, between `~` delimiters, a regex. The first rule that changes
//! the URL wins. A regex replaces every match the way libstdc++'s `std::regex_replace` does:
//! the same steps past empty matches and the same `$` format. Patterns are compiled once,
//! on `add`, where upstream compiled one per request; one that doesn't compile never matches,
//! as upstream skipped a rule whose `std::regex` threw.
mod regex;

use regex::{Captures, Regex};

enum Pattern {
    Prefix(Vec<u8>),
    Regex(Option<Regex>),
}

struct Rule {
    pattern: Pattern,
    replacement: Vec<u8>,
}

/// A server's rules, in the order they were added.
#[derive(Default)]
pub struct RewriteRules(Vec<Rule>);

impl RewriteRules {
    pub fn add(&mut self, pattern: &[u8], replacement: &[u8]) {
        let pattern = match pattern {
            [b'~', inner @ .., b'~'] => Pattern::Regex(Regex::new(inner)),
            _ => Pattern::Prefix(pattern.to_vec()),
        };
        self.0.push(Rule {
            pattern,
            replacement: replacement.to_vec(),
        });
    }

    /// The rewritten URL, or `None` when no rule applies. A prefix rule applies whenever it
    /// matches; a regex rule only when its replacement differs from `url`.
    #[must_use]
    pub fn apply(&self, url: &[u8]) -> Option<Vec<u8>> {
        self.0.iter().find_map(|rule| match &rule.pattern {
            Pattern::Prefix(prefix) => url
                .strip_prefix(prefix.as_slice())
                .map(|suffix| [rule.replacement.as_slice(), suffix].concat()),
            Pattern::Regex(regex) => regex
                .as_ref()
                .and_then(|regex| replace_all(regex, url, &rule.replacement))
                .filter(|rewritten| rewritten != url),
        })
    }
}

/// `std::regex_replace(url, regex, format)`, or `None` when nothing matches. After an empty
/// match, libstdc++'s iterator first tries a non-empty match at the same place, then searches
/// from the next byte; after any other match it searches from its end, empty matches included.
/// Its first step doesn't pass `match_prev_avail` yet, so the non-empty try after a first,
/// empty match sees the input begin there.
fn replace_all(regex: &Regex, url: &[u8], format: &[u8]) -> Option<Vec<u8>> {
    let mut caps = regex.search(url, 0, 0, false)?;
    let mut out = Vec::new();
    let mut previous = 0;
    let mut first = true;
    loop {
        let first_step = std::mem::replace(&mut first, false);
        let std::ops::Range { start, end } = caps.group(0)?;
        out.extend_from_slice(url.get(previous..start)?);
        expand(&mut out, url, &caps, previous, format);
        previous = end;
        let mut from = end;
        if start == end {
            if from == url.len() {
                break;
            }
            let begin = if first_step { from } else { 0 };
            if let Some(next) = regex.search(url, begin, from, true) {
                caps = next;
                continue;
            }
            from += 1;
        }
        match regex.search(url, 0, from, false) {
            Some(next) => caps = next,
            None => break,
        }
    }
    out.extend_from_slice(url.get(previous..)?);
    Some(out)
}

/// libstdc++'s ECMAScript `match_results::format`: `$$`, `$&`, `` $` `` (from the previous
/// match's end), `$'`, and `$n`/`$nn` (nothing for a group that doesn't exist or didn't match).
/// Any other `$` is literal.
fn expand(out: &mut Vec<u8>, url: &[u8], caps: &Captures, previous: usize, format: &[u8]) {
    let group = |index: usize| caps.group(index).and_then(|span| url.get(span));
    let whole = caps.group(0).unwrap_or(0..0);
    let mut rest = format;
    while let Some(dollar) = rest.iter().position(|&byte| byte == b'$') {
        let (literal, tail) = rest.split_at(dollar);
        out.extend_from_slice(literal);
        rest = tail.get(1..).unwrap_or_default();
        let (text, used) = match rest {
            [] | [b'$', ..] => (Some(&b"$"[..]), usize::from(!rest.is_empty())),
            [b'&', ..] => (group(0), 1),
            [b'`', ..] => (url.get(previous..whole.start), 1),
            [b'\'', ..] => (url.get(whole.end..), 1),
            [first @ b'0'..=b'9', second @ b'0'..=b'9', ..] => (
                group(usize::from(first - b'0') * 10 + usize::from(second - b'0')),
                2,
            ),
            [digit @ b'0'..=b'9', ..] => (group(usize::from(digit - b'0')), 1),
            _ => (Some(&b"$"[..]), 0),
        };
        out.extend_from_slice(text.unwrap_or_default());
        rest = rest.get(used..).unwrap_or_default();
    }
    out.extend_from_slice(rest);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(rules: &[(&[u8], &[u8])], url: &[u8]) -> Option<Vec<u8>> {
        let mut owner = RewriteRules::default();
        for (pattern, replacement) in rules {
            owner.add(pattern, replacement);
        }
        owner.apply(url)
    }

    #[test]
    fn first_prefix_match_wins_even_when_it_changes_nothing() {
        assert_eq!(
            apply(&[(b"/a", b"/first"), (b"/a", b"/second")], b"/a/b"),
            Some(b"/first/b".to_vec())
        );
        assert_eq!(
            apply(&[(b"/a", b"/a"), (b"/", b"/later")], b"/a"),
            Some(b"/a".to_vec())
        );
        assert_eq!(
            apply(&[(b"", b"/fallback")], b""),
            Some(b"/fallback".to_vec())
        );
        assert_eq!(
            apply(&[(b"/a\0", b"/b\0")], b"/a\0tail"),
            Some(b"/b\0tail".to_vec())
        );
        assert_eq!(apply(&[(b"/a\0", b"/b\0")], b"/a"), None);
        assert_eq!(
            apply(&[(b"~", b"/literal")], b"~x"),
            Some(b"/literalx".to_vec()),
            "one ~ is a prefix"
        );
        assert_eq!(apply(&[], b"/a"), None);
    }

    #[test]
    fn regex_rules_skip_unchanged_results_and_invalid_patterns() {
        assert_eq!(
            apply(&[(b"~^/a$~", b"/regex"), (b"/", b"/literal")], b"/a"),
            Some(b"/regex".to_vec())
        );
        assert_eq!(
            apply(&[(b"~^/a$~", b"/regex"), (b"/", b"/literal")], b"/ab"),
            Some(b"/literalab".to_vec())
        );
        assert_eq!(
            apply(&[(b"~[~", b"invalid"), (b"/a", b"fallback")], b"/a"),
            Some(b"fallback".to_vec())
        );
        assert_eq!(
            apply(&[(b"~a~", b"a"), (b"", b"fallback")], b"/a"),
            Some(b"fallback/a".to_vec())
        );
        assert_eq!(apply(&[(b"~a~", b"a")], b"/a"), None);
    }

    #[test]
    fn regex_replaces_every_match_with_ecmascript_format() {
        assert_eq!(
            apply(&[(b"~(a)(b)~", b"$2/$1")], b"/abab"),
            Some(b"/b/ab/a".to_vec())
        );
        assert_eq!(
            apply(
                &[(b"~^/view/post/(\\d+)$~", b"/static/$1.html")],
                b"/view/post/3"
            ),
            Some(b"/static/3.html".to_vec())
        );
        assert_eq!(
            apply(&[(b"~b~", b"[$`|$&|$'|$$|$x|$9|$]")], b"abc"),
            Some(b"a[a|b|c|$|$x||$]c".to_vec())
        );
        assert_eq!(
            apply(&[(b"~(b)~", b"<$01$10>")], b"abc"),
            Some(b"a<b>c".to_vec())
        );
    }

    #[test]
    fn regex_dialect_is_ecmascript() {
        let rewrite = |pattern: &str, url: &[u8]| apply(&[(pattern.as_bytes(), b"<$&>")], url);
        // Alternatives in order, lazy and counted quantifiers, any number of quantifiers.
        assert_eq!(
            apply(&[(b"~(a|ab)(c|bcd)(d*)~", b"[$1|$2|$3]")], b"abcd"),
            Some(b"[a|bcd|]".to_vec())
        );
        assert_eq!(rewrite("~a+?~", b"aa"), Some(b"<a><a>".to_vec()));
        assert_eq!(rewrite("~a{2,}?~", b"aaaaa"), Some(b"<aa><aa>a".to_vec()));
        assert_eq!(
            rewrite("~(?:ab){1,2}~", b"ababab"),
            Some(b"<abab><ab>".to_vec())
        );
        assert_eq!(rewrite("~a**~", b"baa"), Some(b"<>b<aa><>".to_vec()));
        // Classes: POSIX names, escapes, ranges, `[]` and `[^]`.
        assert_eq!(
            rewrite("~[[:digit:]_]+~", b"a1_2b"),
            Some(b"a<1_2>b".to_vec())
        );
        assert_eq!(rewrite("~[\\d.-]+~", b"v1.2-3"), Some(b"v<1.2-3>".to_vec()));
        // libstdc++'s escapes: `\uHHHH` keeps its low byte, `\cX` is `X`, `\0` is NUL even
        // before a digit, and any other letter is itself.
        assert_eq!(
            rewrite("~\\x41\\u0142\\cJ\\q\\A\\01~", b"ABJqA\x001"),
            Some(b"<ABJqA\x001>".to_vec())
        );
        assert_eq!(rewrite("~[[:DIGIT:]]+~", b"a12"), Some(b"a<12>".to_vec()));
        assert_eq!(rewrite("~[]~", b"ab"), None);
        assert_eq!(rewrite("~a[^]~", b"a\n"), Some(b"<a\n>".to_vec()));
        assert_eq!(rewrite("~\\Bb~", b"ab b"), Some(b"a<b> b".to_vec()));
        // What std::regex rejects, and backreferences and lookahead, never match.
        for pattern in [
            "a{2,1}",
            "a{",
            "(a",
            "a)",
            "[b-a]",
            "[\\d-z]",
            "*a",
            "^*",
            "\\c",
            "[\\B]",
            "[[:word:]]",
            "(a)\\1",
            "a(?=b)",
            "(?i)a",
        ] {
            assert_eq!(rewrite(&format!("~{pattern}~"), b"ab"), None, "{pattern}");
        }
        assert_eq!(
            rewrite(&format!("~{}a{}~", "(".repeat(300), ")".repeat(300)), b"a"),
            None
        );
        assert_eq!(
            rewrite("~(?:a{1000}){1000}~", b"a"),
            None,
            "over the state limit"
        );
        // Every copy of a body adds states, `(?:)`'s too, and a count over INT_MAX throws.
        for pattern in ["a(?:){100000}", "a(?:){4294967295}((?:){4294967295}){9}"] {
            assert_eq!(rewrite(&format!("~{pattern}~"), b"ba"), None, "{pattern}");
        }
    }

    #[test]
    fn empty_matches_step_like_libstdcxx() {
        assert_eq!(apply(&[(b"~~", b"x")], b"/a"), Some(b"x/xax".to_vec()));
        assert_eq!(apply(&[(b"~~", b"x")], b""), Some(b"x".to_vec()));
        // libstdc++ reports the empty match right after "aaa"; Rust's iterators skip it.
        assert_eq!(
            apply(&[(b"~a*~", b"X")], b"baaac"),
            Some(b"XbXXcX".to_vec())
        );
        // After an empty match, the first non-empty match at its end in priority order,
        // which sees that end as the input's start when the empty match was the first.
        assert_eq!(apply(&[(b"~a*?~", b"X")], b"aa"), Some(b"XXXXX".to_vec()));
        assert_eq!(apply(&[(b"~|a~", b"X")], b"/a"), Some(b"X/XXX".to_vec()));
        assert_eq!(
            apply(&[(b"~\\B|^b~", b"X")], b"abab"),
            Some(b"aXXXaXb".to_vec())
        );
        assert_eq!(
            apply(&[(b"~.~", b"-")], b"a\rb\n"),
            Some(b"-\r-\n".to_vec())
        );
    }
}
