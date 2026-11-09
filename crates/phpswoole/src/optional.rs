//! What configure builds in or leaves out, where upstream's `#ifdef`s ran it:
//! the coroutine PDO drivers and stdext (the cargo features of the same names,
//! which the hybrid turns on with their configure switches) and `Swoole\Thread`
//! (`SW_THREAD`, [`built_with`]). ssh2 and ftp, whose
//! handlers are bundled C, stay native (`HostOps::optional_client_minit`).
use crate::module::{PHP_FATAL_ERROR, Step};
use std::ffi::{CStr, c_char};
use swoole_rs_corebase::{Switch, built_with, log::set_last_error};
use swoole_rs_zend::{E_ERROR, bailout, php_error, sys::zend_class_entry};

// SAFETY: crates/phpthread's, crates/phplocks' and crates/phpstdext's
// phase hooks, linked into every extension; those marked safe have no
// preconditions.
unsafe extern "C" {
    fn swr_v1_phpthread_minit();
    fn swr_v1_phpthread_rinit();
    safe fn swr_v1_phpthread_rshutdown();
    safe fn swr_v1_phpthread_bailout();
    fn swr_v1_phplocks_thread_atomic_minit();
    fn swr_v1_phplocks_thread_lock_minit(
        unconstructed: Option<unsafe extern "C" fn(*const c_char)>,
    );
    fn swr_v1_phpthread_barrier_minit() -> *mut zend_class_entry;
    fn swr_v1_phpthread_queue_minit() -> *mut zend_class_entry;
    fn swr_v1_phpthread_map_minit() -> *mut zend_class_entry;
    fn swr_v1_phpthread_arraylist_minit() -> *mut zend_class_entry;
}

#[cfg(feature = "stdext")]
// SAFETY: crates/phpstdext's MINIT, linked with this feature.
unsafe extern "C" {
    fn swr_v1_phpstdext_minit();
}

/// MINIT, after the servers: the drivers, `Swoole\Thread`, then stdext.
pub(crate) fn minit() -> Vec<Step> {
    let drivers: Vec<Step> = Vec::from([
        #[cfg(feature = "pgsql")]
        Step::Minit(swoole_rs_pgsql::minit::php_swoole_pgsql_minit),
        #[cfg(feature = "odbc")]
        Step::Minit(odbc_minit),
        #[cfg(feature = "oci")]
        Step::Minit(swoole_rs_oci::minit::php_swoole_oracle_minit),
        #[cfg(feature = "sqlite")]
        Step::Minit(swoole_rs_sqlite::minit::php_swoole_sqlite_minit),
        #[cfg(feature = "firebird")]
        Step::Minit(swoole_rs_firebird::minit::php_swoole_firebird_minit),
    ]);
    let stdext: Vec<Step> = Vec::from([
        #[cfg(feature = "stdext")]
        Step::Hook(swr_v1_phpstdext_minit),
    ]);
    let thread = built([(Switch::Thread, Step::Hook(thread_minit))]);
    drivers.into_iter().chain(thread).chain(stdext).collect()
}

/// MSHUTDOWN: the drivers that hold process state.
pub(crate) fn mshutdown() -> Vec<Step> {
    Vec::from([
        #[cfg(feature = "pgsql")]
        Step::Hook(swoole_rs_pgsql::minit::php_swoole_pgsql_mshutdown),
        #[cfg(feature = "oci")]
        Step::Hook(swoole_rs_oci::minit::php_swoole_oracle_mshutdown),
        #[cfg(feature = "sqlite")]
        Step::Hook(swoole_rs_sqlite::minit::php_swoole_sqlite_mshutdown),
        #[cfg(feature = "firebird")]
        Step::Hook(swoole_rs_firebird::minit::php_swoole_firebird_mshutdown),
    ])
}

/// RINIT: Oracle's shared environment, then the main thread's script.
pub(crate) fn rinit() -> Vec<Step> {
    let drivers: Vec<Step> = Vec::from([
        #[cfg(feature = "oci")]
        Step::Hook(swoole_rs_oci::php_swoole_oracle_rinit),
    ]);
    let switched = [(Switch::Thread, Step::Hook(swr_v1_phpthread_rinit))];
    drivers.into_iter().chain(built(switched)).collect()
}

/// RSHUTDOWN: the thread's arguments.
pub(crate) fn rshutdown() -> Vec<Step> {
    built([(Switch::Thread, Step::Hook(swr_v1_phpthread_rshutdown))]).collect()
}

/// The steps whose switch the build has.
fn built<const N: usize>(steps: [(Switch, Step); N]) -> impl Iterator<Item = Step> {
    steps
        .into_iter()
        .filter(|(switch, _)| built_with(*switch))
        .map(|(_, step)| step)
}

/// `swoole_<driver>_set_blocking(blocking)` of a driver the build has.
pub fn set_blocking(driver: Switch, blocking: bool) {
    let set: Option<extern "C" fn(bool)> = match driver {
        #[cfg(feature = "pgsql")]
        Switch::Pgsql => Some(swoole_rs_pgsql::swoole_pgsql_set_blocking),
        #[cfg(feature = "odbc")]
        Switch::Odbc => Some(swoole_rs_odbc::swoole_odbc_set_blocking),
        #[cfg(feature = "oci")]
        Switch::Oracle => Some(swoole_rs_oci::swoole_oracle_set_blocking),
        #[cfg(feature = "sqlite")]
        Switch::Sqlite => Some(swoole_rs_sqlite::swoole_sqlite_set_blocking),
        #[cfg(feature = "firebird")]
        Switch::Firebird => Some(swoole_rs_firebird::swoole_firebird_set_blocking),
        _ => None,
    };
    if let Some(set) = set {
        set(blocking);
    }
}

/// `SW_MUST_BE_MAIN_THREAD`'s test: always true without `SW_THREAD`.
pub(crate) fn main_thread() -> bool {
    !built_with(Switch::Thread) || {
        // SAFETY: no preconditions.
        unsafe { swoole_rs_zend::sys::swr_zend_tsrm_is_main_thread() }
    }
}

/// `php_swoole_thread_bailout()`, outside the main thread of a `SW_THREAD` build.
pub(crate) fn thread_bailout() {
    if !main_thread() {
        swr_v1_phpthread_bailout();
    }
}

#[cfg(feature = "odbc")]
unsafe extern "C" fn odbc_minit(module_number: std::ffi::c_int) {
    // SAFETY: MINIT, with the module's number; upstream ignores the result too.
    unsafe { swoole_rs_odbc::minit::php_swoole_odbc_minit(module_number) };
}

/// `Swoole\Thread` and its locks, atomics, barrier and containers, in
/// upstream's order, inside one `zend_try` as upstream's MINIT.
unsafe extern "C" fn thread_minit() {
    // SAFETY: MINIT, before any PHP thread starts (as for each below).
    unsafe { swr_v1_phpthread_minit() };
    // SAFETY: as above.
    unsafe { swr_v1_phplocks_thread_atomic_minit() };
    // SAFETY: as above, with a reporter that lives for the process.
    unsafe { swr_v1_phplocks_thread_lock_minit(Some(unconstructed)) };
    // SAFETY: as above.
    unsafe { swr_v1_phpthread_barrier_minit() };
    // SAFETY: as above.
    unsafe { swr_v1_phpthread_queue_minit() };
    // SAFETY: as above.
    unsafe { swr_v1_phpthread_map_minit() };
    // SAFETY: as above.
    unsafe { swr_v1_phpthread_arraylist_minit() };
}

/// `php_swoole_fatal_error(E_ERROR, "%s", message)` for `Thread\Lock`, which
/// calls it under `zend_try`; the error's bailout continues there, through this
/// frame, which owns nothing.
unsafe extern "C" fn unconstructed(message: *const c_char) {
    set_last_error(PHP_FATAL_ERROR);
    // SAFETY: crates/phplocks' static, NUL-terminated message.
    let message = unsafe { CStr::from_ptr(message) };
    // SAFETY: a handler's error, as upstream raises it.
    bailout::raise(unsafe { php_error(E_ERROR, message) });
}
