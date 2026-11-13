//! The former `rewrite_gate.cc`, check for check: `url_rewrite_rules` in Rust against the
//! pinned `add_rewrite_rule` and `apply_rewrite_rules` (`swoole_rs_oracle::http_rewrite`),
//! over the same rules, URLs and `std::mt19937` draws. The exports the gate called are
//! replaced by the safe functions that were their bodies (README.md maps them).
//!
//! The Rust dialect is libstdc++'s `std::regex`, so this runs where the oracle links it.
#![cfg(target_os = "linux")]
use std::ffi::{c_char, c_uint, c_void};

use swoole_rs_http::{HttpStaticContext, RewriteRules};
use swoole_rs_oracle::http_rewrite as reference;
use swoole_rs_oracle::http_static_context::swr_oracle_mt19937;

type Rule<'a> = (&'a [u8], &'a [u8]);

/// The oracle's `text` callback: append one text to the `Vec<Vec<u8>>` behind `out`.
unsafe extern "C" fn text(out: *mut c_void, text: *const c_char, length: usize) {
    // SAFETY: `upstream` lends its `Vec` as `out` for the call.
    let out = unsafe { &mut *out.cast::<Vec<Vec<u8>>>() };
    // SAFETY: the oracle lends `length` bytes of a live string.
    out.push(unsafe { std::slice::from_raw_parts(text.cast::<u8>(), length) }.to_vec());
}

/// One URL through both sides: whether a rule applied, the URL, the original URL and
/// one publication exactly when a rule applied. `Err` describes a disagreement, or a
/// side that failed outright.
fn compare(url: &[u8], rules: &[Rule]) -> Result<(), String> {
    let native: Vec<_> = rules
        .iter()
        .map(|(pattern, replacement)| reference::Rule {
            pattern: pattern.as_ptr().cast(),
            pattern_length: pattern.len(),
            replacement: replacement.as_ptr().cast(),
            replacement_length: replacement.len(),
        })
        .collect();
    let mut texts: Vec<Vec<u8>> = Vec::new();
    let mut publications: c_uint = 0;
    // SAFETY: the rules and URL are live for their lengths; the outputs are locals.
    let old = unsafe {
        reference::rewrite(
            native.as_ptr(),
            native.len(),
            url.as_ptr().cast(),
            url.len(),
            &raw mut publications,
            text,
            (&raw mut texts).cast(),
        )
    };
    let [old_url, old_original] = &texts[..] else {
        return Err("upstream threw".into());
    };
    let mut candidate = RewriteRules::default();
    for (pattern, replacement) in rules {
        candidate.add(pattern, replacement);
    }
    let mut context = HttpStaticContext::new(url, 4096).ok_or("no static context")?;
    let now = context.rewrite(&candidate);
    let now_url = context.text(0).ok_or("no URL text")?;
    let now_original = context.text(1).ok_or("no original URL text")?;
    if (old != 0) == now
        && old_url == now_url
        && old_original == now_original
        && publications == c_uint::from(now)
    {
        Ok(())
    } else {
        Err(format!(
            "rewrite outcome mismatch: {} vs {}",
            old_url.escape_ascii(),
            now_url.escape_ascii()
        ))
    }
}

/// The first `count` outputs of `std::mt19937(seed)`.
fn mt19937(seed: u32, count: usize) -> std::vec::IntoIter<u32> {
    let mut outputs = vec![0; count];
    // SAFETY: `outputs` holds `count` values.
    unsafe { swr_oracle_mt19937(seed, outputs.as_mut_ptr(), count) };
    outputs.into_iter()
}

/// Run `body` on a stack the size of the gate's main thread, which both regex engines
/// recurse on. False if the thread could not start or `body` panicked.
fn on_main_stack(body: impl FnOnce() + Send) -> bool {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(8 << 20)
            .spawn_scoped(scope, body)
            .is_ok_and(|thread| thread.join().is_ok())
    })
}

#[test]
fn rule_table() {
    let long = [b'a'; 4096];
    let rules: &[&[Rule]] = &[
        &[],
        &[(b"", b"")],
        &[(b"", b"/fallback")],
        &[(b"/a", b"/b")],
        &[(b"/a", b"/a")],
        &[(b"/a", b"/first"), (b"/a", b"/second")],
        &[(b"~^/a$~", b"/regex"), (b"/", b"/literal")],
        &[(b"~[~", b"invalid"), (b"/a", b"fallback")],
        &[(b"~a~", b"a"), (b"", b"fallback")],
        &[(b"~~", b"x")],
        &[(b"~(a)(b)~", b"$2/$1")],
        &[(b"~", b"/literal")],
        &[(b"/a\0", b"/b\0")],
        // The regex dialect: every match replaced, empty matches, the $ format, classes and anchors.
        &[(b"~a*~", b"X")],
        &[(b"~a|~", b"X")],
        &[(b"~b*~", b"-")],
        &[(b"~(a)|(b)~", b"[$1|$2]")],
        &[(b"~a~", b"[$`|$&|$'|$$|$x|$9|$01|$10|$]")],
        &[(b"~a~", b"$")],
        &[
            (b"~^/view/post/(\\d+)$~", b"/static/$1.html"),
            (b"/article/", b"/static/article/"),
        ],
        &[(b"~\\w+~", b"<$&>")],
        &[(b"~\\s~", b"_")],
        &[(b"~.~", b".")],
        &[(b"~\\b~", b"|")],
        &[(b"~[^/]+$~", b"index")],
        &[(b"~(/a)+~", b"$1")],
        &[(b"~a{2}~", b"2")],
        &[(b"~\\/~", b"\\")],
        &[(b"~^~", b"^")],
        &[(b"~$~", b"$")],
        // Lazy quantifiers, alternation order, counted repeats and POSIX classes.
        &[(b"~a+?~", b"<$&>")],
        &[(b"~(a|ab)(c|bcd)(d*)~", b"[$1|$2|$3]")],
        &[(b"~x{1,2}?~", b"y")],
        &[(b"~(?:ab)+~", b"-")],
        &[(b"~[a-c]+~", b"<$&>")],
        &[(b"~\\d{2,}~", b"N")],
        &[(b"~[^a-z/]~", b"_")],
        &[(b"~[[:digit:]]+~", b"#")],
        &[(b"~[[:alpha:]_]+$~", b"$&!")],
        // A non-empty match after an empty one, escapes as libstdc++ reads them, and the state limit.
        &[(b"~a*?~", b"X")],
        &[(b"~|a~", b"X")],
        &[(b"~\\B|^b~", b"X")],
        &[(b"~\\x41\\u0142\\cJ\\q\\A\\01~", b"<$&>")],
        &[(b"~[[:DIGIT:]]+~", b"#")],
        &[(b"~a(?:){100000}~", b"b")],
        &[(b"~a(?:a{0}){100000}~", b"b")],
        &[(b"~a(?:){4294967295}~", b"b")],
        // Ranges whose ends straddle 0x7f/0x80, which libstdc++ compares as char.
        &[(b"~[a-\\xff]~", b"<$&>")],
        &[(b"~[\\xff-a]~", b"<$&>")],
        &[(b"~[\\x7f-\\x80]~", b"<$&>")],
        // Collating elements by POSIX name, equivalence classes, and either before a dash.
        &[(
            b"~[[.circumflex.][.left-curly-bracket.][.vertical-line.][.backslash.][.IS4.][.alert.]]~",
            b"<$&>",
        )],
        &[(b"~[[.1.]]~", b"X")],
        &[(b"~[[.a.]]~", b"X")],
        &[(b"~[[=A=]]~", b"<$&>")],
        &[(b"~[[.hyphen.]-[.period.]]~", b"<$&>")],
        &[(b"~[[.hyphen.]-/]~", b"<$&>")],
        &[(b"~[[=a=]-z]~", b"<$&>")],
        &[(b"~[[.a.]-z]~", b"<$&>")],
        &[(b"~[[:alpha:]-z]~", b"<$&>")],
    ];
    let urls: &[&[u8]] = &[
        b"",
        b"/",
        b"/a",
        b"/a/b",
        b"/ab",
        b"/aa",
        b"/a\0tail",
        &long,
        b"baaac",
        b"/view/post/3",
        b"/view/post/x",
        b"/article/settings.html",
        b"a b\r\nc",
        b"/a/a/a",
        b"abcd",
        b"/a1b22c333/_x",
        b"abab",
        b"ABJqA\x001",
        b"\xff\0a\x80\x7f",
        b"^{|\\\x1c\x07aAz-./",
    ];
    assert!(on_main_stack(|| {
        for url in urls {
            for set in rules {
                if let Err(mismatch) = compare(url, set) {
                    panic!("{mismatch} for {:?}", url.escape_ascii());
                }
            }
        }
    }));
}

/// Random byte URLs under up to seven prefix rules of random replacements, half of them
/// with a prefix that cannot match.
#[test]
fn random_prefix_rules() {
    assert!(on_main_stack(|| {
        let mut draws = mt19937(34129, 2048 * 800);
        // `rng() % modulus`.
        let mut below = |modulus: usize| usize::try_from(draws.next().unwrap()).unwrap() % modulus;
        for _ in 0..2048 {
            let url: Vec<u8> = (0..below(80))
                .map(|_| u8::try_from(below(256)).unwrap())
                .collect();
            let mut rules = Vec::new();
            for _ in 0..below(8) {
                let cut = if url.is_empty() {
                    0
                } else {
                    below(url.len() + 1)
                };
                let mut prefix = url[..cut].to_vec();
                let replacement: Vec<u8> = (0..below(80))
                    .map(|_| u8::try_from(below(256)).unwrap())
                    .collect();
                if below(2) != 0 {
                    prefix = [b"mismatch", &prefix[..]].concat();
                }
                rules.push((prefix, replacement));
            }
            let rules: Vec<Rule> = rules.iter().map(|(p, r)| (&p[..], &r[..])).collect();
            if let Err(mismatch) = compare(&url, &rules) {
                panic!("{mismatch} for {:?}", url.escape_ascii());
            }
        }
    }));
}

/// Random patterns from `std::regex`'s ECMAScript grammar, one rule each, over short URLs
/// of the same bytes; every mismatch is listed. No atom starts with a digit, `=` or `!`,
/// so none forms a backreference or lookahead, which the Rust dialect leaves out.
#[test]
fn regex_sweep() {
    const ATOMS: &[&[u8]] = &[
        b"a",
        b"b",
        b"/",
        b"\x80",
        b"\xff",
        b".",
        b"\\d",
        b"\\w",
        b"\\s",
        b"\\W",
        b"\\b",
        b"\\B",
        b"\\x80",
        b"\\xff",
        b"\\u00e9",
        b"\\cJ",
        b"\\0",
        b"\\/",
        b"\\.",
        b"[ab]",
        b"[^a]",
        b"[a-\\xff]",
        b"[\\x7f-\\x80]",
        b"[\\x80-\\xff]",
        b"[\\d-]",
        b"[[:alpha:]]",
        b"[]",
        b"[^]",
        b"^",
        b"$",
        b"(",
        b")",
        b"(?:",
        b"|",
        b"*",
        b"+",
        b"?",
        b"*?",
        b"+?",
        b"??",
        b"{2}",
        b"{0,1}",
        b"{1,}",
        b"{0}",
        b"{",
        b"}",
        b"]",
        b"[",
        b"-",
        b",",
        b"\\",
        // Escapes and class syntax at their edges: short or bad hex, `\c` before a non-letter,
        // control escapes, `\b` and `\B` in a class, collating and equivalence classes, unknown
        // and unclosed names, and counted repeats left open or reversed.
        b"\\x4g",
        b"\\u12",
        b"\\c1",
        b"\\c",
        b"\\n",
        b"\\t",
        b"\\v",
        b"\\f",
        b"\\r",
        b"\\S",
        b"\\D",
        b"\\e",
        b"\\$",
        b"\\^",
        b"\\*",
        b"\\(",
        b"\\|",
        b"[\\b]",
        b"[\\B]",
        b"[\\s\\S]",
        b"[[.a.]]",
        b"[[=a=]]",
        b"[[:foo:]]",
        b"[[:",
        b"[:a:]",
        b"(?",
        b"{1",
        b"{,2}",
        b"{2,1}",
    ];
    const BYTES: &[&[u8]] = &[b"a", b"b", b"/", b"\x80", b"\xff", b"1", b" ", b"\n", b"_"];
    const FORMATS: &[&[u8]] = &[b"X", b"$&", b"[$1]", b"<$2>", b"$`", b"$'", b"", b"$$"];
    assert!(on_main_stack(|| {
        let mut draws = mt19937(7349, 15000 * 18);
        // `rng() % modulus`.
        let mut below = |modulus: usize| usize::try_from(draws.next().unwrap()).unwrap() % modulus;
        let mut failures = Vec::new();
        for _ in 0..15000 {
            let mut pattern = b"~".to_vec();
            for _ in 0..=below(7) {
                pattern.extend_from_slice(ATOMS[below(ATOMS.len())]);
            }
            pattern.push(b'~');
            let mut url = Vec::new();
            for _ in 0..below(9) {
                url.extend_from_slice(BYTES[below(BYTES.len())]);
            }
            let format = FORMATS[below(FORMATS.len())];
            if let Err(mismatch) = compare(&url, &[(&pattern, format)]) {
                failures.push(format!(
                    "{} on \"{}\": {mismatch}",
                    pattern.escape_ascii(),
                    url.escape_ascii()
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} random regex rules disagree:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }));
}

#[test]
fn no_rules_short_circuit() {
    assert_eq!(reference::without_rules(), 0, "null rules short circuit");
}
