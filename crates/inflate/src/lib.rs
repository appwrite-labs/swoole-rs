//! HTTP body codecs over the system libraries the PHP build links: response
//! decompression (gzip/deflate over a per-`Client` zlib stream, with the raw-deflate
//! retry; Brotli and zstd streams in [`brotli`] and [`zstd`]), their compression for
//! the server, and permessage-deflate inflation.

#![forbid(unsafe_code)]

pub mod zstd;

pub use swoole_rs_brotli::{self as brotli, DecodeError};

use std::ffi::c_int;
use swoole_rs_corebase::buffer::BoundedBytes;

pub use swoole_rs_zlib::error;
use swoole_rs_zlib::{
    ENCODING_DEFLATE, ENCODING_GZIP, ENCODING_RAW, Stream, Z_DATA_ERROR, Z_OK, Z_STREAM_END,
    Z_SYNC_FLUSH,
};

/// Default decoded HTTP response body limit, covering the largest upstream compression PHPT.
pub const DEFAULT_BODY_LIMIT: usize = 32 * 1024 * 1024;

/// A per-response gzip/deflate inflate stream. `out` holds the bytes produced by
/// the most recent chunk; the caller copies them into its body buffer.
pub struct Inflate {
    stream: Option<Stream>,
    first_chunk: bool,
    gzip: bool,
    limit: usize,
    remaining: usize,
    out: BoundedBytes,
}

impl Inflate {
    /// A gzip (or deflate) response decompressor.
    #[must_use]
    pub fn new(gzip: bool, limit: usize) -> Self {
        Self {
            stream: None,
            first_chunk: true,
            gzip,
            limit,
            remaining: limit,
            out: BoundedBytes::new(limit.saturating_add(1), 0),
        }
    }

    /// A gzip-only decompressor (`inflateInit2(MAX_WBITS + 16)`), which never
    /// falls back to raw deflate: the HTTP/2 client's.
    #[must_use]
    pub fn gzip_only(limit: usize) -> Self {
        let mut inflate = Self::new(true, limit);
        inflate.first_chunk = false;
        inflate
    }

    /// Inflate one body chunk within the response's remaining budget.
    /// # Errors
    /// Returns the configured limit on overflow or a codec error on empty or malformed input.
    pub fn decode(&mut self, input: &[u8]) -> Result<&[u8], DecodeError> {
        let codec_error =
            || DecodeError::Codec("HttpClient::decompress_response failed".to_owned());
        if input.is_empty() {
            return Err(codec_error());
        }
        if self.stream.is_none() {
            let encoding = if self.gzip {
                ENCODING_GZIP
            } else {
                ENCODING_DEFLATE
            };
            self.stream = Stream::inflate(encoding).ok();
        }
        let stream = self.stream.as_mut().ok_or_else(codec_error)?;
        let mut result = run(stream, input, &mut self.out, self.remaining);
        // Upstream retries a first-chunk data error as raw deflate (no zlib header).
        if std::mem::take(&mut self.first_chunk)
            && result == Err(MessageFailure::Inflate(Z_DATA_ERROR))
        {
            self.stream = Stream::inflate(ENCODING_RAW).ok();
            result = run(
                self.stream.as_mut().ok_or_else(codec_error)?,
                input,
                &mut self.out,
                self.remaining,
            );
        }
        let filled = result.map_err(|err| match err {
            MessageFailure::Limit(_) => DecodeError::Limit(self.limit),
            _ => codec_error(),
        })?;
        self.remaining -= filled;
        Ok(self.out.as_ref().split_at(filled).0)
    }
}

/// Why [`message`] failed.
#[derive(Debug, PartialEq, Eq)]
pub enum MessageFailure {
    /// `inflateInit2() failed by %s`.
    Init(c_int),
    /// `inflate() failed, Error: %s[%d]`.
    Inflate(c_int),
    /// The decoded message exceeded this byte limit.
    Limit(usize),
}

/// Inflate a permessage-deflate payload up to `max` decoded bytes.
///
/// # Errors
/// Returns the configured limit on overflow or the zlib status on a codec error.
pub fn message(input: &[u8], max: usize) -> Result<Vec<u8>, MessageFailure> {
    let mut stream = Stream::inflate(ENCODING_RAW).map_err(MessageFailure::Init)?;
    let mut out = BoundedBytes::new(max.saturating_add(1), 0);
    let filled = run(&mut stream, input, &mut out, max)?;
    let mut out = out.into_vec();
    out.truncate(filled);
    Ok(out)
}

fn run(
    stream: &mut Stream,
    mut input: &[u8],
    out: &mut BoundedBytes,
    max: usize,
) -> Result<usize, MessageFailure> {
    let ceiling = max.saturating_add(1);
    let mut end = match out.as_ref().len() {
        0 => input.len().saturating_add(1024),
        retained => retained,
    }
    .min(ceiling);
    let mut filled = 0;
    loop {
        let prefix = out.writable_prefix(end).map_err(MessageFailure::Limit)?;
        let room = prefix.split_at_mut(filled).1;
        let room_len = room.len().min(u32::MAX as usize);
        let step = stream.run_into_slice(&mut input, room, Z_SYNC_FLUSH);
        filled += step.written;
        if filled > max {
            return Err(MessageFailure::Limit(max));
        }
        if step.status == Z_STREAM_END
            || (input.is_empty()
                && (step.status == Z_OK && step.written < room_len
                    || step.status == swoole_rs_zlib::Z_BUF_ERROR && filled != 0))
        {
            return Ok(filled);
        }
        if step.status != Z_OK {
            return Err(MessageFailure::Inflate(step.status));
        }
        if filled == end {
            end = end.saturating_mul(2).min(ceiling);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deflate(bytes: &[u8], window_bits: c_int) -> Vec<u8> {
        // Produce a reference stream with zlib's own deflate, matching window bits.
        let mut stream = Stream::deflate(6, window_bits, 8).unwrap();
        let mut out = Vec::with_capacity(bytes.len() + 64);
        let step = stream.run_into(&mut &bytes[..], &mut out, swoole_rs_zlib::Z_FINISH);
        assert_eq!(step.status, Z_STREAM_END);
        out
    }

    fn inflate_all(gzip: bool, compressed: &[u8]) -> Option<Vec<u8>> {
        Inflate::new(gzip, DEFAULT_BODY_LIMIT)
            .decode(compressed)
            .ok()
            .map(<[u8]>::to_vec)
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
        let mut inflate = Inflate::new(true, payload.len());
        let mut bounded = Inflate::new(true, payload.len() - 1);
        assert_eq!(
            compressed
                .chunks(7)
                .try_for_each(|part| bounded.decode(part).map(|_| ())),
            Err(DecodeError::Limit(payload.len() - 1))
        );
        let mut got = Vec::new();
        for piece in compressed.chunks(7) {
            got.extend_from_slice(inflate.decode(piece).expect("chunk inflates"));
        }
        assert_eq!(got, payload);
    }

    #[test]
    fn zero_budget_accepts_only_empty_output() {
        let mut http = Inflate::gzip_only(0);
        assert_eq!(http.decode(&deflate(b"", ENCODING_GZIP)), Ok(&[][..]));
        let mut http = Inflate::gzip_only(0);
        assert_eq!(
            http.decode(&deflate(b"x", ENCODING_GZIP)),
            Err(DecodeError::Limit(0))
        );
        assert_eq!(message(&deflate(b"", ENCODING_RAW), 0), Ok(Vec::new()));
        assert_eq!(
            message(&deflate(b"x", ENCODING_RAW), 0),
            Err(MessageFailure::Limit(0))
        );
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert_eq!(inflate_all(true, b"not a gzip stream at all"), None);
        // Empty input is a compression error, matching the pinned in_len == 0 guard.
        assert!(inflate_all(true, b"").is_none());
    }
}
