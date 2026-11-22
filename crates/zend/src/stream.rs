//! A PHP stream a Rust function opens through its wrapper
//! (`php_stream_open_wrapper`), writes and closes, as the C it replaces did. A
//! user stream wrapper runs PHP code on each call, so each is a zend-sys
//! `swr_zend_try_stream_*` shim inside `zend_try`, its bailout deferred; a
//! caller stops at [`bailout::pending`], where C's longjmp would have left.
use crate::bailout;
use crate::sys::php_stream;
use std::ffi::{CStr, c_int};
use std::marker::PhantomData;
use std::ptr::NonNull;
use swoole_rs_corebase::lent::{Count, In, lend_in};
use swoole_rs_corebase::sys::Text;

/// `REPORT_ERRORS`: the open warns when it fails.
pub const REPORT_ERRORS: c_int = 8;

/// A stream [`Stream::open`] opened, which no zval holds: dropping it closes it.
#[repr(transparent)]
pub struct Stream(NonNull<php_stream>);

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
