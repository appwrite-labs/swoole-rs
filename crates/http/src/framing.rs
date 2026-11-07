//! Bounded equivalents of Swoole's permissive first-party HTTP framing.
use super::{HttpSpan, INVALID, bytes, maps::METHODS};
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
    pub(crate) fn parse(&mut self, input: &[u8], operation: u32) -> i32 {
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
fn hex(byte: u8) -> Option<usize> {
    match byte {
        b'0'..=b'9' => Some(usize::from(byte - b'0')),
        b'a'..=b'f' => Some(usize::from(byte - b'a' + 10)),
        b'A'..=b'F' => Some(usize::from(byte - b'A' + 10)),
        _ => None,
    }
}
fn chunk_number(input: &[u8], begin: usize) -> (usize, usize) {
    let mut at = begin;
    while input.get(at).is_some_and(|&b| c_space(b)) {
        at += 1;
    }
    let negative = input.get(at) == Some(&b'-');
    if negative || input.get(at) == Some(&b'+') {
        at += 1;
    }
    if input.get(at) == Some(&b'0')
        && matches!(input.get(at + 1), Some(b'x' | b'X'))
        && input.get(at + 2).and_then(|&b| hex(b)).is_some()
    {
        at += 2;
    }
    let digits = at;
    let mut value = Some(0usize);
    while let Some(digit) = input.get(at).and_then(|&b| hex(b)) {
        value = value.and_then(|v| v.checked_mul(16)?.checked_add(digit));
        at += 1;
    }
    if at == digits {
        return (0, begin);
    }
    let value = value.map_or(usize::MAX, |v| if negative { v.wrapping_neg() } else { v });
    (value, at)
}
#[repr(C)]
#[derive(Default)]
pub struct HttpChunkStep {
    pub line_offset: usize,
    pub line_length: usize,
    pub chunk_length: usize,
    pub next: usize,
    pub body_length: u64,
}
pub(crate) fn chunk_step(
    input: &[u8],
    cursor: usize,
    header: usize,
    initial: bool,
) -> (i32, HttpChunkStep) {
    let mut step = HttpChunkStep::default();
    if cursor > input.len() || header > input.len() {
        return (INVALID, step);
    }
    if initial && (input.len() - cursor < 5 || !input.ends_with(b"0\r\n\r\n")) {
        return (INVALID, step);
    }
    let (length, end) = chunk_number(input, cursor);
    if input.get(end) != Some(&b'\r') {
        return (-2, step);
    }
    step.line_offset = cursor;
    step.line_length = end - cursor;
    step.chunk_length = length;
    if length == 0 {
        step.body_length = (end as u64).wrapping_sub(header as u64).wrapping_add(4);
        return (2, step);
    }
    if let Some(next) = end
        .checked_add(4)
        .and_then(|v| v.checked_add(length))
        .filter(|&n| n < input.len())
    {
        step.next = next;
        (1, step)
    } else {
        (3, step)
    }
}
/// Preserve the native Expect scanner's exact skip/matching behavior.
/// # Safety
/// Input is readable. No references, callbacks or allocation escape.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_expect(data: *const u8, length: usize) -> i32 {
    // SAFETY: the caller lends `length` initialized bytes at `data` (null only when empty).
    let input = match unsafe { bytes(data, length) } {
        Ok(v) => v,
        Err(e) => return e,
    };
    // The native byte loop, jumping between CRs: with a body attached (echo-64k) the
    // scan runs over the whole request, so per-byte cost dominates (#338).
    let mut at = 0;
    while let Some(cr) = memchr::memchr(b'\r', tail(input, at)) {
        at += cr;
        if length - at > 9 {
            at += 2;
            if starts(tail(input, at), b"Expect: ") {
                return i32::from(starts(tail(input, at + 8), b"100-continue"));
            }
            at += 1;
        }
        at += 1;
    }
    0
}
pub(crate) fn header_value(
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
                if !error && starts(tail(input, at), name) {
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
    fn expect(input: &[u8]) -> i32 {
        // SAFETY: a live slice.
        unsafe { swr_v1_http_expect(input.as_ptr(), input.len()) }
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
                expect(input),
                expect_reference(input),
                "{:?}",
                String::from_utf8_lossy(input)
            );
        }
        assert_eq!(expect(b"GET / HTTP/1.1\r\nExpect: 100-continue\r\n\r\n"), 1);
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
    fn incremental_headers_and_bounded_chunk_scanning() {
        let mut f = HttpFrame::default();
        assert_eq!(header_end(b"abc\r\n\r", &mut f), INVALID);
        assert_eq!(f.offset, 3);
        assert_eq!(header_end(b"abc\r\n\r\n", &mut f), 0);
        assert_eq!(f.header_length, 7);
        let (status, step) = chunk_step(b"3\r\nabc\r\n0\r\n\r\n", 0, 0, true);
        assert_eq!((status, step.chunk_length, step.next), (1, 3, 8));
        let (status, step) = chunk_step(b"3\r\nabc\r\n0\r\n\r\n", 8, 0, false);
        assert_eq!((status, step.body_length), (2, 13));
        assert_eq!(
            chunk_step(b"FFFFFFFFFFFFFFFF\r\nx0\r\n\r\n", 0, 0, true).0,
            3
        );
    }
}
