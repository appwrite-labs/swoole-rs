//! Safe streams over the system zlib (the libz the PHP build links): every call
//! the inflate and deflate crates make into C is here, so those crates, which
//! handle untrusted bytes, forbid `unsafe` (#1953).
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr::null_mut;

pub use libz_sys::{
    Z_BUF_ERROR, Z_DATA_ERROR, Z_FINISH, Z_NO_FLUSH, Z_OK, Z_STREAM_END, Z_SYNC_FLUSH,
};
use libz_sys::{
    Z_DEFAULT_STRATEGY, Z_DEFLATED, deflate, deflateEnd, deflateInit2_, inflate, inflateEnd,
    inflateInit2_, uInt, uLong, z_stream, z_streamp,
};

/// The most input or output one [`Stream::run`] hands zlib (its `uInt`).
pub const CHUNK: usize = uInt::MAX as usize;

/// A [`Stream`]'s `z_stream`. Only [`fresh`] makes one, and only zlib's own calls
/// write its `state`, which is therefore null (not yet initialised, or ended) or the
/// state an init call made for this very stream with [`c_alloc`]: what zlib's state
/// check accepts or refuses without reading anything else.
#[repr(transparent)]
struct Z(z_stream);

unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
    fn free(ptr: *mut c_void);
    fn zError(status: c_int) -> *const c_char;
    /// The running library's static version string.
    safe fn zlibVersion() -> *const c_char;
    // zlib's own declarations of the two calls that take only the stream, typed as
    // the stream it is: each checks the stream's state first and touches neither its
    // input nor its output.
    safe fn deflateReset(z: &mut Z) -> c_int;
    safe fn deflateBound(z: &mut Z, length: uLong) -> uLong;
}

extern "C" fn c_alloc(_: *mut c_void, items: uInt, size: uInt) -> *mut c_void {
    let bytes = usize::try_from(items)
        .unwrap_or(0)
        .saturating_mul(usize::try_from(size).unwrap_or(0));
    // SAFETY: malloc has no preconditions; a null return signals failure to zlib.
    unsafe { malloc(bytes) }
}

unsafe extern "C" fn c_free(_: *mut c_void, address: *mut c_void) {
    // SAFETY: `address` is null or was returned by `c_alloc`'s malloc.
    unsafe { free(address) };
}

/// What one [`Stream::run`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// zlib's status.
    pub status: c_int,
    /// Output bytes written.
    pub written: usize,
}

/// An initialised inflate or deflate stream, ended on drop. Boxed: zlib's state
/// points back at its `z_stream`, so that must not move.
pub struct Stream {
    z: Box<Z>,
    deflate: bool,
}

fn fresh() -> Z {
    Z(z_stream {
        next_in: null_mut(),
        avail_in: 0,
        total_in: 0,
        next_out: null_mut(),
        avail_out: 0,
        total_out: 0,
        msg: null_mut(),
        state: null_mut(),
        zalloc: c_alloc,
        zfree: c_free,
        opaque: null_mut(),
        data_type: 0,
        adler: 0,
        reserved: 0,
    })
}

fn size() -> c_int {
    c_int::try_from(size_of::<z_stream>()).unwrap_or(c_int::MAX)
}

impl Stream {
    /// `inflateInit2(window_bits)`, allocating with malloc.
    /// # Errors
    /// zlib's status when it is not `Z_OK`.
    pub fn inflate(window_bits: c_int) -> Result<Self, c_int> {
        let mut z = Box::new(fresh());
        // SAFETY: a fresh stream with its allocator, the running version and the struct's size.
        let status = unsafe { inflateInit2_(&raw mut z.0, window_bits, zlibVersion(), size()) };
        // Only an initialised stream becomes a `Stream`, which `drop` ends.
        (status == Z_OK)
            .then_some(Self { z, deflate: false })
            .ok_or(status)
    }

    /// `deflateInit2(level, Z_DEFLATED, window_bits, mem_level, Z_DEFAULT_STRATEGY)`.
    /// # Errors
    /// zlib's status when it is not `Z_OK`.
    pub fn deflate(level: c_int, window_bits: c_int, mem_level: c_int) -> Result<Self, c_int> {
        let mut z = Box::new(fresh());
        // SAFETY: as in `inflate`.
        let status = unsafe {
            deflateInit2_(
                &raw mut z.0,
                level,
                Z_DEFLATED,
                window_bits,
                mem_level,
                Z_DEFAULT_STRATEGY,
                zlibVersion(),
                size(),
            )
        };
        (status == Z_OK)
            .then_some(Self { z, deflate: true })
            .ok_or(status)
    }

    /// One `inflate(flush)` or `deflate(flush)` writing `out`; `input` advances
    /// past what it read.
    pub fn run(&mut self, input: &mut &[u8], out: &mut [u8], flush: c_int) -> Step {
        self.step(input, out.as_mut_ptr(), out.len(), flush)
    }

    /// [`run`](Self::run) into `out`'s spare capacity, appending what it wrote.
    pub fn run_into(&mut self, input: &mut &[u8], out: &mut Vec<u8>, flush: c_int) -> Step {
        let spare = out.spare_capacity_mut();
        let step = self.step(input, spare.as_mut_ptr().cast(), spare.len(), flush);
        // SAFETY: zlib wrote the first `written` spare bytes, so the new length is
        // initialised and within capacity.
        unsafe { out.set_len(out.len() + step.written) };
        step
    }

    /// `out` is `room` writable bytes, live for the call.
    fn step(&mut self, input: &mut &[u8], out: *mut u8, room: usize, flush: c_int) -> Step {
        let z = &mut self.z.0;
        z.next_in = input.as_ptr().cast_mut();
        z.avail_in = uInt::try_from(input.len()).unwrap_or(uInt::MAX);
        z.next_out = out;
        z.avail_out = uInt::try_from(room).unwrap_or(uInt::MAX);
        let (avail_in, avail_out) = (z.avail_in, z.avail_out);
        let call: unsafe extern "C" fn(z_streamp, c_int) -> c_int =
            if self.deflate { deflate } else { inflate };
        // SAFETY: an initialised stream over the live input, which zlib only reads,
        // and `room` writable output bytes.
        let status = unsafe { call(&raw mut self.z.0, flush) };
        let used = |before: uInt, after: uInt| usize::try_from(before - after).unwrap_or(0);
        *input = input
            .get(used(avail_in, self.z.0.avail_in)..)
            .unwrap_or_default();
        Step {
            status,
            written: used(avail_out, self.z.0.avail_out),
        }
    }

    /// `deflateReset`: the deflate stream as `deflateInit2` left it, keeping its
    /// level, window and memory. zlib's status.
    pub fn reset(&mut self) -> c_int {
        // zlib refuses an inflate stream.
        deflateReset(&mut self.z)
    }

    /// `deflateBound` for `length` input bytes.
    pub fn deflate_bound(&mut self, length: usize) -> usize {
        let length = uLong::try_from(length).unwrap_or(uLong::MAX);
        // zlib bounds conservatively for an inflate stream.
        let bound = deflateBound(&mut self.z, length);
        usize::try_from(bound).unwrap_or(usize::MAX)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let end: unsafe extern "C" fn(z_streamp) -> c_int =
            if self.deflate { deflateEnd } else { inflateEnd };
        // SAFETY: an initialised stream, ended once here.
        unsafe { end(&raw mut self.z.0) };
    }
}

/// zlib's message for a status (`zError`).
#[must_use]
pub fn error(status: c_int) -> &'static CStr {
    // zlib's statuses run from Z_VERSION_ERROR (-6) to Z_NEED_DICT (2); it has no
    // message for any other number.
    if !(-6..=2).contains(&status) {
        return c"";
    }
    // SAFETY: one of zlib's own statuses, each of which has a message.
    let message = unsafe { zError(status) };
    // SAFETY: zError returns a static, NUL-terminated string for any status.
    unsafe { CStr::from_ptr(message) }
}
