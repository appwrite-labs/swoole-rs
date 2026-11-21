use super::*;
use std::cell::Cell;

thread_local! {
    /// The waits the fake host saw, and how many succeed before one fails.
    static WAITS: Cell<i32> = const { Cell::new(0) };
    static WAITABLE: Cell<i32> = const { Cell::new(i32::MAX) };
    static EVENT: Cell<c_int> = const { Cell::new(0) };
}

unsafe extern "C" fn wait(_session: *mut LIBSSH2_SESSION, event: c_int) -> bool {
    EVENT.with(|seen| seen.set(event));
    WAITS.with(|waits| waits.set(waits.get() + 1));
    WAITABLE.with(|left| {
        left.set(left.get() - 1);
        left.get() >= 0
    })
}

static OPS: Ops = Ops { wait };

fn reset(waitable: i32) {
    WAITS.with(|waits| waits.set(0));
    WAITABLE.with(|left| left.set(waitable));
}

fn waits() -> i32 {
    WAITS.with(Cell::get)
}

#[test]
fn outbound_blocks_wait_for_writing() {
    assert_eq!(event(LIBSSH2_SESSION_BLOCK_OUTBOUND), WRITE, "outbound");
    assert_eq!(event(LIBSSH2_SESSION_BLOCK_OUTBOUND | 1), WRITE, "both");
    assert_eq!(event(1), READ, "inbound");
    assert_eq!(event(0), READ, "neither");
}

#[test]
fn eagain_waits_and_retries() {
    reset(i32::MAX);
    let mut results = [LIBSSH2_ERROR_EAGAIN, LIBSSH2_ERROR_EAGAIN, 7].into_iter();
    let result = unsafe {
        Wait::new(&raw const OPS, ptr::null_mut()).retry(WRITE, || results.next().unwrap())
    };
    assert_eq!(result, 7, "the final result");
    assert_eq!(waits(), 2, "one wait per EAGAIN");
    assert_eq!(
        EVENT.with(Cell::get),
        WRITE,
        "the event it started blocked on"
    );
}

#[test]
fn a_failed_wait_reports_no_socket() {
    reset(0);
    let result =
        unsafe { Wait::new(&raw const OPS, ptr::null_mut()).retry(READ, || LIBSSH2_ERROR_EAGAIN) };
    assert_eq!(result, LIBSSH2_ERROR_SOCKET_NONE, "socket none");
    assert_eq!(waits(), 1, "one wait");
}

#[test]
fn other_errors_return_at_once() {
    reset(i32::MAX);
    let result = unsafe { Wait::new(&raw const OPS, ptr::null_mut()).retry(READ, || -18) };
    assert_eq!(result, -18, "passed through");
    assert_eq!(waits(), 0, "no wait");
}

#[test]
fn null_with_eagain_waits_and_retries() {
    reset(i32::MAX);
    let mut value = 5_u8;
    let target = &raw mut value;
    let mut results = [ptr::null_mut(), target].into_iter();
    let handle = unsafe {
        Wait::new(&raw const OPS, ptr::null_mut()).retry_pointer(
            READ,
            || results.next().unwrap(),
            || LIBSSH2_ERROR_EAGAIN,
        )
    };
    assert_eq!(handle, target, "the handle");
    assert_eq!(waits(), 1, "one wait");
}

#[test]
fn null_with_another_error_or_a_failed_wait_is_null() {
    reset(i32::MAX);
    let handle = unsafe {
        Wait::new(&raw const OPS, ptr::null_mut()).retry_pointer(READ, ptr::null_mut::<u8>, || -18)
    };
    assert!(handle.is_null(), "another error");
    assert_eq!(waits(), 0, "no wait");
    reset(0);
    let handle = unsafe {
        Wait::new(&raw const OPS, ptr::null_mut())
            .retry_pointer(READ, ptr::null_mut::<u8>, || LIBSSH2_ERROR_EAGAIN)
    };
    assert!(handle.is_null(), "a failed wait");
    assert_eq!(waits(), 1, "one wait");
}

#[test]
fn wide_results_narrow_as_upstream_does() {
    assert_eq!((-37_isize).narrow(), -37, "ssize_t");
    assert_eq!(0x1_0000_0005_usize.narrow(), 5, "size_t");
}
