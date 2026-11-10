//! `swoole_implicit_fn()`: functions for Swoole's own tests, never documented
//! and unsafe by design (they exit, bail out, abort and print raw handler
//! addresses), as `php_swoole.cc` defined it.
use crate::EXCEPTION;
use std::ffi::{c_char, c_int, c_void};
use swoole_rs_corebase::{Switch, built_with};
use swoole_rs_error::ErrorCode::{SW_ERROR_FOR_TEST, SW_ERROR_INVALID_PARAMS};
use swoole_rs_zend::{
    Array, Call, Message, Value, ZString,
    bailout::{self, Bailout},
    handlers,
    sys::{
        swr_zend_facade_function_table, swr_zend_set_exit_status, swr_zval_facade_refcount,
        zend_function,
    },
    throw_exception,
};

handlers! {
    zif_swoole_implicit_fn => implicit_fn;
}

unsafe extern "C" {
    fn php_printf(format: *const c_char, ...) -> usize;
}

/// `sw_php_exit(status)`: `EG(exit_status) = status`, then the bailout that
/// ends the request, or the thread in a `SW_THREAD` build. The frames it
/// crosses own nothing.
#[unsafe(no_mangle)]
pub extern "C" fn sw_php_exit(status: c_int) {
    // SAFETY: a request's executor globals.
    unsafe { swr_zend_set_exit_status(status) };
    if built_with(Switch::Thread) {
        crate::optional::thread_exit();
    } else {
        bailout::raise(Err(Bailout));
    }
}

/// `SwooleG.fatal_error`, which exits the process or bails out of a thread.
type FatalError = unsafe extern "C" fn(code: c_int, format: *const c_char, ...);

fn implicit_fn(call: &Call) {
    let Ok(name) = call.arity(1, 2).and_then(|()| call.string(1, b"")) else {
        return;
    };
    let args = call.zval(2);
    let arg = call.value(2);
    let is = |candidate: &[u8]| name.eq_ignore_ascii_case(candidate);
    if is(b"fatal_error") {
        let hook = swoole_rs_corebase::swr_v1_core_fatal_error();
        if !hook.is_null() {
            // SAFETY: MINIT stored php_swoole.cc's `fatal_error` there.
            let fatal: FatalError = unsafe { std::mem::transmute::<*mut c_void, FatalError>(hook) };
            // SAFETY: it never returns; nothing here needs a drop.
            unsafe { fatal(SW_ERROR_FOR_TEST as c_int, c"test".as_ptr()) };
        }
        // SAFETY: a literal format with no conversions.
        unsafe { php_printf(c"never be executed here\n".as_ptr()) };
    } else if is(b"bailout") {
        #[expect(clippy::cast_possible_truncation, reason = "C's (int) of a zend_long")]
        let status = arg.map_or(95, |status| status.long() as c_int);
        // It bails out across this frame, which owns nothing.
        sw_php_exit(status);
    } else if is(b"abort") {
        std::process::abort();
    } else if is(b"refcount") {
        // SAFETY: upstream's zval_refcount_p of the second argument, which must be refcounted.
        call.set_long(i64::from(unsafe { swr_zval_facade_refcount(args) }));
    } else if is(b"func_handler") {
        let lookup = arg.map_or_else(|| ZString::new(b""), Value::string);
        // SAFETY: a running request's EG(function_table).
        let table = unsafe { swr_zend_facade_function_table() };
        // SAFETY: the request's live table, which this lookup does not change.
        let table = unsafe { Array::from_ptr(table) };
        let found = table.and_then(|table| table.find(lookup.bytes()));
        let Some(function) = found.and_then(Value::pointer) else {
            call.set_bool(false);
            return;
        };
        // SAFETY: a function table's values point at its live functions.
        let function = unsafe { &*function.cast::<zend_function>() };
        // SAFETY: upstream reads every function as internal.
        let handler = unsafe { function.internal_function.handler };
        let address = handler.map_or(std::ptr::null(), |handler| handler as *const c_void);
        // SAFETY: C's stdout, as upstream's printf.
        unsafe { libc::printf(c"zif_handler=%p\n".as_ptr(), address) };
    } else {
        let mut message = Message::new();
        message.push(b"unknown fn '");
        message.push(name);
        message.push(b"'");
        // SAFETY: a handler, with Swoole\Exception registered at MINIT.
        unsafe {
            throw_exception(
                EXCEPTION.entry(),
                message.as_cstr(),
                SW_ERROR_INVALID_PARAMS as i64,
            );
        }
    }
}
