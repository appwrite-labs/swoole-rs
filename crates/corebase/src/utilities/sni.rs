// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/server/port.cc (Swoole contributors).
//! `ListenPort::ssl_matches_wildcard_name`'s rule, over byte slices.

/// Whether an SNI `subject` matches certificate name `cert`, ignoring ASCII case
/// (`strcasecmp` in PHP's C locale). Besides an exact match, the first `*` in
/// `cert` stands for part of one label: it must be in the left-most label, the
/// text before it must start `subject`, the text after it must end `subject`
/// without overlapping that start, and what it covers holds no `.`.
#[must_use]
pub fn sni_wildcard_matches(subject: &[u8], cert: &[u8]) -> bool {
    if subject.eq_ignore_ascii_case(cert) {
        return true;
    }
    let Some(star) = cert.iter().position(|&b| b == b'*') else {
        return false;
    };
    let (prefix, suffix) = cert.split_at(star);
    let suffix = suffix.get(1..).unwrap_or_default();
    if prefix.contains(&b'.') || prefix.len() + suffix.len() > subject.len() {
        return false;
    }
    let (start, rest) = subject.split_at(prefix.len());
    let (middle, end) = rest.split_at(rest.len() - suffix.len());
    start.eq_ignore_ascii_case(prefix)
        && end.eq_ignore_ascii_case(suffix)
        && !middle.contains(&b'.')
}

#[cfg(test)]
mod tests {
    use super::sni_wildcard_matches as matches;

    #[test]
    fn wildcards_cover_one_left_most_label() {
        let m = |s: &str, c: &str| matches(s.as_bytes(), c.as_bytes());
        assert!(m("Example.COM", "example.com"), "case-insensitive exact");
        assert!(m("a.example.com", "*.example.com"), "one label");
        assert!(!m("a.b.example.com", "*.example.com"), "not two labels");
        assert!(!m("example.com", "*.example.com"), "not the bare domain");
        assert!(m("www1.example.com", "www*.example.com"), "prefix");
        assert!(!m("w.example.com", "ww*.example.com"), "prefix must match");
        assert!(!m("a.example.com", "a.*.com"), "wildcard not left-most");
        assert!(!m("a.example.com", "b.example.com"), "no wildcard");
        assert!(!m("ab", "ab*b"), "prefix and suffix may not overlap");
        assert!(m("abb", "ab*b"), "an empty middle");
    }
}
