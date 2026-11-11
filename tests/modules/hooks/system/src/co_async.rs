//! Upstream's `core-tests/src/coroutine/async.cpp`, one function per TEST with the same
//! inputs and expected values: `coroutine::async(handler, event, timeout)` and
//! `async(fn)` are crates/hooks's `swr_v1_hook_async` and `async_job` over
//! the coroutine's own operations (`COROUTINE_ASYNC`), as the hooked functions call them.

use std::ffi::{c_char, c_int, c_void};
use std::net::Ipv4Addr;

use swoole_rs_hooks::offload::{AsyncEvent, async_job, swr_v1_hook_async};
use swoole_rs_hooks::resolve::COROUTINE_ASYNC;

use super::{AF_INET, ENOENT, O_RDONLY, SW_ERROR_CO_TIMEDOUT};
use super::{coroutine_cancel, current, errno, open, run, timer};

const MAGIC_CODE: isize = 0x700_9501;
const SW_OK: c_int = 0;
const SW_ERROR_CO_CANCELED: c_int = 10017;

unsafe extern "C" {
    fn swr_v1_clients_gethostbyname(flags: c_int, name: *const c_char, addr: *mut c_char) -> c_int;
    fn inet_ntop(family: c_int, src: *const c_void, dst: *mut c_char, size: u32) -> *const c_char;
    fn usleep(usec: u32) -> c_int;
}

/// `AsyncEvent ev = {}`.
fn empty() -> AsyncEvent {
    // SAFETY: integers, a float, raw pointers and optional function pointers, for which
    // all-zero is the empty event.
    unsafe { std::mem::zeroed() }
}

/// `swoole::coroutine::async(handler, event, timeout)`.
fn co_async(
    handler: unsafe extern "C" fn(*mut AsyncEvent),
    event: &mut AsyncEvent,
    timeout: f64,
) -> bool {
    // SAFETY: a live event and a pool handler for it, in a coroutine.
    unsafe { swr_v1_hook_async(event, handler, timeout, &COROUTINE_ASYNC) }
}

/// `swoole::coroutine::async(fn)`.
fn co_async_fn(mut job: impl FnMut() + Send) -> bool {
    async_job(&mut job)
}

unsafe extern "C" fn sleep_1ms(event: *mut AsyncEvent) {
    // SAFETY: usleep(3); the pool's copy of the event.
    unsafe {
        usleep(1000);
        (*event).retval = MAGIC_CODE;
    }
}

unsafe extern "C" fn sleep_200ms(event: *mut AsyncEvent) {
    // SAFETY: usleep(3); the pool's copy of the event.
    unsafe {
        usleep(200_000);
        (*event).retval = MAGIC_CODE;
    }
}

fn usleep_() {
    run(|| {
        let mut ev = empty();
        let retval = co_async(sleep_1ms, &mut ev, -1.0);
        assert!(retval);
        assert_eq!(ev.retval, MAGIC_CODE);
    });
}

fn gethostbyname() {
    run(|| {
        let domain = c"www.baidu.com";
        let mut ip = String::new();

        let retval = co_async_fn(|| {
            let mut buf = [0 as c_char; 128];
            // SAFETY: a NUL-terminated name and room for an IPv4 address.
            if unsafe { swr_v1_clients_gethostbyname(AF_INET, domain.as_ptr(), buf.as_mut_ptr()) }
                == SW_OK
            {
                let mut addr = [0 as c_char; 128];
                // SAFETY: an IPv4 address and a 128-byte buffer.
                unsafe { inet_ntop(AF_INET, buf.as_ptr().cast(), addr.as_mut_ptr(), 128) };
                // SAFETY: inet_ntop wrote a NUL-terminated address.
                ip = unsafe { std::ffi::CStr::from_ptr(addr.as_ptr()) }
                    .to_string_lossy()
                    .into_owned();
            } else {
                ip = "unknown".into();
            }
        });

        assert!(retval);
        // `regex_match(ip, "(\d{1,3}).(\d{1,3}).(\d{1,3}).(\d{1,3})")`, which inet_ntop's
        // output matches exactly when it is a dotted quad.
        assert!(ip.parse::<Ipv4Addr>().is_ok(), "{ip}");
    });
}

fn error() {
    run(|| {
        let mut retval: c_int = 0x700_9501;
        let test_file = c"/tmp/swoole_core_test_file_not_exists";
        co_async_fn(|| {
            // SAFETY: a NUL-terminated path.
            retval = unsafe { open(test_file.as_ptr(), O_RDONLY) };
        });
        assert_eq!(retval, -1);
        assert_eq!(errno(), ENOENT);
    });
}

fn cancel() {
    run(|| {
        let mut ev = empty();
        let co = current();
        timer::after(50, move |_| {
            // SAFETY: the case's coroutine, parked in async.
            unsafe { coroutine_cancel(co) };
        });

        let retval = co_async(sleep_200ms, &mut ev, -1.0);

        assert!(!retval);
        assert_eq!(ev.error, SW_ERROR_CO_CANCELED);
    });
}

fn timeout() {
    run(|| {
        let mut ev = empty();

        let retval = co_async(sleep_200ms, &mut ev, 0.1);

        assert!(!retval);
        assert_eq!(ev.error, SW_ERROR_CO_TIMEDOUT);
    });
}

/// `coroutine_async.<name>`, in upstream's order.
pub(super) const CASES: super::Cases = &[
    ("usleep", usleep_),
    ("gethostbyname", gethostbyname),
    ("error", error),
    ("cancel", cancel),
    ("timeout", timeout),
];
