//! `Request::get_chunked_body_length`: a chunked body's length, scanned across reads.

/// `ChunkState`.
pub const CHUNK_SIZE: u8 = 0;
pub const CHUNK_DATA: u8 = 1;
pub const CHUNK_TRAILER: u8 = 2;

/// Where a chunked body's scan stopped: the request's chunk fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HttpChunks {
    /// `CHUNK_SIZE`, `CHUNK_DATA` or `CHUNK_TRAILER`: what the body expects next.
    pub state: u8,
    /// The current chunk's size, where its line or data starts (0 until the body
    /// is first scanned), and how far its size line has been scanned.
    pub length: usize,
    pub offset: usize,
    pub scan_offset: usize,
}

/// How a scan ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpChunked {
    /// The trailer section ended; the body's length from the header's end.
    Complete(u64),
    /// More input is needed.
    Partial,
    /// Bad framing.
    Malformed,
    /// Past the maximum length.
    TooLarge,
}

/// RFC 9110 `tchar`.
fn token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

/// `qdtext`, or with `pair` the character of a `quoted-pair`, which may also be `"` or `\\`.
fn quoted_text(byte: u8, pair: bool) -> bool {
    byte == b'\t' || (byte >= 0x20 && byte != 0x7f && (pair || (byte != b'"' && byte != b'\\')))
}

/// A chunk size line without its CRLF: hex digits, then `;name[=token|"quoted"]`
/// extensions, with no size above `max`.
fn chunk_size(line: &[u8], max: usize) -> Result<usize, HttpChunked> {
    let mut length = 0usize;
    let mut at = 0;
    while let Some(digit) = line.get(at).and_then(|&b| char::from(b).to_digit(16)) {
        let digit = digit as usize;
        if digit > max || length > (max - digit) / 16 {
            return Err(HttpChunked::TooLarge);
        }
        length = length * 16 + digit;
        at += 1;
    }
    if at == 0 {
        return Err(HttpChunked::Malformed);
    }
    let run = |at: &mut usize, test: fn(u8) -> bool| {
        let start = *at;
        while line.get(*at).is_some_and(|&b| test(b)) {
            *at += 1;
        }
        *at > start
    };
    while let Some(&byte) = line.get(at) {
        at += 1;
        if byte != b';' || !run(&mut at, token) {
            return Err(HttpChunked::Malformed);
        }
        match line.get(at) {
            None => break,
            Some(b'=') => at += 1,
            Some(_) => continue,
        }
        match line.get(at) {
            Some(b'"') => loop {
                at += 1;
                match line.get(at) {
                    Some(b'"') => {
                        at += 1;
                        break;
                    }
                    Some(b'\\') if line.get(at + 1).is_some_and(|&b| quoted_text(b, true)) => {
                        at += 1;
                    }
                    Some(&b) if quoted_text(b, false) => {}
                    _ => return Err(HttpChunked::Malformed),
                }
            },
            _ if run(&mut at, token) => {}
            _ => return Err(HttpChunked::Malformed),
        }
    }
    Ok(length)
}

impl HttpChunks {
    /// Scans `input`'s chunked body, which starts at `header_length`, from where the
    /// last call stopped, with nothing past `max`.
    pub fn scan(&mut self, input: &[u8], header_length: usize, max: usize) -> HttpChunked {
        // A clean request leaves zero as the uninitialized offset, and the header has
        // been parsed here.
        if self.offset == 0 {
            self.offset = header_length;
            self.scan_offset = self.offset;
        }
        loop {
            if self.state == CHUNK_DATA {
                if self.offset > input.len() || self.length > input.len() - self.offset {
                    return HttpChunked::Partial;
                }
                let end = self.offset + self.length;
                for (at, byte) in [(end, b'\r'), (end + 1, b'\n')] {
                    match input.get(at) {
                        None => return HttpChunked::Partial,
                        Some(&b) if b != byte => return HttpChunked::Malformed,
                        _ => {}
                    }
                }
                (self.offset, self.scan_offset) = (end + 2, end + 2);
                self.length = 0;
                self.state = CHUNK_SIZE;
                continue;
            }
            let line_start = self.offset;
            let scan = self.scan_offset.min(input.len());
            let rest = input.get(scan..).unwrap_or_default();
            let Some(found) = memchr::memchr2(b'\r', b'\n', rest) else {
                self.scan_offset = input.len();
                return HttpChunked::Partial;
            };
            let line_end = scan + found;
            match input.get(line_end..) {
                Some([b'\r', b'\n', ..]) => {}
                Some([b'\r']) => {
                    self.scan_offset = line_end;
                    return HttpChunked::Partial;
                }
                _ => return HttpChunked::Malformed,
            }
            (self.offset, self.scan_offset) = (line_end + 2, line_end + 2);
            if self.state == CHUNK_TRAILER {
                if self.offset > max {
                    return HttpChunked::TooLarge;
                }
                if line_end == line_start {
                    return HttpChunked::Complete((self.offset - header_length) as u64);
                }
                continue;
            }
            let line = input.get(line_start..line_end).unwrap_or_default();
            self.length = match chunk_size(line, max) {
                Ok(length) => length,
                Err(outcome) => return outcome,
            };
            // What must still fit: the chunk and its CRLF, then the last chunk and the
            // trailer section's end; the last chunk's trailer only needs its CRLF.
            let (state, rest) = if self.length == 0 {
                (CHUNK_TRAILER, Some(2))
            } else {
                (CHUNK_DATA, self.length.checked_add(2 + 5))
            };
            self.state = state;
            if rest.is_none_or(|rest| max.checked_sub(self.offset).is_none_or(|room| room < rest)) {
                return HttpChunked::TooLarge;
            }
        }
    }
}
