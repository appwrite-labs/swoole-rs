//! What configure builds in or leaves out, where upstream's `#ifdef`s ran it:
//! the coroutine PDO drivers, stdext, the bundled ssh2 client and
//! ext/sockets' `Socket` (the cargo features of the same names, which the
//! hybrid turns on with their configure switches) and `Swoole\Thread`
//! (`SW_THREAD`, [`built_with`]).
use crate::module::{Families, Step, families};
use std::ffi::c_int;
use swoole_rs_corebase::{Switch, built_with};
use swoole_rs_zend::{Owned, Value};

#[cfg(feature = "ssh2")]
// the bundled ext/ssh2's module hooks (thirdparty/php/ssh2), linked with this feature.
unsafe extern "C" {
    /// ssh2's information table: rows of its static version strings, printed.
    safe fn php_swoole_ssh2_minfo();
}

#[cfg(feature = "sockets")]
// ext/sockets' class and import, which PHP exports when it has the extension.
unsafe extern "C" {
    /// `Socket`, an internal class: written once, by ext/sockets' MINIT, before
    /// any request reads it, and live for the process.
    safe static socket_ce: Option<swoole_rs_zend::ClassEntry<'static>>;
    /// Imports `fd` into the new `Socket`, which takes it.
    safe fn socket_import_file_descriptor(fd: c_int, socket: PhpSocket<'_>) -> bool;
}

/// `Z_SOCKET_P` of a `Socket` [`socket_create`] just made: ext/sockets'
/// `php_socket` (`int bsd_socket, type, error, blocking; zval zstream;
/// zend_object std`) ends with the object. Only `socket_create` makes one.
#[cfg(feature = "sockets")]
#[repr(transparent)]
struct PhpSocket<'a>(
    std::ptr::NonNull<std::ffi::c_void>,
    std::marker::PhantomData<&'a Owned>,
);

/// MINIT, after the HTTP/2 client: ssh2's.
pub(crate) fn client_minit() -> Vec<Step> {
    Vec::from([
        #[cfg(feature = "ssh2")]
        Step::Typed(ssh2_minit),
    ])
}

#[cfg(feature = "ssh2")]
fn ssh2_minit(minit: &swoole_rs_zend::Minit) {
    unsafe extern "C" {
        /// ssh2's MINIT, which the token proves this is, with the module's number.
        safe fn php_swoole_ssh2_minit(module_number: c_int) -> c_int;
    }
    // Upstream ignores the result too.
    php_swoole_ssh2_minit(minit.module());
}

/// MSHUTDOWN, after the drivers': ssh2's.
pub(crate) fn client_mshutdown() -> Vec<Step> {
    Vec::from([
        #[cfg(feature = "ssh2")]
        Step::Shutdown(ssh2_mshutdown),
    ])
}

#[cfg(feature = "ssh2")]
fn ssh2_mshutdown(_mshutdown: &swoole_rs_zend::Mshutdown) {
    unsafe extern "C" {
        /// ssh2's MSHUTDOWN, which the token proves this is, after its MINIT.
        safe fn php_swoole_ssh2_mshutdown() -> c_int;
    }
    php_swoole_ssh2_mshutdown();
}

/// MINFO's ssh2 table, a `zend_try` body.
pub(crate) fn client_minfo() {
    #[cfg(feature = "ssh2")]
    php_swoole_ssh2_minfo();
}

/// A duplicate of an ext/sockets `Socket`'s descriptor; -1 after throwing
/// "failed to dup socket fd", -2 after "invalid socket fd", -3 when `value` is
/// no `Socket` or the build lacks ext/sockets.
/// In a request.
pub fn socket_cast(value: &Value) -> c_int {
    #[cfg(feature = "sockets")]
    {
        // `Z_SOCKET_P(value)->bsd_socket` of a `Socket`, the class ext/sockets
        // registers as `socket_ce`.
        let Some((fd, _)) = value.php_socket_fd() else {
            return -3;
        };
        let (fd, message, code) = if fd == -1 {
            (-2, c"invalid socket fd", libc::EBADF)
        } else {
            let error = match swoole_rs_corebase::sys::dup(fd) {
                Ok(dup) => return dup,
                Err(error) => error,
            };
            let errno = error.raw_os_error().unwrap_or_default();
            (-1, c"failed to dup socket fd", errno)
        };
        crate::EXCEPTION.throw(message, i64::from(code));
        fd
    }
    #[cfg(not(feature = "sockets"))]
    {
        let _ = value;
        -3
    }
}

/// An ext/sockets `Socket` over `fd`, which it takes, and whether it imported
/// `fd`: false, with the value undefined, when the build lacks ext/sockets.
#[must_use]
pub fn socket_create(fd: c_int) -> (Owned, bool) {
    #[cfg(feature = "sockets")]
    {
        const STD: usize = 4 * size_of::<c_int>() + size_of::<swoole_rs_zend::sys::zval>();
        // Only an abstract class or an interface fails, leaving no object.
        let out = socket_ce.map_or_else(Owned::default, swoole_rs_zend::ClassEntry::create_object);
        // The new `Socket`, whose `php_socket` precedes its object.
        let socket = out
            .object()
            .and_then(|object| std::ptr::NonNull::new(object.cast::<u8>().wrapping_sub(STD)));
        let Some(socket) = socket else {
            return (out, false);
        };
        let imported =
            socket_import_file_descriptor(fd, PhpSocket(socket.cast(), std::marker::PhantomData));
        (out, imported)
    }
    #[cfg(not(feature = "sockets"))]
    {
        let _ = fd;
        (Owned::default(), false)
    }
}

/// MINIT, after the servers: the drivers, `Swoole\Thread`, then stdext.
pub(crate) fn minit() -> Vec<Step> {
    let drivers: Vec<Step> = Vec::from([
        #[cfg(feature = "pgsql")]
        Step::Typed(swoole_rs_pgsql::minit::minit),
        #[cfg(feature = "odbc")]
        Step::Typed(swoole_rs_odbc::minit::minit),
        #[cfg(feature = "oci")]
        Step::Typed(swoole_rs_oci::minit::minit),
        #[cfg(feature = "sqlite")]
        Step::Typed(swoole_rs_sqlite::minit::minit),
        #[cfg(feature = "firebird")]
        Step::Typed(swoole_rs_firebird::minit::minit),
    ]);
    let stdext: Vec<Step> = Vec::from([
        #[cfg(feature = "stdext")]
        Step::Typed(swoole_rs_phpstdext::stdext_minit),
    ]);
    let thread = built([(Switch::Thread, Step::Typed(thread_minit))]);
    drivers.into_iter().chain(thread).chain(stdext).collect()
}

/// MSHUTDOWN: the drivers that hold process state.
pub(crate) fn mshutdown() -> Vec<Step> {
    Vec::from([
        #[cfg(feature = "pgsql")]
        Step::Shutdown(swoole_rs_pgsql::minit::mshutdown),
        #[cfg(feature = "oci")]
        Step::Shutdown(swoole_rs_oci::minit::mshutdown),
        #[cfg(feature = "sqlite")]
        Step::Shutdown(swoole_rs_sqlite::minit::mshutdown),
        #[cfg(feature = "firebird")]
        Step::Shutdown(swoole_rs_firebird::minit::mshutdown),
    ])
}

/// RINIT: Oracle's shared environment, then the main thread's script.
pub(crate) fn rinit(families: &Families) -> Vec<Step> {
    let drivers: Vec<Step> = Vec::from([
        #[cfg(feature = "oci")]
        Step::Hook(swoole_rs_oci::php_swoole_oracle_rinit),
    ]);
    let switched = [(Switch::Thread, Step::Request(families.thread_rinit))];
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
    !built_with(Switch::Thread) || swoole_rs_zend::is_main_thread()
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

/// `Swoole\Thread` and its locks, atomics, barrier and containers, in
/// upstream's order, inside one `zend_try` as upstream's MINIT.
fn thread_minit(minit: &swoole_rs_zend::Minit) {
    let Some(families) = families() else {
        return;
    };
    (families.thread_minit)(minit);
    // Raised here, inside this step's `zend_try`, as the registrations' own are.
    swoole_rs_zend::bailout::raise((families.locks_minit[3])(minit));
    for register in families.thread_classes_minit {
        register(minit);
    }
}
