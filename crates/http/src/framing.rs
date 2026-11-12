//! Bounded equivalents of Swoole's permissive first-party HTTP framing.
use super::{HttpSpan, INVALID, maps::METHODS};
#[repr(C)]
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
    /// Operation 0 reads the request line (or an HTTP/2 preface), 1 finds the header
    /// block's end: 0 on success.
    pub fn parse(&mut self, input: &[u8], operation: u32) -> i32 {
        match operation {
            0 => request_line(input, self),
            1 => header_end(input, self),
            _ => INVALID,
        }
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
#[expect(
    clippy::cast_possible_truncation,
    reason = "request buffers are capped by package_max_length, a uint32_t"
)]
fn field(position: usize) -> u32 {
    position as u32
}
fn request_line(input: &[u8], frame: &mut HttpFrame) -> i32 {
    if input.len() < 16 {
        return INVALID;
    }
    // Every method but the trailing PRI, which is matched below.
    let pri = METHODS.len() - 1;
    let mut at = if let Some((index, method)) = METHODS
        .iter()
        .take(pri)
        .enumerate()
        .find(|(_, m)| input.starts_with(m.to_bytes()))
    {
        frame.method = u32::try_from(index + 1).unwrap_or(0);
        method.to_bytes().len()
    } else if input.starts_with(b"PRI") {
        frame.method = u32::try_from(METHODS.len()).unwrap_or(0);
        if input.starts_with(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n") {
            frame.offset = 24;
            return 0;
        }
        frame.excepted = 1;
        return INVALID;
    } else {
        frame.excepted = 1;
        return INVALID;
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
                    return INVALID;
                }
                _ => {}
            },
            2 => {
                if byte != b' ' {
                    frame.version = match input.get(at..at + 8) {
                        None => return INVALID,
                        Some(b"HTTP/1.1") => 2,
                        Some(b"HTTP/1.0") => 1,
                        Some(_) => {
                            frame.excepted = 1;
                            return INVALID;
                        }
                    };
                    frame.offset = at + 8;
                    frame.line_length = field(frame.offset);
                    return 0;
                }
            }
            _ => unreachable!(),
        }
        at += 1;
    }
    // The native implementation forms input.end()+8 here even when no version
    // was found. Keep partial state but reject that out-of-allocation pointer.
    INVALID
}
fn header_end(input: &[u8], frame: &mut HttpFrame) -> i32 {
    // Jump to each '\r' and test the four bytes there. Upstream's get_header_length
    // compares at every position; both find the first "\r\n\r\n".
    let Some(rest) = input.get(frame.offset..) else {
        return INVALID;
    };
    let mut from = 0;
    while let Some(found) = rest
        .get(from..)
        .and_then(|tail| memchr::memchr(b'\r', tail))
    {
        let at = from + found;
        if rest.get(at..at + 4) == Some(b"\r\n\r\n") {
            frame.offset += at + 4;
            frame.header_length = field(frame.offset);
            return 0;
        }
        from = at + 1;
    }
    frame.offset = frame.offset.max(input.len().saturating_sub(3));
    INVALID
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
            if starts(tail(input, at), b"Expect: ") {
                return starts(tail(input, at + 8), b"100-continue");
            }
            at += 1;
        }
        at += 1;
    }
    false
}
/// The value of header `name` among the lines of `input[start..end]`, into `output`:
/// 1 when found, 0 when not, `INVALID` for bad bounds.
pub fn header_value(
    input: &[u8],
    start: usize,
    end: usize,
    name: &[u8],
    output: &mut HttpSpan,
) -> i32 {
    let name_length = name.len();
    *output = HttpSpan::default();
    let Some(input) = input.get(..end).filter(|_| start <= end) else {
        return INVALID;
    };
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
                    *output = HttpSpan {
                        offset: buffer,
                        length: at - buffer,
                    };
                    return 1;
                }
            }
            _ => unreachable!(),
        }
        at += 1;
    }
    0
}
#[cfg(test)]
mod tests {
    use super::*;
    /// The native byte-at-a-time loop the memchr scan must match.
    fn expect_reference(input: &[u8]) -> i32 {
        let (length, mut at) = (input.len(), 0);
        while at < length {
            if input[at] == b'\r' && length - at > 9 {
                at += 2;
                if starts(&input[at..], b"Expect: ") {
                    return i32::from(starts(&input[at + 8..], b"100-continue"));
                }
                at += 1;
            }
            at += 1;
        }
        0
    }
    #[test]
    fn expect_scan_matches_the_native_loop() {
        // Every string over the bytes the loop branches on, up to six long.
        let alphabet = b"\r\nEx";
        let mut inputs = vec![Vec::new()];
        for _ in 0..6 {
            let longer: Vec<Vec<u8>> = inputs
                .iter()
                .flat_map(|s| alphabet.iter().map(move |&b| [s.as_slice(), &[b]].concat()))
                .collect();
            inputs.extend(longer);
        }
        for prefix in ["", "x", "\r", "\r\n", "GET / HTTP/1.1"] {
            for header in [
                "\r\nExpect: 100-continue",
                "\r\nexpect: 100-CONTINUE",
                "\r\nExpect: nope",
                "\r\nExpect: 100",
                "\r\nX: 1\r\nExpect: 100-continue",
            ] {
                for suffix in ["", "\r\n", "\r\n\r\nbody\r"] {
                    inputs.push([prefix, header, suffix].concat().into_bytes());
                }
            }
        }
        for input in &inputs {
            assert_eq!(
                i32::from(expect(input)),
                expect_reference(input),
                "{:?}",
                String::from_utf8_lossy(input)
            );
        }
        assert!(expect(b"GET / HTTP/1.1\r\nExpect: 100-continue\r\n\r\n"));
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
                        length: at - buffer,
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
                    let mut got = HttpSpan::default();
                    let found = header_value(input, start, input.len(), name, &mut got);
                    let want = header_value_reference(input, start, input.len(), name);
                    assert_eq!(found, i32::from(want != HttpSpan::default()));
                    assert_eq!(
                        got,
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
        assert_eq!(request_line(b"GET/target HTTP/1.1\r\n", &mut f), 0);
        assert_eq!(
            (f.method, f.url_offset, f.url_length, f.version, f.offset),
            (2, 3, 7, 2, 19)
        );
        let mut f = HttpFrame::default();
        assert_eq!(request_line(b"GET /long-target HTTP/1", &mut f), INVALID);
        assert_eq!(f.excepted, 0);
        assert_eq!(f.url_length, 12);
        let mut f = HttpFrame::default();
        assert_eq!(request_line(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n", &mut f), 0);
        assert_eq!(f.offset, 24);
    }
    #[test]
    fn incremental_headers() {
        let mut f = HttpFrame::default();
        assert_eq!(header_end(b"abc\r\n\r", &mut f), INVALID);
        assert_eq!(f.offset, 3);
        assert_eq!(header_end(b"abc\r\n\r\n", &mut f), 0);
        assert_eq!(f.header_length, 7);
    }
}
