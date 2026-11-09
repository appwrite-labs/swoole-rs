//! The driver's side of interrupted I/O (upstream's
//! `php_swoole_pgsql_handle_io_error` and `php_swoole_pgsql_check_liveness`):
//! a connection whose wait was interrupted fails with a connection error and
//! is never reused, and a hooked persistent connection resets without
//! blocking the event loop.
use crate::{blocking, has_io_error, take_io_error, wait_connect};
use pq_sys::{
    ConnStatusType, PGconn, PQconsumeInput, PQreset, PQresetPoll, PQresetStart, PQsetnonblocking,
    PQsocket, PQstatus,
};
use std::ffi::{c_char, c_int, c_void};
use swoole_rs_zend::sys::{swr_zend_pdo_driver_data, swr_zend_pdo_forget_persistent};

/// `SUCCESS` and `FAILURE`.
const SUCCESS: c_int = 0;
const FAILURE: c_int = -1;

/// The connection of `dbh`, a `pdo_dbh_t` whose driver data is a
/// `pdo_pgsql_db_handle` (its first field, `server`).
/// # Safety
/// `dbh` is a live pgsql `pdo_dbh_t`.
unsafe fn server(dbh: *mut c_void) -> *mut PGconn {
    // SAFETY: per the caller.
    let handle = unsafe { swr_zend_pdo_driver_data(dbh) };
    // SAFETY: `server` starts the handle.
    unsafe { *handle.cast::<*mut PGconn>() }
}

/// `php_swoole_pgsql_handle_io_error`: whether the connection's I/O was
/// interrupted, with its message in `message`. The first time, the socket is
/// shut down so the rest of the reply is never read as another's, and the
/// persistent pool lets go of the handle; PDO's own owners keep it until
/// their destructors `PQfinish` it.
/// # Safety
/// `dbh` is a live pgsql `pdo_dbh_t`; `message` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_pgsql_handle_io_error(
    dbh: *mut c_void,
    message: *mut *const c_char,
) -> bool {
    // SAFETY: per the caller.
    let conn = unsafe { server(dbh) };
    let Some((code, first)) = take_io_error(conn) else {
        return false;
    };
    if first {
        // SAFETY: a live connection.
        let fd = unsafe { PQsocket(conn) };
        if fd >= 0 {
            // SAFETY: the connection's own socket.
            unsafe { libc::shutdown(fd, libc::SHUT_RDWR) };
            // SAFETY: a live connection.
            unsafe { PQconsumeInput(conn) };
        }
        // SAFETY: per the caller.
        unsafe { swr_zend_pdo_forget_persistent(dbh) };
    }
    // SAFETY: per the caller.
    unsafe { *message = swoole_rs_error::swoole_strerror(code) };
    true
}

/// `php_swoole_pgsql_check_liveness`: an interrupted connection is dead; a
/// broken one resets, by polling with the hook on.
/// # Safety
/// `dbh` is a live pgsql `pdo_dbh_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_pgsql_check_liveness(dbh: *mut c_void) -> c_int {
    // SAFETY: per the caller.
    let conn = unsafe { server(dbh) };
    if has_io_error(conn) {
        return FAILURE;
    }
    // SAFETY: a live connection.
    let alive = unsafe { PQconsumeInput(conn) } != 0;
    // SAFETY: as above.
    if !alive || unsafe { PQstatus(conn) } == ConnStatusType::CONNECTION_BAD {
        if !blocking() {
            // SAFETY: as above.
            let started = unsafe { PQresetStart(conn) } != 0;
            // SAFETY: as above.
            if !started || unsafe { PQsocket(conn) } < 0 {
                return FAILURE;
            }
            // SAFETY: as above.
            unsafe { PQsetnonblocking(conn, 1) };
            // SAFETY: a live connection with a socket; the hook is on, in a coroutine.
            return if unsafe { wait_connect(conn, PQresetPoll) } {
                SUCCESS
            } else {
                FAILURE
            };
        }
        // SAFETY: as above.
        unsafe { PQreset(conn) };
    }
    // SAFETY: as above.
    if unsafe { PQstatus(conn) } == ConnStatusType::CONNECTION_OK {
        SUCCESS
    } else {
        FAILURE
    }
}
