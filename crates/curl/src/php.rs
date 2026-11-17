// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 ext-src/swoole_curl.cc (Swoole contributors).
//! The check behind `swoole_curl_get_handle`: whether the bundled `ext/curl`'s
//! `php_curl` may be used by the current coroutine, given the multi its easy
//! handle waits in.
use crate::{Easy, host};
use swoole_rs_zend::{E_WARNING, bailout};

/// `SW_ERROR_PHP_FATAL_ERROR`.
const PHP_FATAL_ERROR: i32 = swoole_rs_error::ErrorCode::SW_ERROR_PHP_FATAL_ERROR as i32;

swoole_rs_zend::exports! {
/// Whether the current coroutine may use the `php_curl` whose easy handle is
/// `cp`: false after the warning of a missing coroutine handle or the fatal
/// error of a multi another coroutine waits on.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_curl_check_handle(cp: Easy<'_>, exclusive: bool, required: bool) -> bool {
    let shut_down = swoole_rs_phpswoole::globals().req_status.get()
        == swoole_rs_phpswoole::ReqStatus::RshutdownEnd;
    if !exclusive || shut_down || host::current().is_null() {
        return true;
    }
    let Some(handle) = cp.handle() else {
        if required {
            swoole_rs_corebase::log::set_last_error(PHP_FATAL_ERROR);
            bailout::defer(swoole_rs_zend::report(
                E_WARNING,
                c"The given handle is not initialized in coroutine",
            ));
            return false;
        }
        return true;
    };
    let Some(multi) = handle.multi() else {
        return true;
    };
    let mut buffer = [0; 128];
    let bound = host::bound_co(&multi, &mut buffer);
    drop(multi);
    let Err((code, message)) = bound else {
        return true;
    };
    // SAFETY: Swoole's fatal error hook; C++ calls this, and this frame owns
    // nothing that needs a drop: the multi's reference is dropped above.
    unsafe { swoole_rs_corebase::fatal_error(code, message) };
    false
}
}
