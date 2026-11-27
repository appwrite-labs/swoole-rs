use super::{HttpStaticContext, Original, Url};
use crate::RewriteRules;

fn rewrite(rules: &RewriteRules, url: &[u8], expected: Option<&[u8]>) {
    let mut handler = HttpStaticContext::new(url, 128).unwrap();
    assert_eq!(handler.rewrite(rules).unwrap(), expected.is_some());
    assert_eq!(handler.text(Url), expected.unwrap_or(url));
    assert_eq!(handler.text(Original), url);
}

#[test]
fn rewrite_first_match_does_not_chain() {
    let mut rules = RewriteRules::default();
    rules.add(b"/old/", b"/new/").unwrap();
    rules.add(b"/new/", b"/final/").unwrap();
    rewrite(&rules, b"/old/file.html", Some(b"/new/file.html"));
    rewrite(&rules, b"/missing/file.html", None);
}

#[test]
fn rewrite_regex_replaces_all_captures() {
    let mut rules = RewriteRules::default();
    rules.add(b"~([0-9]+)~", b"[$1]").unwrap();
    rewrite(&rules, b"/12/34.html", Some(b"/[12]/[34].html"));
}

#[test]
fn rewrite_unchanged_regex_uses_next_rule() {
    let mut rules = RewriteRules::default();
    rules.add(b"~^/same$~", b"/same").unwrap();
    rules.add(b"/same", b"/fallback").unwrap();
    rewrite(&rules, b"/same", Some(b"/fallback"));
}

#[test]
fn rewrite_invalid_regex_is_ignored() {
    let mut rules = RewriteRules::default();
    rules.add(b"~[~", b"/invalid").unwrap();
    rewrite(&rules, b"/source", None);
    rules.add(b"~^/source$~", b"/target").unwrap();
    rewrite(&rules, b"/source", Some(b"/target"));
}

#[test]
fn rewrite_empty_regex_and_literal_pattern() {
    let mut rules = RewriteRules::default();
    rules.add(b"/literal.*", b"/target").unwrap();
    rules.add(b"~~", b"x").unwrap();
    rewrite(&rules, b"/literal.*file", Some(b"/targetfile"));
    rewrite(&rules, b"/a", Some(b"x/xax"));
}

#[test]
fn rewrite_compiled_regex_is_shared_between_threads() {
    let mut rules = RewriteRules::default();
    rules
        .add(b"~^/item/([0-9]+)$~", b"/static/$1.html")
        .unwrap();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for _ in 0..64 {
                    rewrite(&rules, b"/item/42", Some(b"/static/42.html"));
                }
            });
        }
    });
}
