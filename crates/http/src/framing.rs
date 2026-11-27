//! Bounded equivalents of Swoole's permissive first-party HTTP framing.
use super::{HttpSpan, HttpVersion, maps::METHODS};
use swoole_rs_corebase::constants::HTTP2_PREFACE;
#[derive(Default, Clone, Copy)]
pub struct HttpFrame {
    pub offset: usize,
    pub content_length: u64,
    pub method: u32,
    pub version: u32,
    pub excepted: u32,
    pub url_offset: u32,
    pub url_length: u32,
    pub line_length: u32,
    pub header_length: u32,
    pub known_length: u32,
}
impl HttpFrame {
    /// Reads the request line (or an HTTP/2 preface): whether it is complete and valid.
    pub fn request_line(&mut self, input: &[u8]) -> bool {
        request_line(input, self)
    }

    /// Finds the header block's end: whether it is there yet.
    pub fn header_end(&mut self, input: &[u8]) -> bool {
        header_end(input, self)
    }
}
fn c_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t'..=b'\r')
}
/// `input[at..]`, or empty past the end.
fn tail(input: &[u8], at: usize) -> &[u8] {
    input.get(at..).unwrap_or_default()
}
fn starts(input: &[u8], needle: &[u8]) -> bool {
    super::case_prefix(input, needle)
}
/// A request-buffer position stored in one of upstream's `uint32_t` Request fields.
/// Request buffers are capped by `package_max_length`, a `uint32_t`.
fn field(position: usize) -> u32 {
    u32::try_from(position).unwrap_or(u32::MAX)
}
fn request_line(input: &[u8], frame: &mut HttpFrame) -> bool {
    if input.len() < 16 {
        return false;
    }
    // Every method but the trailing PRI, which is matched below.
    let pri = METHODS.len() - 1;
    let mut at = if let Some((index, method)) = METHODS
        .iter()
        .take(pri)
        .enumerate()
        // Byte by byte, the first byte ruling most out: a `bcmp` call's cost
        // depends on where the buffer sits (glibc's page-crossing check), which
        // moved process-mode's instruction count between runs of one build (#5554).
        .find(|(_, m)| {
            let m = m.to_bytes();
            input.len() >= m.len() && m.iter().zip(input).all(|(a, b)| a == b)
        }) {
        frame.method = u32::try_from(index + 1).unwrap_or(0);
        method.to_bytes().len()
    } else if input.starts_with(b"PRI") {
        frame.method = u32::from(crate::METHOD_PRI);
        if input.starts_with(HTTP2_PREFACE) {
            frame.offset = HTTP2_PREFACE.len();
            return true;
        }
        frame.excepted = 1;
        return false;
    } else {
        frame.excepted = 1;
        return false;
    };
    let mut state = 0;
    while let Some(&byte) = input.get(at) {
        match state {
            0 => {
                if byte != b' ' {
                    frame.url_offset = field(at);
                    state = 1;
                }
            }
            1 => match byte {
                b' ' => {
                    frame.url_length = field(at).wrapping_sub(frame.url_offset);
                    state = 2;
                }
                b'\r' | b'\n' => {
                    frame.excepted = 1;
                    return false;
                }
                _ => {}
            },
            2 => {
                if byte != b' ' {
                    frame.version = match input.get(at..at + 8) {
                        None => break,
                        Some(b"HTTP/1.1") => HttpVersion::Http11 as u32,
                        Some(b"HTTP/1.0") => HttpVersion::Http10 as u32,
                        Some(_) => {
                            frame.excepted = 1;
                            return false;
                        }
                    };
                    frame.offset = at + 8;
                    frame.line_length = field(frame.offset);
                    return true;
                }
            }
            _ => unreachable!(),
        }
        at += 1;
    }
    // A zero method tells the read loop to retry with the next packet.
    frame.method = 0;
    false
}
/// Offset of the first CRLFCRLF that separates HTTP headers from the body.
#[must_use]
pub fn header_delimiter(input: &[u8]) -> Option<usize> {
    swoole_rs_corebase::buffer::string::find_bytes(input, b"\r\n\r\n")
}

fn header_end(input: &[u8], frame: &mut HttpFrame) -> bool {
    let Some(rest) = input.get(frame.offset..) else {
        return false;
    };
    if let Some(at) = header_delimiter(rest) {
        frame.offset += at + 4;
        frame.header_length = field(frame.offset);
        return true;
    }
    frame.offset = frame.offset.max(input.len().saturating_sub(3));
    false
}
/// Upstream's `has_expect_header`: an `Expect: 100-continue` header, found with the
/// native scanner's exact skip/matching behavior.
#[must_use]
pub fn expect(input: &[u8]) -> bool {
    let length = input.len();
    // The native byte loop, jumping between CRs: with a body attached (echo-64k) the
    // scan runs over the whole request, so per-byte cost dominates (#338).
    let mut at = 0;
    while let Some(cr) = memchr::memchr(b'\r', tail(input, at)) {
        at += cr;
        if length - at > 9 {
            at += 2;
            if starts(tail(input, at), b"Expect:") {
                return starts(super::skip_ows(tail(input, at + 7)), b"100-continue");
            }
            at += 1;
        }
        at += 1;
    }
    false
}
/// The value of header `name` among the lines of `input[start..end]`; `None` when
/// it is absent or the bounds are bad.
#[must_use]
pub fn header_value(input: &[u8], start: usize, end: usize, name: &[u8]) -> Option<HttpSpan> {
    let name_length = name.len();
    let input = input.get(..end).filter(|_| start <= end)?;
    let (mut at, mut state, mut index, mut buffer, mut error) = (start, 0, 0usize, 0, false);
    while let Some(&byte) = input.get(at) {
        match state {
            0 => {
                if tail(input, at).starts_with(b"\r\n") {
                    index = 0;
                    error = false;
                    at += 1;
                    continue;
                }
                // Only a name at `index` 0 or 1 can be the value's: past that, a match
                // only sets `error`, which nothing reads before the line's CRLF clears it.
                // Skipping those compares keeps `strncasecmp` to two per line (#3411).
                if !error && index <= 1 && starts(tail(input, at), name) {
                    let colon = at + name_length;
                    if input.get(colon) != Some(&b':') || colon >= end || index > 1 {
                        error = true;
                        at += 1;
                        continue;
                    }
                    at = colon;
                    state = 1;
                }
                index += 1;
            }
            1 => {
                if !c_space(byte) {
                    buffer = at;
                    state = 2;
                }
            }
            2 => {
                if tail(input, at).starts_with(b"\r\n") {
                    return Some(HttpSpan {
                        offset: buffer,
                        length: super::rtrim_ows(input.get(buffer..at)?).len(),
                    });
                }
            }
            _ => unreachable!(),
        }
        at += 1;
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_ids_match_the_pinned_http_enum() {
        assert_eq!(
            [HttpVersion::Http10, HttpVersion::Http11, HttpVersion::Http2].map(|v| v as u8),
            swoole_rs_oracle::http_frame::VERSIONS,
            "swHttpVersion"
        );
    }

    /// The native loop, comparing the name at every byte of a line.
    fn header_value_reference(input: &[u8], start: usize, end: usize, name: &[u8]) -> HttpSpan {
        let input = &input[..end];
        let (mut at, mut state, mut index, mut buffer, mut error) = (start, 0, 0usize, 0, false);
        while at < input.len() {
            match state {
                0 => {
                    if input[at..].starts_with(b"\r\n") {
                        (index, error) = (0, false);
                        at += 1;
                        continue;
                    }
                    if !error && starts(&input[at..], name) {
                        let colon = at + name.len();
                        if input.get(colon) != Some(&b':') || colon >= end || index > 1 {
                            error = true;
                            at += 1;
                            continue;
                        }
                        at = colon;
                        state = 1;
                    }
                    index += 1;
                }
                1 if !c_space(input[at]) => (buffer, state) = (at, 2),
                2 if input[at..].starts_with(b"\r\n") => {
                    return HttpSpan {
                        offset: buffer,
                        length: input[buffer..at]
                            .iter()
                            .rposition(|b| !matches!(b, b' ' | b'\t'))
                            .map_or(0, |last| last + 1),
                    };
                }
                _ => {}
            }
            at += 1;
        }
        HttpSpan::default()
    }
    #[test]
    fn header_value_matches_the_native_loop() {
        // Every string over the bytes the loop branches on, up to six long.
        let alphabet = b"\r\naA: ";
        let mut inputs = vec![Vec::new()];
        for _ in 0..6 {
            let longer: Vec<Vec<u8>> = inputs
                .iter()
                .filter(|s| s.len() == inputs.last().map_or(0, Vec::len))
                .flat_map(|s| alphabet.iter().map(move |&b| [s.as_slice(), &[b]].concat()))
                .collect();
            inputs.extend(longer);
        }
        for name in [&b"a"[..], b"aa", b"Aa:"] {
            for input in &inputs {
                for start in 0..input.len().min(3) {
                    let found = header_value(input, start, input.len(), name);
                    let want = header_value_reference(input, start, input.len(), name);
                    assert_eq!(found.is_some(), want != HttpSpan::default());
                    assert_eq!(
                        found.unwrap_or_default(),
                        want,
                        "{:?} in {:?}",
                        name,
                        String::from_utf8_lossy(input)
                    );
                }
            }
        }
    }
    #[test]
    fn request_line_retains_permissive_prefix_matching_and_partial_mutations() {
        let mut f = HttpFrame::default();
        assert!(request_line(b"GET/target HTTP/1.1\r\n", &mut f));
        assert_eq!(
            (f.method, f.url_offset, f.url_length, f.version, f.offset),
            (2, 3, 7, 2, 19)
        );
        let mut f = HttpFrame::default();
        assert!(!request_line(b"GET /long-target HTTP/1", &mut f));
        assert_eq!((f.method, f.excepted), (0, 0));
        assert_eq!(f.url_length, 12);
        assert!(!request_line(b"GET /long-target", &mut f));
        assert_eq!((f.method, f.excepted), (0, 0));
        let mut f = HttpFrame::default();
        assert!(request_line(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n", &mut f));
        assert_eq!(f.offset, 24);
    }
    #[test]
    fn incremental_headers() {
        let mut f = HttpFrame::default();
        assert!(!header_end(b"abc\r\n\r", &mut f));
        assert_eq!(f.offset, 3);
        assert!(header_end(b"abc\r\n\r\n", &mut f));
        assert_eq!(f.header_length, 7);
    }
}
