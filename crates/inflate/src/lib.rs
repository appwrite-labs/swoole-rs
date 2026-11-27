//! HTTP body codecs over the system libraries the PHP build links: response
//! decompression (gzip/deflate over a per-`Client` zlib stream, with the raw-deflate
//! header detection; Brotli and zstd streams in [`brotli`] and [`zstd`]), their compression for
//! the server, and permessage-deflate inflation.

#![forbid(unsafe_code)]

pub mod zstd;

pub use swoole_rs_brotli::{self as brotli, DecodeError};

use std::ffi::c_int;
use swoole_rs_corebase::buffer::BoundedBytes;

pub use swoole_rs_zlib::error;
use swoole_rs_zlib::{
    ENCODING_DEFLATE, ENCODING_GZIP, ENCODING_RAW, Stream, Z_OK, Z_STREAM_END, Z_SYNC_FLUSH,
};

/// Default decoded HTTP response body limit, covering the largest upstream compression PHPT.
pub const DEFAULT_BODY_LIMIT: usize = 32 * 1024 * 1024;

/// A per-response gzip/deflate inflate stream. `out` holds the bytes produced by
/// the most recent chunk; the caller copies them into its body buffer.
pub struct Inflate {
    stream: Option<Stream>,
    header_byte: Option<u8>,
    finished: bool,
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
            header_byte: None,
            finished: false,
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
        Self::new(true, limit)
    }

    /// Whether the last member ended at a codec-confirmed boundary.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.finished
    }

    /// Inflate one body chunk within the response's remaining budget.
    /// # Errors
    /// Returns the configured limit on overflow or a codec error on empty or malformed input.
    pub fn decode(&mut self, mut input: &[u8]) -> Result<&[u8], DecodeError> {
        let codec_error =
            || DecodeError::Codec("HttpClient::decompress_response failed".to_owned());
        let Some((&initial, rest)) = input.split_first() else {
            return Err(codec_error());
        };
        let header;
        let mut prefix = &[][..];
        if !self.gzip && self.stream.is_none() {
            let (first, tail) = match self.header_byte.take() {
                Some(previous) => (previous, input),
                None => (initial, rest),
            };
            let Some((&second, rest)) = tail.split_first() else {
                self.header_byte = Some(first);
                return Ok(&[]);
            };
            header = [first, second];
            let zlib =
                first & 15 == 8 && first >> 4 <= 7 && u16::from_be_bytes(header).is_multiple_of(31);
            self.stream = Some(
                Stream::inflate(if zlib { ENCODING_DEFLATE } else { ENCODING_RAW })
                    .map_err(|_| codec_error())?,
            );
            prefix = &header;
            input = rest;
        }
        let mut filled = 0;
        for mut part in [prefix, input] {
            while !part.is_empty() {
                if self.finished && !self.gzip {
                    return Err(codec_error());
                }
                if self.stream.is_none() || self.finished {
                    self.stream = Some(Stream::inflate(ENCODING_GZIP).map_err(|_| codec_error())?);
                }
                let stream = self.stream.as_mut().ok_or_else(codec_error)?;
                (filled, self.finished) =
                    run(stream, &mut part, &mut self.out, self.remaining, filled).map_err(
                        |err| match err {
                            MessageFailure::Limit(_) => DecodeError::Limit(self.limit),
                            _ => codec_error(),
                        },
                    )?;
            }
        }
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
pub fn message(mut input: &[u8], max: usize) -> Result<Vec<u8>, MessageFailure> {
    let mut stream = Stream::inflate(ENCODING_RAW).map_err(MessageFailure::Init)?;
    let mut out = BoundedBytes::new(max.saturating_add(1), 0);
    let (filled, _) = run(&mut stream, &mut input, &mut out, max, 0)?;
    let mut out = out.into_vec();
    out.truncate(filled);
    Ok(out)
}

fn run(
    stream: &mut Stream,
    input: &mut &[u8],
    out: &mut BoundedBytes,
    max: usize,
    mut filled: usize,
) -> Result<(usize, bool), MessageFailure> {
    let ceiling = max.saturating_add(1);
    let mut end = match out.as_ref().len() {
        0 => input.len().saturating_add(1024),
        retained => retained,
    }
    .min(ceiling);
    loop {
        if filled == end {
            end = end.saturating_mul(2).min(ceiling);
        }
        let prefix = out.writable_prefix(end).map_err(MessageFailure::Limit)?;
        let room = prefix.split_at_mut(filled).1;
        let room_len = room.len().min(u32::MAX as usize);
        let step = stream.run_into_slice(input, room, Z_SYNC_FLUSH);
        filled += step.written;
        if filled > max {
            return Err(MessageFailure::Limit(max));
        }
        if step.status == Z_STREAM_END
            || (input.is_empty()
                && (step.status == Z_OK && step.written < room_len
                    || step.status == swoole_rs_zlib::Z_BUF_ERROR && filled != 0))
        {
            return Ok((filled, step.status == Z_STREAM_END));
        }
        if step.status != Z_OK {
            return Err(MessageFailure::Inflate(step.status));
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
        assert_eq!(inflate_all(true, &deflate(&payload, -15)), None);
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
    fn gzip_members_share_the_response_budget_and_require_a_complete_trailer() {
        let member = deflate(b"abc", ENCODING_GZIP);
        let members = member.repeat(2);
        let mut decoder = Inflate::new(true, 6);
        let (prefix, trailer) = members.split_at(members.len() - 1);
        assert_eq!(decoder.decode(prefix), Ok(&b"abcabc"[..]));
        assert!(!decoder.finished());
        assert_eq!(decoder.decode(trailer), Ok(&[][..]));
        assert!(decoder.finished());
        let mut limited = Inflate::new(true, 5);
        assert_eq!(limited.decode(&members), Err(DecodeError::Limit(5)));
    }

    #[test]
    fn deflate_reads_a_split_header_and_rejects_trailing_bytes() {
        for encoding in [ENCODING_RAW, ENCODING_DEFLATE] {
            let wire = deflate(b"abc", encoding);
            let mut decoder = Inflate::new(false, 3);
            let (first, rest) = wire.split_at(1);
            assert_eq!(decoder.decode(first), Ok(&[][..]));
            assert_eq!(decoder.decode(rest), Ok(&b"abc"[..]));
            assert!(decoder.finished());
            assert!(decoder.decode(b"x").is_err());
        }
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
