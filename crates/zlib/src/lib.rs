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
    Z_DEFAULT_STRATEGY, Z_DEFLATED, deflate, inflate, uInt, uLong, z_stream, z_streamp,
};

/// The most input or output one [`Stream::run`] hands zlib (its `uInt`).
pub const CHUNK: usize = uInt::MAX as usize;

/// A [`Stream`]'s `z_stream`. Only [`fresh`] makes one, and only zlib's own calls
/// write its `state`, which is therefore null (not yet initialised, or ended) or the
/// state an init call made for this very stream with [`c_alloc`]: what zlib's state
/// check accepts or refuses without reading anything else.
#[repr(transparent)]
struct Z(z_stream);

/// The running library's version string, static and NUL-terminated, which an init
/// call compares with its own. Only `zlibVersion` makes one.
#[repr(transparent)]
struct Version(*const c_char);

unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
    fn free(ptr: *mut c_void);
    /// The running library's static version string.
    safe fn zlibVersion() -> Version;
    // zlib's own declarations of the calls that take only the stream, typed as the
    // stream it is, and plain values. The init calls refuse a version, struct size
    // or setting they don't accept and otherwise allocate the stream's state with
    // its allocator; the others check the stream's state first (the end calls then
    // free it with the stream's allocator and null it). None touches the stream's
    // input or output.
    safe fn inflateInit2_(z: &mut Z, window_bits: c_int, version: Version, size: c_int) -> c_int;
    safe fn deflateInit2_(
        z: &mut Z,
        level: c_int,
        method: c_int,
        window_bits: c_int,
        mem_level: c_int,
        strategy: c_int,
        version: Version,
        size: c_int,
    ) -> c_int;
    safe fn deflateReset(z: &mut Z) -> c_int;
    safe fn deflateBound(z: &mut Z, length: uLong) -> uLong;
    safe fn deflateEnd(z: &mut Z) -> c_int;
    safe fn inflateEnd(z: &mut Z) -> c_int;
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
        let status = inflateInit2_(&mut z, window_bits, zlibVersion(), size());
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
        let status = deflateInit2_(
            &mut z,
            level,
            Z_DEFLATED,
            window_bits,
            mem_level,
            Z_DEFAULT_STRATEGY,
            zlibVersion(),
            size(),
        );
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
        if self.deflate {
            deflateEnd(&mut self.z);
        } else {
            inflateEnd(&mut self.z);
        }
    }
}

/// zlib's message for a status (`zError`): its `z_errmsg` table, indexed from
/// `Z_NEED_DICT` (2) down to `Z_VERSION_ERROR` (-6); empty for any other number.
#[must_use]
pub fn error(status: c_int) -> &'static CStr {
    const MESSAGES: [&CStr; 9] = [
        c"need dictionary",
        c"stream end",
        c"",
        c"file error",
        c"stream error",
        c"data error",
        c"insufficient memory",
        c"buffer error",
        c"incompatible version",
    ];
    usize::try_from(2 - i64::from(status))
        .ok()
        .and_then(|index| MESSAGES.get(index))
        .copied()
        .unwrap_or(c"")
}
