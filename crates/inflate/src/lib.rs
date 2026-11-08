//! HTTP body codecs over the system libraries the PHP build links: response
//! decompression (gzip/deflate over a per-`Client` zlib stream, with the raw-deflate
//! retry; Brotli and zstd streams in [`brotli`] and [`zstd`]), their compression for
//! the server, and permessage-deflate inflation.

#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

pub mod brotli;
pub mod zstd;

use std::ffi::c_int;

use libz_sys::{inflate, inflateEnd, inflateInit2_, uInt, z_stream, zlibVersion};

// swoole's SW_ZLIB_ENCODING_* window-bits (ext-src/php_swoole_http.h).
const ENCODING_GZIP: c_int = 0x1f;
const ENCODING_DEFLATE: c_int = 0x0f;
const ENCODING_RAW: c_int = -0xf;
// zlib return and flush codes we branch on.
const Z_OK: c_int = 0;
const Z_STREAM_END: c_int = 1;
const Z_DATA_ERROR: c_int = -3;
const Z_SYNC_FLUSH: c_int = 2;

const OUT_GROW: usize = 16384;

unsafe extern "C" {
    fn malloc(size: usize) -> *mut std::ffi::c_void;
    fn free(ptr: *mut std::ffi::c_void);
    fn zError(status: c_int) -> *const std::ffi::c_char;
}

// zlib's `zalloc`/`zfree` are non-nullable in libz-sys, so a zeroed z_stream is
// UB; supply the C allocator (which, unlike Rust's, frees without the layout).
unsafe extern "C" fn zlib_alloc(
    _opaque: *mut std::ffi::c_void,
    items: libz_sys::uInt,
    size: libz_sys::uInt,
) -> *mut std::ffi::c_void {
    let bytes = usize::try_from(items)
        .unwrap_or(0)
        .saturating_mul(usize::try_from(size).unwrap_or(0));
    // SAFETY: malloc has no preconditions; a null return signals failure to zlib.
    unsafe { malloc(bytes) }
}
unsafe extern "C" fn zlib_free(_opaque: *mut std::ffi::c_void, address: *mut std::ffi::c_void) {
    // SAFETY: `address` is null or was returned by `zlib_alloc`'s malloc.
    unsafe { free(address) };
}

/// A zeroed stream with the allocator pair set — the valid pre-`inflateInit2` state.
fn fresh_stream() -> z_stream {
    let mut uninit = std::mem::MaybeUninit::<z_stream>::zeroed();
    let ptr = uninit.as_mut_ptr();
    // SAFETY: `ptr` is writable, zeroed z_stream storage; set the allocator field
    // (its zeroed null value would be an invalid fn pointer).
    unsafe { (*ptr).zalloc = zlib_alloc };
    // SAFETY: same storage; set the deallocator field.
    unsafe { (*ptr).zfree = zlib_free };
    // SAFETY: every field is now valid — the two fn pointers are set and the rest
    // are raw pointers/integers for which zero is valid.
    unsafe { uninit.assume_init() }
}

/// A per-response gzip/deflate inflate stream. `out` holds the bytes produced by
/// the most recent chunk; the caller copies them into its body buffer.
pub struct Inflate {
    stream: z_stream,
    active: bool,
    first_chunk: bool,
    gzip: bool,
    out: Vec<u8>,
}

impl Inflate {
    fn init(&mut self, encoding: c_int) -> bool {
        self.stream = fresh_stream();
        let size = c_int::try_from(size_of::<z_stream>()).unwrap_or(c_int::MAX);
        // SAFETY: zlibVersion returns a static NUL-terminated version string.
        let version = unsafe { zlibVersion() };
        // SAFETY: a fresh stream with the running zlib version and its own size.
        let status = unsafe { inflateInit2_(&raw mut self.stream, encoding, version, size) };
        self.active = status == Z_OK;
        self.active
    }

    /// Inflate all of `input` into `self.out`. Returns 0 on success, else the zlib
    /// error status (negative). Mirrors the pinned `Z_SYNC_FLUSH` loop.
    fn run(&mut self, input: &[u8]) -> c_int {
        self.out.clear();
        self.stream.next_in = input.as_ptr().cast_mut();
        self.stream.avail_in = u32::try_from(input.len()).unwrap_or(u32::MAX);
        loop {
            if self.out.len() == self.out.capacity() {
                self.out.reserve(OUT_GROW);
            }
            let spare = self.out.capacity() - self.out.len();
            // SAFETY: `len() <= capacity`, so this one-past stays within the allocation.
            self.stream.next_out = unsafe { self.out.as_mut_ptr().add(self.out.len()) };
            self.stream.avail_out = u32::try_from(spare).unwrap_or(u32::MAX);
            let before = self.stream.avail_out;
            // SAFETY: the stream is initialized; next_in/next_out denote the readable
            // input and the writable spare capacity set just above.
            let status = unsafe { inflate(&raw mut self.stream, Z_SYNC_FLUSH) };
            let produced =
                usize::try_from(before.saturating_sub(self.stream.avail_out)).unwrap_or(0);
            // SAFETY: zlib wrote `produced` bytes into the spare capacity; the new
            // length stays within capacity (`produced <= spare`).
            unsafe { self.out.set_len(self.out.len() + produced) };
            if status == Z_STREAM_END || (status == Z_OK && self.stream.avail_in == 0) {
                return 0;
            }
            if status != Z_OK {
                return status;
            }
        }
    }

    fn chunk(&mut self, input: &[u8]) -> bool {
        let first = self.first_chunk;
        if !self.active
            && !self.init(if self.gzip {
                ENCODING_GZIP
            } else {
                ENCODING_DEFLATE
            })
        {
            return false;
        }
        let status = self.run(input);
        self.first_chunk = false;
        if status == 0 {
            return true;
        }
        // Upstream retries a first-chunk data error as raw deflate (no zlib header).
        if status == Z_DATA_ERROR && first {
            // SAFETY: the stream was initialized by `init` above.
            unsafe { inflateEnd(&raw mut self.stream) };
            self.active = false;
            return self.init(ENCODING_RAW) && self.run(input) == 0;
        }
        false
    }
}

impl Inflate {
    /// A gzip (or deflate) response decompressor.
    #[must_use]
    pub fn new(gzip: bool) -> Self {
        Self {
            stream: fresh_stream(),
            active: false,
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
}

impl Drop for Inflate {
    fn drop(&mut self) {
        if self.active {
            // SAFETY: an initialized stream is ended exactly once here.
            unsafe { inflateEnd(&raw mut self.stream) };
        }
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
pub fn message(input: &[u8]) -> Result<Vec<u8>, MessageFailure> {
    /// `SW_BUFFER_SIZE_STD / 2`.
    const HEADROOM: usize = 4096;
    let mut stream = fresh_stream();
    let size = c_int::try_from(size_of::<z_stream>()).unwrap_or(c_int::MAX);
    // SAFETY: zlibVersion returns a static version string.
    let version = unsafe { zlibVersion() };
    // SAFETY: a fresh stream, the running zlib's version and the struct size.
    let status = unsafe { inflateInit2_(&raw mut stream, ENCODING_RAW, version, size) };
    if status != Z_OK {
        return Err(MessageFailure::Init(status));
    }
    stream.next_in = input.as_ptr().cast_mut();
    stream.avail_in = uInt::try_from(input.len()).unwrap_or(uInt::MAX);
    let mut out = vec![0_u8; input.len() + 1024];
    let mut length = 0;
    let status = loop {
        stream.avail_out = uInt::try_from(out.len() - length).unwrap_or(uInt::MAX);
        // SAFETY: `length <= out.len()`, so this is inside or one past `out`.
        stream.next_out = unsafe { out.as_mut_ptr().add(length) };
        // SAFETY: an initialized stream over the live input and output.
        let status = unsafe { inflate(&raw mut stream, Z_SYNC_FLUSH) };
        if status >= 0 {
            length = usize::try_from(stream.total_out).unwrap_or(usize::MAX);
        }
        if status == Z_STREAM_END || (status == Z_OK && stream.avail_in == 0) {
            break Z_OK;
        }
        if status != Z_OK {
            break status;
        }
        if length + HEADROOM >= out.len() {
            out.resize(out.len() * 2, 0);
        }
    };
    // SAFETY: an initialized stream, ended once.
    unsafe { inflateEnd(&raw mut stream) };
    if status != Z_OK {
        return Err(MessageFailure::Inflate(status));
    }
    out.truncate(length);
    Ok(out)
}

/// zlib's message for a status (`zError`).
#[must_use]
pub fn error(status: c_int) -> &'static std::ffi::CStr {
    // SAFETY: zError has no preconditions.
    let message = unsafe { zError(status) };
    // SAFETY: zError returns a static, NUL-terminated string for any status.
    unsafe { std::ffi::CStr::from_ptr(message) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deflate(bytes: &[u8], window_bits: c_int) -> Vec<u8> {
        // Produce a reference stream with zlib's own deflate, matching window bits.
        let mut stream: z_stream = fresh_stream();
        let size = c_int::try_from(size_of::<z_stream>()).unwrap_or(c_int::MAX);
        let status = unsafe {
            libz_sys::deflateInit2_(
                &raw mut stream,
                6,
                libz_sys::Z_DEFLATED,
                window_bits,
                8,
                libz_sys::Z_DEFAULT_STRATEGY,
                zlibVersion(),
                size,
            )
        };
        assert_eq!(status, Z_OK);
        let mut out = vec![0u8; bytes.len() + 64];
        stream.next_in = bytes.as_ptr().cast_mut();
        stream.avail_in = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        stream.next_out = out.as_mut_ptr();
        stream.avail_out = u32::try_from(out.len()).unwrap_or(u32::MAX);
        let done = unsafe { libz_sys::deflate(&raw mut stream, 4) }; // Z_FINISH
        assert_eq!(done, Z_STREAM_END);
        let produced = out.len() - usize::try_from(stream.avail_out).unwrap_or(0);
        out.truncate(produced);
        unsafe { libz_sys::deflateEnd(&raw mut stream) };
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
