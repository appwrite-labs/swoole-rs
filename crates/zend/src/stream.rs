//! A PHP stream a Rust function opens through its wrapper
//! (`php_stream_open_wrapper`), reads, writes, stats and closes, as the C it replaces did. A
//! user stream wrapper runs PHP code on each call, so each is a zend-sys
//! `swr_zend_try_stream_*` shim inside `zend_try`, its bailout deferred; a
//! caller stops at [`bailout::pending`], where C's longjmp would have left.
use crate::bailout;
use crate::sys::php_stream;
use std::ffi::{CStr, c_int};
use std::marker::PhantomData;
use std::ptr::NonNull;
use swoole_rs_corebase::lent::{Count, In, Out, lend_in, lend_out};
use swoole_rs_corebase::sys::Text;

/// `REPORT_ERRORS`: the open warns when it fails.
pub const REPORT_ERRORS: c_int = 8;

/// A stream [`Stream::open`] opened, which no zval holds: dropping it closes it.
#[repr(transparent)]
pub struct Stream(NonNull<php_stream>);

/// What `php_stream_stat` says of a stream.
#[derive(Clone, Copy, Debug)]
pub struct Stat {
    pub size: i64,
    pub mode: u32,
    pub atime: i64,
    pub mtime: i64,
}

/// A [`Stream`] lent for one call.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Lent<'a>(NonNull<php_stream>, PhantomData<&'a Stream>);

unsafe extern "C" {
    /// `php_stream_open_wrapper(path, mode, options, NULL)` inside `zend_try`
    /// (nonzero for a bailout), the stream in `out`, or none.
    safe fn swr_zend_try_stream_open(
        path: Text<'_>,
        mode: Text<'_>,
        options: c_int,
        out: &mut Option<Stream>,
    ) -> c_int;
    /// `php_stream_write` of the lent bytes inside `zend_try`: what it wrote in
    /// `written`, -1 when it failed or bailed out.
    safe fn swr_zend_try_stream_write<'a>(
        stream: Lent<'_>,
        bytes: In<'a, u8>,
        count: Count<'a>,
        written: &mut isize,
    ) -> c_int;
    /// `php_stream_read` into the lent buffer inside `zend_try`: what it read
    /// in `read`, -1 when it failed or bailed out.
    safe fn swr_zend_try_stream_read<'a>(
        stream: Lent<'_>,
        buffer: Out<'a, u8>,
        count: Count<'a>,
        read: &mut isize,
    ) -> c_int;
    /// `php_stream_stat` inside `zend_try`: the size, mode and times in
    /// theirs, or `failed`.
    safe fn swr_zend_try_stream_stat(
        stream: Lent<'_>,
        failed: &mut bool,
        size: &mut i64,
        mode: &mut u32,
        atime: &mut i64,
        mtime: &mut i64,
    ) -> c_int;
    /// `php_stream_close` inside `zend_try`, which frees the stream.
    safe fn swr_zend_try_stream_close(stream: Stream) -> c_int;
}

impl Stream {
    /// `php_stream_open_wrapper(path, mode, options, NULL)`: the stream, or
    /// None when it failed or bailed out (deferred).
    #[must_use]
    pub fn open(path: Text<'_>, mode: &CStr, options: c_int) -> Option<Self> {
        let mut out = None;
        bailout::defer(bailout::trapped(swr_zend_try_stream_open(
            path,
            mode.into(),
            options,
            &mut out,
        )));
        out
    }

    /// `php_stream_write`: the bytes written, or None when it failed or bailed
    /// out (deferred).
    #[must_use]
    pub fn write(&self, bytes: &[u8]) -> Option<usize> {
        let mut written = -1;
        bailout::defer(bailout::trapped(lend_in(bytes, |bytes, count| {
            swr_zend_try_stream_write(Lent(self.0, PhantomData), bytes, count, &mut written)
        })));
        usize::try_from(written).ok()
    }
}

impl Stream {
    /// `php_stream_read` into `buffer`: the bytes read, or None when it failed
    /// or bailed out (deferred).
    #[must_use]
    pub fn read(&self, buffer: &mut [u8]) -> Option<usize> {
        let mut read = -1;
        bailout::defer(bailout::trapped(lend_out(buffer, |buffer, count| {
            swr_zend_try_stream_read(Lent(self.0, PhantomData), buffer, count, &mut read)
        })));
        usize::try_from(read).ok()
    }

    /// `php_stream_stat`: the size, mode and times, or None when it failed or
    /// bailed out (deferred).
    #[must_use]
    pub fn stat(&self) -> Option<Stat> {
        let mut failed = true;
        let mut stat = Stat {
            size: 0,
            mode: 0,
            atime: 0,
            mtime: 0,
        };
        bailout::defer(bailout::trapped(swr_zend_try_stream_stat(
            Lent(self.0, PhantomData),
            &mut failed,
            &mut stat.size,
            &mut stat.mode,
            &mut stat.atime,
            &mut stat.mtime,
        )));
        (!failed).then_some(stat)
    }
}

impl Drop for Stream {
    /// `php_stream_close`, its bailout deferred. While a bailout is pending
    /// the stream is left open, as a C caller's longjmp left it: the request's
    /// resource list closes it at shutdown, and no more PHP code runs first.
    fn drop(&mut self) {
        if bailout::pending() {
            return;
        }
        bailout::defer(bailout::trapped(swr_zend_try_stream_close(Stream(self.0))));
    }
}
