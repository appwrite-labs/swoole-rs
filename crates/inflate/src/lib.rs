//! HTTP body codecs over the system libraries the PHP build links: response
//! decompression (gzip/deflate over a per-`Client` zlib stream, with the raw-deflate
//! retry; Brotli and zstd streams in [`brotli`] and [`zstd`]), their compression for
//! the server, and permessage-deflate inflation.

#![forbid(unsafe_code)]

pub mod zstd;

pub use swoole_rs_brotli as brotli;

use std::ffi::c_int;

pub use swoole_rs_zlib::error;
use swoole_rs_zlib::{Stream, Z_DATA_ERROR, Z_OK, Z_STREAM_END, Z_SYNC_FLUSH};

// swoole's SW_ZLIB_ENCODING_* window-bits (ext-src/php_swoole_http.h).
const ENCODING_GZIP: c_int = 0x1f;
const ENCODING_DEFLATE: c_int = 0x0f;
const ENCODING_RAW: c_int = -0xf;

const OUT_GROW: usize = 16384;

/// A per-response gzip/deflate inflate stream. `out` holds the bytes produced by
/// the most recent chunk; the caller copies them into its body buffer.
pub struct Inflate {
    stream: Option<Stream>,
    first_chunk: bool,
    gzip: bool,
    out: Vec<u8>,
}

/// Inflate all of `input` into `out`, or the zlib error status. Mirrors the
/// pinned `Z_SYNC_FLUSH` loop.
fn run(stream: &mut Stream, mut input: &[u8], out: &mut Vec<u8>) -> Result<(), c_int> {
    out.clear();
    loop {
        if out.len() == out.capacity() {
            out.reserve(OUT_GROW);
        }
        let step = stream.run_into(&mut input, out, Z_SYNC_FLUSH);
        if step.status == Z_STREAM_END || (step.status == Z_OK && input.is_empty()) {
            return Ok(());
        }
        if step.status != Z_OK {
            return Err(step.status);
        }
    }
}

impl Inflate {
    /// A gzip (or deflate) response decompressor.
    #[must_use]
    pub fn new(gzip: bool) -> Self {
        Self {
            stream: None,
            first_chunk: true,
            gzip,
            out: Vec::new(),
        }
    }

    /// A gzip-only decompressor (`inflateInit2(MAX_WBITS + 16)`), which never
    /// falls back to raw deflate: the HTTP/2 client's.
    #[must_use]
    pub fn gzip_only() -> Self {
        let mut inflate = Self::new(true);
        inflate.first_chunk = false;
        inflate
    }

    /// Inflate one body chunk: the bytes it produced, or None on an empty
    /// input or a decompression error, as the pinned method.
    pub fn decode(&mut self, input: &[u8]) -> Option<&[u8]> {
        (!input.is_empty() && self.chunk(input)).then_some(&self.out[..])
    }

    fn chunk(&mut self, input: &[u8]) -> bool {
        let first = self.first_chunk;
        if self.stream.is_none() {
            let encoding = if self.gzip {
                ENCODING_GZIP
            } else {
                ENCODING_DEFLATE
            };
            self.stream = Stream::inflate(encoding).ok();
        }
        let Some(stream) = self.stream.as_mut() else {
            return false;
        };
        let status = run(stream, input, &mut self.out);
        self.first_chunk = false;
        // Upstream retries a first-chunk data error as raw deflate (no zlib header).
        if status == Err(Z_DATA_ERROR) && first {
            self.stream = None;
            self.stream = Stream::inflate(ENCODING_RAW).ok();
            return (self.stream.as_mut())
                .is_some_and(|raw| run(raw, input, &mut self.out).is_ok());
        }
        status.is_ok()
    }
}

/// Why [`message`] failed.
#[derive(Debug, PartialEq, Eq)]
pub enum MessageFailure {
    /// `inflateInit2() failed by %s`.
    Init(c_int),
    /// `inflate() failed, Error: %s[%d]`.
    Inflate(c_int),
}

/// `swoole::websocket::message_uncompress`: a permessage-deflate payload (RFC
/// 7692, raw deflate) inflated with sync flushes into a buffer that starts at
/// `input.len() + 1024` bytes and doubles whenever less than 4096 are free.
/// # Errors
/// `inflateInit2` or `inflate` failed; the status says which.
pub fn message(mut input: &[u8]) -> Result<Vec<u8>, MessageFailure> {
    /// `SW_BUFFER_SIZE_STD / 2`.
    const HEADROOM: usize = 4096;
    let mut stream = Stream::inflate(ENCODING_RAW).map_err(MessageFailure::Init)?;
    let mut out = Vec::with_capacity(input.len() + 1024);
    loop {
        let step = stream.run_into(&mut input, &mut out, Z_SYNC_FLUSH);
        if step.status == Z_STREAM_END || (step.status == Z_OK && input.is_empty()) {
            return Ok(out);
        }
        if step.status != Z_OK {
            return Err(MessageFailure::Inflate(step.status));
        }
        if out.len() + HEADROOM >= out.capacity() {
            out.reserve_exact(2 * out.capacity() - out.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deflate(bytes: &[u8], window_bits: c_int) -> Vec<u8> {
        // Produce a reference stream with zlib's own deflate, matching window bits.
        let mut stream = Stream::deflate(6, window_bits, 8).unwrap();
        let mut out = vec![0u8; bytes.len() + 64];
        let step = stream.run(&mut &bytes[..], &mut out, swoole_rs_zlib::Z_FINISH);
        assert_eq!(step.status, Z_STREAM_END);
        out.truncate(step.written);
        out
    }

    fn inflate_all(gzip: bool, compressed: &[u8]) -> Option<Vec<u8>> {
        Inflate::new(gzip).decode(compressed).map(<[u8]>::to_vec)
    }

    #[test]
    fn round_trips_gzip_and_deflate() {
        let payload = b"the quick brown fox jumps over the lazy dog, repeatedly. ".repeat(40);
        // gzip: window bits 31 (0x1f); zlib deflate: 15 (0x0f).
        assert_eq!(
            inflate_all(true, &deflate(&payload, 31)).as_deref(),
            Some(&payload[..])
        );
        assert_eq!(
            inflate_all(false, &deflate(&payload, 15)).as_deref(),
            Some(&payload[..])
        );
    }

    #[test]
    fn deflate_decoder_retries_raw_stream() {
        // A raw deflate stream (window bits -15) has no zlib header; the deflate
        // decoder must hit Z_DATA_ERROR on the first chunk and retry as raw.
        let payload = b"raw deflate body without a zlib header".to_vec();
        assert_eq!(
            inflate_all(false, &deflate(&payload, -15)).as_deref(),
            Some(&payload[..])
        );
    }

    #[test]
    fn chunked_input_reassembles() {
        let payload = b"chunk boundaries must not corrupt the inflate stream".repeat(8);
        let compressed = deflate(&payload, 31);
        let mut inflate = Inflate::new(true);
        let mut got = Vec::new();
        for piece in compressed.chunks(7) {
            got.extend_from_slice(inflate.decode(piece).expect("chunk inflates"));
        }
        assert_eq!(got, payload);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert_eq!(inflate_all(true, b"not a gzip stream at all"), None);
        // Empty input is a compression error, matching the pinned in_len == 0 guard.
        assert!(inflate_all(true, b"").is_none());
    }
}
