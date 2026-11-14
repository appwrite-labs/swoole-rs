//! The driver's side of interrupted I/O (upstream's
//! `php_swoole_pgsql_handle_io_error` and `php_swoole_pgsql_check_liveness`):
//! a connection whose wait was interrupted fails with a connection error and
//! is never reused, and a hooked persistent connection resets without
//! blocking the event loop.
use crate::{Conn, blocking, has_io_error, take_io_error, wait_connect};
use pq_sys::{ConnStatusType, PQreset, PQresetPoll, PQresetStart, PostgresPollingStatusType};
use std::ffi::{c_char, c_int, c_void};
use swoole_rs_zend::sys::{swr_zend_pdo_driver_data, swr_zend_pdo_forget_persistent};

impl Conn {
    fn reset_poll(self) -> PostgresPollingStatusType {
        // SAFETY: a live connection.
        unsafe { PQresetPoll(self.0) }
    }

    fn reset_start(self) -> bool {
        // SAFETY: as above.
        unsafe { PQresetStart(self.0) != 0 }
    }

    fn reset(self) {
        // SAFETY: as above.
        unsafe { PQreset(self.0) };
    }
}

/// `SUCCESS` and `FAILURE`.
const SUCCESS: c_int = 0;
const FAILURE: c_int = -1;

/// A live pgsql `pdo_dbh_t` the driver handed over: its driver data is a
/// `pdo_pgsql_db_handle`, whose first field is the connection.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Dbh(*mut c_void);

impl Dbh {
    fn server(self) -> Conn {
        // SAFETY: a live pgsql `pdo_dbh_t`.
        let handle = unsafe { swr_zend_pdo_driver_data(self.0) };
        // SAFETY: `server` starts the handle.
        unsafe { *handle.cast::<Conn>() }
    }
}

/// `php_swoole_pgsql_handle_io_error`: whether the connection's I/O was
/// interrupted, with its message in `message`. The first time, the socket is
/// shut down so the rest of the reply is never read as another's, and the
/// persistent pool lets go of the handle; PDO's own owners keep it until
/// their destructors `PQfinish` it.
/// # Safety
/// `message` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_pgsql_handle_io_error(
    dbh: Dbh,
    message: *mut *const c_char,
) -> bool {
    let conn = dbh.server();
    let Some((code, first)) = take_io_error(conn) else {
        return false;
    };
    if first {
        let fd = conn.socket();
        if fd >= 0 {
            swoole_rs_corebase::sys::shutdown(fd, libc::SHUT_RDWR);
            conn.consume_input();
        }
        // SAFETY: a live `pdo_dbh_t`.
        unsafe { swr_zend_pdo_forget_persistent(dbh.0) };
    }
    // SAFETY: per the caller.
    unsafe { *message = swoole_rs_error_ffi::swoole_strerror(code) };
    true
}

/// `php_swoole_pgsql_check_liveness`: an interrupted connection is dead; a
/// broken one resets, by polling with the hook on.
#[unsafe(no_mangle)]
pub extern "C" fn php_swoole_pgsql_check_liveness(dbh: Dbh) -> c_int {
    let conn = dbh.server();
    if has_io_error(conn) {
        return FAILURE;
    }
    if !conn.consume_input() || conn.status() == ConnStatusType::CONNECTION_BAD {
        if !blocking() {
            if !conn.reset_start() || conn.socket() < 0 {
                return FAILURE;
            }
            conn.set_nonblocking(true);
            return if wait_connect(conn, Conn::reset_poll) {
                SUCCESS
            } else {
                FAILURE
            };
        }
        conn.reset();
    }
    if conn.status() == ConnStatusType::CONNECTION_OK {
        SUCCESS
    } else {
        FAILURE
    }
}
