//! HTTP response body compression with zlib, as `swoole::http::Context::compress`
//! did it: one `deflate(Z_FINISH)` of the whole body into a buffer sized by
//! upstream's bound, with the caller's allocator (PHP's request allocator in
//! the server).
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

use libz_sys::{
    alloc_func, deflate, deflateBound, deflateEnd, deflateInit2_, free_func, uInt, uLong, z_stream,
    zlibVersion,
};
use std::ffi::c_int;

/// `deflateInit2`'s window bits for gzip and zlib (`SW_ZLIB_ENCODING_*`).
pub const GZIP: c_int = 0x1f;
pub const DEFLATE: c_int = 0x0f;

const Z_OK: c_int = 0;
const Z_STREAM_END: c_int = 1;
const Z_FINISH: c_int = 4;
const Z_DEFLATED: c_int = 8;
const Z_DEFAULT_STRATEGY: c_int = 0;
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

/// Deflate `data` into `out` at `level` with `window_bits` (`GZIP` or
/// `DEFLATE`); the compressed length.
/// # Errors
/// `deflateInit2` or `deflate` did not succeed; the status says which.
pub fn compress(
    data: &[u8],
    out: &mut [u8],
    window_bits: c_int,
    level: c_int,
    zalloc: alloc_func,
    zfree: free_func,
) -> Result<usize, Failure> {
    let mut storage = std::mem::MaybeUninit::<z_stream>::zeroed();
    let stream = storage.as_mut_ptr();
    // SAFETY: zeroed z_stream storage; the allocator fields must not stay null.
    unsafe { (*stream).zalloc = zalloc };
    // SAFETY: as above.
    unsafe { (*stream).zfree = zfree };
    let size = c_int::try_from(size_of::<z_stream>()).unwrap_or(c_int::MAX);
    // SAFETY: zlibVersion returns a static version string.
    let version = unsafe { zlibVersion() };
    // SAFETY: a zeroed stream with its allocator, the running version and its size.
    let status = unsafe {
        deflateInit2_(
            stream,
            level,
            Z_DEFLATED,
            window_bits,
            MEM_LEVEL,
            Z_DEFAULT_STRATEGY,
            version,
            size,
        )
    };
    if status != Z_OK {
        return Err(Failure::Init(status));
    }
    // SAFETY: an initialised stream; the input outlives the call.
    unsafe { (*stream).next_in = data.as_ptr().cast_mut() };
    // SAFETY: as above.
    unsafe { (*stream).avail_in = uInt::try_from(data.len()).unwrap_or(uInt::MAX) };
    // SAFETY: as above; the output outlives the call.
    unsafe { (*stream).next_out = out.as_mut_ptr() };
    // SAFETY: as above.
    unsafe { (*stream).avail_out = uInt::try_from(out.len()).unwrap_or(uInt::MAX) };
    // SAFETY: an initialised stream over live buffers.
    let status = unsafe { deflate(stream, Z_FINISH) };
    // SAFETY: as above; ends it once.
    unsafe { deflateEnd(stream) };
    if status != Z_STREAM_END {
        return Err(Failure::Deflate(status));
    }
    // SAFETY: the stream deflateEnd left readable.
    let total = unsafe { (*stream).total_out };
    Ok(usize::try_from(total).unwrap_or(0))
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
    data: &[u8],
    level: c_int,
    zalloc: alloc_func,
    zfree: free_func,
) -> Result<Vec<u8>, MessageFailure> {
    const Z_NO_FLUSH: c_int = 0;
    const Z_SYNC_FLUSH: c_int = 2;
    const Z_BUF_ERROR: c_int = -5;
    /// `SW_ZLIB_ENCODING_RAW`.
    const RAW: c_int = -0xf;
    let level = match level {
        0 => -1,
        level => level.min(9),
    };
    let mut storage = std::mem::MaybeUninit::<z_stream>::zeroed();
    let stream = storage.as_mut_ptr();
    // SAFETY: zeroed z_stream storage; the allocator fields must not stay null.
    unsafe { (*stream).zalloc = zalloc };
    // SAFETY: as above.
    unsafe { (*stream).zfree = zfree };
    let stream_size = c_int::try_from(size_of::<z_stream>()).unwrap_or(c_int::MAX);
    // SAFETY: zlibVersion returns a static version string.
    let version = unsafe { zlibVersion() };
    // SAFETY: a zeroed stream with its allocator, the running version and its size.
    let status = unsafe {
        deflateInit2_(
            stream,
            level,
            Z_DEFLATED,
            RAW,
            MEM_LEVEL,
            Z_DEFAULT_STRATEGY,
            version,
            stream_size,
        )
    };
    if status != Z_OK {
        return Err(MessageFailure::Init(status));
    }
    // SAFETY: an initialised stream; the input outlives every deflate below.
    unsafe { (*stream).next_in = data.as_ptr().cast_mut() };
    // SAFETY: as above.
    unsafe { (*stream).avail_in = uInt::try_from(data.len()).unwrap_or(uInt::MAX) };
    // SAFETY: an initialised stream.
    let bound = unsafe { deflateBound(stream, uLong::try_from(data.len()).unwrap_or(uLong::MAX)) };
    let mut max = usize::try_from(bound).unwrap_or(usize::MAX);
    let mut out = Vec::new();
    let mut length = 0;
    let mut written = 0;
    let result = loop {
        // SAFETY: an initialised stream.
        if unsafe { (*stream).avail_out } == 0 {
            // Z_SYNC_FLUSH can produce more than deflateBound(): another
            // window, then 4096 bytes at a time.
            if written + max > out.len() {
                out.resize(written + max, 0);
            }
            length = written + max;
            // SAFETY: as above; `out` holds `written + max` bytes.
            unsafe { (*stream).avail_out = uInt::try_from(max).unwrap_or(uInt::MAX) };
            // SAFETY: `written <= out.len()`, inside or one past `out`.
            let next = unsafe { out.as_mut_ptr().add(written) };
            // SAFETY: an initialised stream.
            unsafe { (*stream).next_out = next };
            max = 4096;
        }
        let remaining = length - written;
        // SAFETY: an initialised stream.
        let flush = if unsafe { (*stream).avail_in } == 0 {
            Z_SYNC_FLUSH
        } else {
            Z_NO_FLUSH
        };
        // SAFETY: an initialised stream over live buffers.
        let result = unsafe { deflate(stream, flush) };
        // SAFETY: as above.
        written += remaining - usize::try_from(unsafe { (*stream).avail_out }).unwrap_or(0);
        if result != Z_OK {
            break result;
        }
    };
    // SAFETY: an initialised stream, ended once.
    unsafe { deflateEnd(stream) };
    if result != Z_BUF_ERROR || written < 4 {
        return Err(MessageFailure::Deflate);
    }
    out.truncate(written - 4);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use libz_sys::{inflate, inflateEnd, inflateInit2_, voidpf};

    unsafe extern "C" fn alloc(_: voidpf, items: uInt, size: uInt) -> voidpf {
        unsafe { libc_calloc(items.try_into().unwrap(), size.try_into().unwrap()) }
    }
    unsafe extern "C" fn free(_: voidpf, address: voidpf) {
        unsafe { libc_free(address) }
    }
    unsafe extern "C" {
        #[link_name = "calloc"]
        fn libc_calloc(items: usize, size: usize) -> voidpf;
        #[link_name = "free"]
        fn libc_free(address: voidpf);
    }

    fn round_trip(window_bits: c_int) {
        let data = b"hello hello hello hello swoole ".repeat(40);
        let mut out = vec![0; bound(data.len())];
        let length = compress(&data, &mut out, window_bits, 1, alloc, free).unwrap();
        assert!(length < data.len(), "compressible input shrinks");
        if window_bits == GZIP {
            assert_eq!(&out[..2], &[0x1f, 0x8b], "gzip magic");
        }
        let mut stream = std::mem::MaybeUninit::<z_stream>::zeroed();
        let ptr = stream.as_mut_ptr();
        let mut plain = vec![0u8; data.len()];
        unsafe {
            (*ptr).zalloc = alloc;
            (*ptr).zfree = free;
            let size = c_int::try_from(size_of::<z_stream>()).unwrap();
            assert_eq!(
                inflateInit2_(ptr, window_bits, zlibVersion(), size),
                Z_OK,
                "init"
            );
            (*ptr).next_in = out.as_mut_ptr();
            (*ptr).avail_in = uInt::try_from(length).unwrap();
            (*ptr).next_out = plain.as_mut_ptr();
            (*ptr).avail_out = uInt::try_from(plain.len()).unwrap();
            assert_eq!(inflate(ptr, Z_FINISH), Z_STREAM_END, "inflate");
            inflateEnd(ptr);
        }
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
            let compressed = message(&data, 0, alloc, free).unwrap();
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
        let compressed = message(b"Hello", -1, alloc, free).unwrap();
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
        let mut out = vec![0; 4];
        assert!(
            matches!(
                compress(&data, &mut out, GZIP, 1, alloc, free),
                Err(Failure::Deflate(_))
            ),
            "Z_BUF_ERROR or Z_OK, not Z_STREAM_END"
        );
    }
}
