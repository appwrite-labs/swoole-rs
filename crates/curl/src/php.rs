// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 ext-src/swoole_curl.cc (Swoole contributors).
//! `swoole_curl_get_handle`: the bundled `ext/curl`'s `php_curl` behind a
//! `CurlHandle` zval, checked against the coroutine its multi waits in.
use crate::{Easy, host};
use swoole_rs_curl_sys::CURL;
use swoole_rs_zend::{E_WARNING, Value, bailout, sys::zval};

/// `SW_ERROR_PHP_FATAL_ERROR`.
const PHP_FATAL_ERROR: i32 = 503;

/// The `php_curl` of `zid` (whose first field is its easy handle) on behalf
/// of the current coroutine; null after the warning of a missing coroutine
/// handle or the fatal error of a multi another coroutine waits on.
/// # Safety
/// `zid` is a live `CurlHandle` zval, in a request.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_curl_get_handle(
    zid: *mut zval,
    exclusive: bool,
    required: bool,
) -> *mut *mut CURL {
    // SAFETY: per the caller, a live zval.
    let Some(object) = unsafe { Value::from_ptr(zid) }.and_then(Value::object) else {
        return std::ptr::null_mut();
    };
    // SAFETY: a live object.
    let handlers = unsafe { (*object).handlers };
    // SAFETY: its live handlers; `offset` is where `std` sits in `php_curl`.
    let offset = unsafe { (*handlers).offset };
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "Z_CURL_P: php_curl, which is pointer-aligned, starts `offset` bytes before its std"
    )]
    let ch = object
        .cast::<u8>()
        .wrapping_sub(usize::try_from(offset).unwrap_or(0))
        .cast::<*mut CURL>();
    let shut_down =
        swoole_rs_phpswoole::globals().req_status.get() == swoole_rs_phpswoole::RSHUTDOWN_END;
    if !exclusive || shut_down || host::current().is_null() {
        return ch;
    }
    // SAFETY: `ch` is the live `php_curl`, which starts with its live easy handle.
    let cp = unsafe { *ch.cast::<Easy>() };
    // SAFETY: null or the easy handle's live Swoole handle.
    let Some(handle) = (unsafe { cp.handle().as_ref() }) else {
        if required {
            swoole_rs_corebase::log::set_last_error(PHP_FATAL_ERROR);
            bailout::raise(swoole_rs_zend::report(
                E_WARNING,
                c"The given handle is not initialized in coroutine",
            ));
            return std::ptr::null_mut();
        }
        return ch;
    };
    // SAFETY: null or the handle's live multi.
    let Some(multi) = (unsafe { handle.multi.as_ref() }) else {
        return ch;
    };
    // SAFETY: this frame owns nothing that needs a drop.
    if unsafe { host::swoole_curl_check_bound_co(multi) }.is_null() {
        return std::ptr::null_mut();
    }
    ch
}
