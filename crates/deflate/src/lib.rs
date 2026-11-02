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
    alloc_func, deflate, deflateEnd, deflateInit2_, free_func, uInt, z_stream, zlibVersion,
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
