//! HTTP response body compression with zlib, as `swoole::http::Context::compress`
//! did it: one `deflate(Z_FINISH)` of the whole body into a buffer sized by
//! upstream's bound. A thread keeps its last stream and resets it for the next
//! body, so zlib's 268 KiB of state is allocated once, with malloc.
#![forbid(unsafe_code)]

use std::cell::Cell;
use std::ffi::c_int;

use swoole_rs_zlib::{
    CHUNK, ENCODING_RAW, Stream, Z_BEST_COMPRESSION, Z_BUF_ERROR, Z_DEFAULT_COMPRESSION, Z_FINISH,
    Z_NO_COMPRESSION, Z_NO_FLUSH, Z_OK, Z_STREAM_END, Z_SYNC_FLUSH,
};

/// `deflateInit2`'s window bits for gzip and zlib (`SW_ZLIB_ENCODING_*`).
pub use swoole_rs_zlib::{ENCODING_DEFLATE as DEFLATE, ENCODING_GZIP as GZIP};

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

thread_local! {
    /// The thread's last stream with its level and window bits, which the next
    /// body or message of the same kind resets instead of allocating its own.
    /// Taken out and set back, never borrowed; zlib's state is boxed inside
    /// `Stream`, so moving one keeps its pointers good.
    static KEPT: Cell<Option<(c_int, c_int, Stream)>> = const { Cell::new(None) };
}

/// A fresh deflate stream at `level` with `window_bits`: the thread's kept one
/// when it matches, reset; else a new one. Taken for the call, so a stream that
/// fails is dropped and one that ends well goes back with [`keep`].
fn stream(level: c_int, window_bits: c_int) -> Result<Stream, c_int> {
    let kept = KEPT.take().and_then(|(kept_level, kept_bits, mut stream)| {
        ((kept_level, kept_bits) == (level, window_bits) && stream.reset()).then_some(stream)
    });
    kept.map_or_else(|| Stream::deflate(level, window_bits, MEM_LEVEL), Ok)
}

fn keep(level: c_int, window_bits: c_int, stream: Stream) {
    KEPT.set(Some((level, window_bits, stream)));
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
) -> Result<Vec<u8>, Failure> {
    let mut stream = stream(level, window_bits).map_err(Failure::Init)?;
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
            Z_STREAM_END => {
                keep(level, window_bits, stream);
                return Ok(out);
            }
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
/// The first output window is `deflateBound`, as upstream uses.
/// # Errors
/// `deflateInit2` failed, or deflate did not end in `Z_BUF_ERROR` with at
/// least the 4-byte tail written.
pub fn message(mut data: &[u8], level: c_int) -> Result<Vec<u8>, MessageFailure> {
    let level = match level {
        Z_NO_COMPRESSION => Z_DEFAULT_COMPRESSION,
        level => level.min(Z_BEST_COMPRESSION),
    };
    let mut stream = stream(level, ENCODING_RAW).map_err(MessageFailure::Init)?;
    let bound = stream.deflate_bound(data.len());
    // Z_SYNC_FLUSH adds an empty stored block (at most six bytes). If the
    // first window fills during the flush, zlib may emit the marker again.
    let capacity = bound
        .checked_add(2 * 6 + 1)
        .ok_or(MessageFailure::Deflate)?;
    let mut out = Vec::with_capacity(capacity);
    let result = loop {
        let end = if out.len() < bound { bound } else { capacity };
        let room = end - out.len();
        if room == 0 {
            return Err(MessageFailure::Deflate);
        }
        let flush = if data.is_empty() {
            Z_SYNC_FLUSH
        } else {
            Z_NO_FLUSH
        };
        let step = stream.run_into_limited(&mut data, &mut out, flush, room);
        if step.status != Z_OK {
            break step.status;
        }
    };
    if result != Z_BUF_ERROR || out.len() < 4 {
        return Err(MessageFailure::Deflate);
    }
    keep(level, ENCODING_RAW, stream);
    out.truncate(out.len() - 4);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(window_bits: c_int) {
        let data = b"hello hello hello hello swoole ".repeat(40);
        let out = compress(&data, bound(data.len()), window_bits, 1).unwrap();
        let length = out.len();
        assert!(length < data.len(), "compressible input shrinks");
        if window_bits == GZIP {
            assert_eq!(&out[..2], &[0x1f, 0x8b], "gzip magic");
        }
        let mut plain = Vec::with_capacity(data.len());
        let mut stream = Stream::inflate(window_bits).expect("init");
        let step = stream.run_into(&mut &out[..length], &mut plain, Z_FINISH);
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
            let compressed = message(&data, 0).unwrap();
            assert!(
                !compressed.ends_with(&[0, 0, 0xff, 0xff]),
                "the sync flush tail is dropped"
            );
            let plain = swoole_rs_inflate::message(&compressed, data.len()).unwrap();
            assert_eq!(plain, data, "permessage-deflate round trip");
            if let Some(max) = data.len().checked_sub(1) {
                assert_eq!(
                    swoole_rs_inflate::message(&compressed, max),
                    Err(swoole_rs_inflate::MessageFailure::Limit(max))
                );
            }
        }
    }

    #[test]
    fn rfc_7692_hello_compresses_as_in_its_example() {
        // RFC 7692 section 7.2.3.1: "Hello" with no context takeover.
        let compressed = message(b"Hello", -1).unwrap();
        assert_eq!(
            compressed,
            [0xf2, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00],
            "the RFC's bytes"
        );
    }

    #[test]
    fn a_kept_stream_compresses_as_a_new_one() {
        let (body, other) = (b"swoole ".repeat(300), b"hello ".repeat(50));
        let gzip = |data: &[u8]| compress(data, bound(data.len()), GZIP, 6).unwrap();
        let first = gzip(&body);
        // The same kind again, another body between, then one after a failure
        // and one after a stream of another kind took the slot.
        assert_eq!(gzip(&body), first, "reset");
        gzip(&other);
        assert_eq!(gzip(&body), first, "after another body");
        assert!(compress(&body, 4, GZIP, 6).is_err(), "too small");
        assert_eq!(gzip(&body), first, "after a failed one");
        message(&other, -1).unwrap();
        assert_eq!(gzip(&body), first, "after a message's raw stream");
        assert_eq!(
            message(b"Hello", -1).unwrap(),
            [0xf2, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00],
            "and a message after a body"
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
            matches!(compress(&data, 4, GZIP, 1), Err(Failure::Deflate(_))),
            "Z_BUF_ERROR or Z_OK, not Z_STREAM_END"
        );
    }
}
