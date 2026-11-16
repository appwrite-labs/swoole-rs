//! `swoole_implicit_fn()`: functions for Swoole's own tests, never documented
//! and unsafe by design (they exit, bail out, abort and print raw handler
//! addresses), as `php_swoole.cc` defined it.
use crate::EXCEPTION;
use std::ffi::{c_int, c_void};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::{Switch, built_with};
use swoole_rs_error::ErrorCode::{SW_ERROR_FOR_TEST, SW_ERROR_INVALID_PARAMS};
use swoole_rs_zend::{
    Call, Message, Value, ZString,
    bailout::{self, Bailout},
    handlers,
};

handlers! {
    zif_swoole_implicit_fn => implicit_fn;
}

unsafe extern "C" {
    /// `ZEND_WRITE` of the lent bytes (an `IoSlice` is an iovec) inside
    /// `zend_try`: nonzero when an output handler bailed out.
    safe fn swr_zend_try_write(text: std::io::IoSlice<'_>) -> c_int;
    /// `EG(exit_status) = status`: one value of the executor's is written.
    safe fn swr_zend_set_exit_status(status: c_int);
    /// `zval_refcount_p`: the count of what a live zval holds, read only; 0 for
    /// a value that holds nothing counted.
    safe fn swr_zval_facade_refcount(value: &Value) -> u32;
    /// libc's `puts`: the NUL-terminated line and a newline to C's stdout.
    safe fn puts(line: swoole_rs_corebase::sys::Text<'_>) -> c_int;
}

/// `sw_php_exit(status)`: `EG(exit_status) = status`, then the bailout that
/// ends the request, or the thread in a `SW_THREAD` build. The frames it
/// crosses own nothing.
#[unsafe(no_mangle)]
pub extern "C" fn sw_php_exit(status: c_int) {
    swr_zend_set_exit_status(status);
    if built_with(Switch::Thread) {
        crate::optional::thread_exit();
    } else {
        bailout::raise(Err(Bailout));
    }
}

fn implicit_fn(call: &Call) {
    let Ok(name) = call.arity(1, 2).and_then(|()| call.string(1, b"")) else {
        return;
    };
    let arg = call.value(2);
    let is = |candidate: &[u8]| name.eq_ignore_ascii_case(candidate);
    if is(b"fatal_error") {
        // The hook MINIT stored never returns: it exits, or ends this thread with
        // the bailout, which goes on from this frame.
        if let Err(bailout) = crate::swoole_fatal_error(SW_ERROR_FOR_TEST as c_int, c"test") {
            bailout.defer();
            return;
        }
        // `php_printf` of a format with no conversions: its bytes, written.
        let text = std::io::IoSlice::new(b"never be executed here\n");
        bailout::defer(bailout::trapped(swr_zend_try_write(text)));
    } else if is(b"bailout") {
        let status = arg.map_or(95, |status| status.long().c_cast::<c_int>());
        // It bails out across this frame, which owns nothing.
        sw_php_exit(status);
    } else if is(b"abort") {
        #[expect(clippy::disallowed_methods, reason = "the script called abort()")]
        std::process::abort();
    } else if is(b"refcount") {
        // Upstream's zval_refcount_p of the second argument.
        call.set_long(i64::from(
            arg.map_or(0, |arg| swr_zval_facade_refcount(arg)),
        ));
    } else if is(b"func_handler") {
        let lookup = arg.map_or_else(|| ZString::new(b""), Value::string);
        // A function PHP code defines has no internal handler.
        let Some(handler) = call.php().internal_handler(lookup.bytes()) else {
            call.set_bool(false);
            return;
        };
        let address = handler.map_or(std::ptr::null(), |handler| handler as *const c_void);
        // Upstream's `printf("zif_handler=%p\n")` to C's stdout; glibc spells null `(nil)`.
        let address = if address.is_null() && cfg!(target_env = "gnu") {
            "(nil)".to_owned()
        } else {
            format!("{address:p}")
        };
        let line = std::ffi::CString::new(format!("zif_handler={address}")).unwrap_or_default();
        puts(line.as_c_str().into());
    } else {
        let mut message = Message::new();
        message.push(b"unknown fn '");
        message.push(name);
        message.push(b"'");
        EXCEPTION.throw(message.as_cstr(), SW_ERROR_INVALID_PARAMS as i64);
    }
}
