//! HTTP response body compression with zlib, as `swoole::http::Context::compress`
//! did it: one `deflate(Z_FINISH)` of the whole body into a buffer sized by
//! upstream's bound, with the caller's allocator (PHP's request allocator in
//! the server).
#![forbid(unsafe_code)]

use std::ffi::c_int;

pub use swoole_rs_zlib::Allocator;
use swoole_rs_zlib::{
    CHUNK, Stream, Z_BUF_ERROR, Z_FINISH, Z_NO_FLUSH, Z_OK, Z_STREAM_END, Z_SYNC_FLUSH,
};

/// `deflateInit2`'s window bits for gzip and zlib (`SW_ZLIB_ENCODING_*`).
pub const GZIP: c_int = 0x1f;
pub const DEFLATE: c_int = 0x0f;

/// `SW_ZLIB_DEF_MEM_LEVEL` (zlib's `MAX_MEM_LEVEL` is 9).
const MEM_LEVEL: c_int = 8;

/// The zlib status a failed step returned.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    Init(c_int),
    Deflate(c_int),
}

/// Upstream's output size for `length` input bytes:
/// `(size_t)((double) length * 1.015) + 10 + 8 + 4 + 1`.
#[must_use]
pub fn bound(length: usize) -> usize {
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream's double arithmetic, byte for byte"
    )]
    let scaled = (length as f64 * 1.015) as usize;
    scaled + 10 + 8 + 4 + 1
}

/// Deflate `data` at `level` with `window_bits` (`GZIP` or `DEFLATE`) into at
/// most `capacity` bytes ([`bound`] for upstream's), which zlib writes without
/// their being zeroed first.
/// # Errors
/// `deflateInit2` or `deflate` did not succeed; the status says which.
pub fn compress(
    data: &[u8],
    capacity: usize,
    window_bits: c_int,
    level: c_int,
    allocator: Allocator,
) -> Result<Vec<u8>, Failure> {
    let mut stream =
        Stream::deflate(level, window_bits, MEM_LEVEL, allocator).map_err(Failure::Init)?;
    let (mut data, mut out) = (data, Vec::with_capacity(capacity));
    loop {
        // One Z_FINISH over the whole body, as upstream; a body past zlib's
        // `uInt` goes in with Z_NO_FLUSH first rather than being cut off.
        let flush = if data.len() <= CHUNK {
            Z_FINISH
        } else {
            Z_NO_FLUSH
        };
        match stream.run_into(&mut data, &mut out, flush).status {
            Z_STREAM_END => return Ok(out),
            Z_OK if flush == Z_NO_FLUSH => {}
            status => return Err(Failure::Deflate(status)),
        }
    }
}

/// Why [`message`] failed.
#[derive(Debug, PartialEq, Eq)]
pub enum MessageFailure {
    /// `deflateInit2() failed, Error: [status]`.
    Init(c_int),
    /// `Failed to compress outgoing frame`.
    Deflate,
}

/// `swoole::websocket::message_compress`: a permessage-deflate payload (RFC
/// 7692), raw deflate with a sync flush and its `00 00 ff ff` tail dropped.
/// Upstream's level clamp: `Z_NO_COMPRESSION` means the default, above 9 is 9.
/// Output space is handed to zlib as upstream did: `deflateBound` bytes, then
/// 4096 at a time.
/// # Errors
/// `deflateInit2` failed, or deflate did not end in `Z_BUF_ERROR` with at
/// least the 4-byte tail written.
pub fn message(
    mut data: &[u8],
    level: c_int,
    allocator: Allocator,
) -> Result<Vec<u8>, MessageFailure> {
    /// `SW_ZLIB_ENCODING_RAW`.
    const RAW: c_int = -0xf;
    let level = match level {
        0 => -1,
        level => level.min(9),
    };
    let mut stream =
        Stream::deflate(level, RAW, MEM_LEVEL, allocator).map_err(MessageFailure::Init)?;
    let mut max = stream.deflate_bound(data.len());
    let mut out = Vec::new();
    let result = loop {
        if out.len() == out.capacity() {
            // Z_SYNC_FLUSH can produce more than deflateBound(): another
            // window, then 4096 bytes at a time.
            out.reserve_exact(max);
            max = 4096;
        }
        let flush = if data.is_empty() {
            Z_SYNC_FLUSH
        } else {
            Z_NO_FLUSH
        };
        let step = stream.run_into(&mut data, &mut out, flush);
        if step.status != Z_OK {
            break step.status;
        }
    };
    if result != Z_BUF_ERROR || out.len() < 4 {
        return Err(MessageFailure::Deflate);
    }
    out.truncate(out.len() - 4);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MALLOC: Allocator = Allocator::MALLOC;

    fn round_trip(window_bits: c_int) {
        let data = b"hello hello hello hello swoole ".repeat(40);
        let out = compress(&data, bound(data.len()), window_bits, 1, MALLOC).unwrap();
        let length = out.len();
        assert!(length < data.len(), "compressible input shrinks");
        if window_bits == GZIP {
            assert_eq!(&out[..2], &[0x1f, 0x8b], "gzip magic");
        }
        let mut plain = vec![0u8; data.len()];
        let mut stream = Stream::inflate(window_bits).expect("init");
        let step = stream.run(&mut &out[..length], &mut plain, Z_FINISH);
        assert_eq!(step.status, Z_STREAM_END, "inflate");
        assert_eq!(plain, data, "round trip");
    }

    #[test]
    fn gzip_and_deflate_round_trip() {
        round_trip(GZIP);
        round_trip(DEFLATE);
    }

    #[test]
    fn websocket_messages_round_trip() {
        for data in [
            b"".to_vec(),
            b"Hello".to_vec(),
            b"swoole websocket ".repeat(5000),
        ] {
            let compressed = message(&data, 0, MALLOC).unwrap();
            assert!(
                !compressed.ends_with(&[0, 0, 0xff, 0xff]),
                "the sync flush tail is dropped"
            );
            let plain = swoole_rs_inflate::message(&compressed).unwrap();
            assert_eq!(plain, data, "permessage-deflate round trip");
        }
    }

    #[test]
    fn rfc_7692_hello_compresses_as_in_its_example() {
        // RFC 7692 section 7.2.3.1: "Hello" with no context takeover.
        let compressed = message(b"Hello", -1, MALLOC).unwrap();
        assert_eq!(
            compressed,
            [0xf2, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00],
            "the RFC's bytes"
        );
    }

    #[test]
    fn bound_is_upstreams() {
        assert_eq!(bound(0), 23, "empty");
        assert_eq!(bound(1000), 1014 + 23, "1000.0 * 1.015 rounds down to 1014");
    }

    #[test]
    fn a_full_buffer_fails_like_upstream() {
        let data = vec![7u8; 1000];
        assert!(
            matches!(
                compress(&data, 4, GZIP, 1, MALLOC),
                Err(Failure::Deflate(_))
            ),
            "Z_BUF_ERROR or Z_OK, not Z_STREAM_END"
        );
    }
}
