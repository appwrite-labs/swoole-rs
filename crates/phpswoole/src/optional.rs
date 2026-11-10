//! What configure builds in or leaves out, where upstream's `#ifdef`s ran it:
//! the coroutine PDO drivers, stdext, the bundled ssh2 and ftp clients and
//! ext/sockets' `Socket` (the cargo features of the same names, which the
//! hybrid turns on with their configure switches) and `Swoole\Thread`
//! (`SW_THREAD`, [`built_with`]).
use crate::module::{Families, PHP_FATAL_ERROR, Step, families};
use std::ffi::{CStr, c_char, c_int, c_void};
use swoole_rs_corebase::{Switch, built_with, log::set_last_error};
use swoole_rs_zend::{E_ERROR, Value, bailout, php_error, sys::zval};

// SAFETY: crates/phplocks' thread locks and atomics, linked into every extension;
// declared, since crates/phplocks depends on this crate.
unsafe extern "C" {
    fn swr_v1_phplocks_thread_atomic_minit();
    fn swr_v1_phplocks_thread_lock_minit(
        unconstructed: Option<unsafe extern "C" fn(*const c_char)>,
    );
}

#[cfg(feature = "stdext")]
// SAFETY: crates/phpstdext's MINIT, linked with this feature.
unsafe extern "C" {
    fn swr_v1_phpstdext_minit();
}

#[cfg(feature = "ssh2")]
// SAFETY: the bundled ext/ssh2's module hooks (thirdparty/php/ssh2), linked with this feature.
unsafe extern "C" {
    fn php_swoole_ssh2_minit(module_number: c_int) -> c_int;
    fn php_swoole_ssh2_mshutdown() -> c_int;
    fn php_swoole_ssh2_minfo();
}

#[cfg(feature = "ftp")]
// SAFETY: the bundled ext/ftp's module hooks (thirdparty/php84/ftp), linked with this feature.
unsafe extern "C" {
    fn zm_startup_ftp(kind: c_int, module_number: c_int) -> c_int;
    fn zm_info_ftp(module: *mut c_void);
}

#[cfg(feature = "sockets")]
// SAFETY: ext/sockets' class and import, which PHP exports when it has the extension.
unsafe extern "C" {
    static socket_ce: *mut swoole_rs_zend::sys::zend_class_entry;
    fn socket_import_file_descriptor(fd: c_int, socket: *mut c_void) -> bool;
}

/// MINIT, after the HTTP/2 client: ssh2's, then ftp's.
pub(crate) fn client_minit() -> Vec<Step> {
    Vec::from([
        #[cfg(feature = "ssh2")]
        Step::Minit(ssh2_minit),
        #[cfg(feature = "ftp")]
        Step::Minit(ftp_minit),
    ])
}

#[cfg(feature = "ssh2")]
extern "C" fn ssh2_minit(module_number: c_int) {
    // SAFETY: MINIT, with the module's number; upstream ignores the result too.
    unsafe { php_swoole_ssh2_minit(module_number) };
}

#[cfg(feature = "ftp")]
extern "C" fn ftp_minit(module_number: c_int) {
    /// `MODULE_PERSISTENT`.
    const PERSISTENT: c_int = 1;
    // SAFETY: as above.
    unsafe { zm_startup_ftp(PERSISTENT, module_number) };
}

/// MSHUTDOWN, after the drivers': ssh2's.
pub(crate) fn client_mshutdown() -> Vec<Step> {
    Vec::from([
        #[cfg(feature = "ssh2")]
        Step::Hook(ssh2_mshutdown),
    ])
}

#[cfg(feature = "ssh2")]
extern "C" fn ssh2_mshutdown() {
    // SAFETY: MSHUTDOWN, after its MINIT.
    unsafe { php_swoole_ssh2_mshutdown() };
}

/// MINFO's ssh2 and ftp tables, a `zend_try` body.
/// # Safety
/// MINFO, with the module's live entry.
pub(crate) unsafe extern "C" fn client_minfo(module: *mut c_void) {
    #[cfg(feature = "ssh2")]
    // SAFETY: per the caller.
    unsafe {
        php_swoole_ssh2_minfo();
    }
    #[cfg(feature = "ftp")]
    // SAFETY: per the caller.
    unsafe {
        zm_info_ftp(module);
    }
    let _ = module;
}

/// A duplicate of an ext/sockets `Socket`'s descriptor; -1 after throwing
/// "failed to dup socket fd", -2 after "invalid socket fd", -3 when `value` is
/// no `Socket` or the build lacks ext/sockets.
/// In a request.
pub fn socket_cast(value: &Value) -> c_int {
    #[cfg(feature = "sockets")]
    {
        let (Some(object), Some(class)) = (value.object(), value.class()) else {
            return -3;
        };
        // SAFETY: ext/sockets' class, set at its MINIT.
        let socket = unsafe { socket_ce };
        // SAFETY: two live classes.
        if !unsafe { swoole_rs_zend::sys::swr_zend_instanceof(class, socket) } {
            return -3;
        }
        // SAFETY: a `Socket`, whose `php_socket` starts with its descriptor.
        let fd = unsafe { *php_socket(object).cast::<c_int>() };
        let (fd, message, code) = if fd == -1 {
            (-2, c"invalid socket fd", libc::EBADF)
        } else {
            // SAFETY: a descriptor the socket owns.
            let dup = unsafe { libc::dup(fd) };
            if dup != -1 {
                return dup;
            }
            let errno = std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or_default();
            (-1, c"failed to dup socket fd", errno)
        };
        let exception = crate::EXCEPTION.entry();
        // SAFETY: a request, with a registered class.
        unsafe { swoole_rs_zend::throw_exception(exception, message, i64::from(code)) };
        fd
    }
    #[cfg(not(feature = "sockets"))]
    {
        let _ = value;
        -3
    }
}

/// `Z_SOCKET_P`: ext/sockets' `php_socket` (`int bsd_socket, type, error,
/// blocking; zval zstream; zend_object std`) ends with the object.
#[cfg(feature = "sockets")]
fn php_socket(object: *mut swoole_rs_zend::sys::zend_object) -> *mut c_void {
    const STD: usize = 4 * size_of::<c_int>() + size_of::<zval>();
    object.cast::<u8>().wrapping_sub(STD).cast()
}

/// An ext/sockets `Socket` over `fd`, which it takes, into `out`; false when
/// it can't import `fd` or the build lacks ext/sockets.
/// # Safety
/// `out` is a writable zval.
pub unsafe fn socket_create(out: *mut zval, fd: c_int) -> bool {
    #[cfg(feature = "sockets")]
    {
        // SAFETY: ext/sockets' class, set at its MINIT; `out` per the caller.
        unsafe { swoole_rs_zend::sys::object_init_ex(out, socket_ce) };
        // SAFETY: the new `Socket` object.
        let object = unsafe { swoole_rs_zend::value::obj_of(out) };
        // SAFETY: the new `Socket`, whose `php_socket` precedes its object.
        unsafe { socket_import_file_descriptor(fd, php_socket(object)) }
    }
    #[cfg(not(feature = "sockets"))]
    {
        let _ = (out, fd);
        false
    }
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
pub(crate) fn rinit(families: &Families) -> Vec<Step> {
    let drivers: Vec<Step> = Vec::from([
        #[cfg(feature = "oci")]
        Step::Hook(swoole_rs_oci::php_swoole_oracle_rinit),
    ]);
    let switched = [(Switch::Thread, Step::Hook(families.thread_rinit))];
    drivers.into_iter().chain(built(switched)).collect()
}

/// RSHUTDOWN: the thread's arguments.
pub(crate) fn rshutdown(families: &Families) -> Vec<Step> {
    built([(Switch::Thread, Step::Hook(families.thread_rshutdown))]).collect()
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

/// `php_swoole_thread_bailout()`: the bailout that ends this PHP thread.
pub(crate) fn thread_exit() {
    if let Some(families) = families() {
        (families.thread_bailout)();
    }
}

/// `php_swoole_thread_bailout()`, outside the main thread of a `SW_THREAD` build.
pub(crate) fn thread_bailout() {
    if !main_thread() {
        thread_exit();
    }
}

#[cfg(feature = "odbc")]
extern "C" fn odbc_minit(module_number: std::ffi::c_int) {
    // SAFETY: MINIT, with the module's number; upstream ignores the result too.
    unsafe { swoole_rs_odbc::minit::php_swoole_odbc_minit(module_number) };
}

/// `Swoole\Thread` and its locks, atomics, barrier and containers, in
/// upstream's order, inside one `zend_try` as upstream's MINIT.
extern "C" fn thread_minit() {
    let Some(families) = families() else {
        return;
    };
    // SAFETY: MINIT, before any PHP thread starts (as for each below).
    unsafe { (families.thread_minit)() };
    // SAFETY: as above.
    unsafe { swr_v1_phplocks_thread_atomic_minit() };
    // SAFETY: as above, with a reporter that lives for the process.
    unsafe { swr_v1_phplocks_thread_lock_minit(Some(unconstructed)) };
    for minit in families.thread_classes_minit {
        // SAFETY: as above.
        unsafe { minit() };
    }
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
