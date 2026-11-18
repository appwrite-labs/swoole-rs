//! The driver's side of interrupted I/O (upstream's
//! `php_swoole_pgsql_handle_io_error` and `php_swoole_pgsql_check_liveness`):
//! a connection whose wait was interrupted fails with a connection error and
//! is never reused, and a hooked persistent connection resets without
//! blocking the event loop.
use crate::{
    Conn, PQreset, PQresetPoll, PQresetStart, blocking, driver, has_io_error, take_io_error,
    wait_connect,
};
use pq_sys::{ConnStatusType, PostgresPollingStatusType};
use std::{borrow::Cow, ffi::CStr};
use swoole_rs_zend::pdo::Dbh;

impl Conn {
    fn reset_poll(self) -> PostgresPollingStatusType {
        PQresetPoll(self)
    }

    fn reset_start(self) -> bool {
        PQresetStart(self) != 0
    }

    fn reset(self) {
        PQreset(self);
    }
}

// crates/zend-sys' shim (shim_pdo.c). `Dbh` is neither `Send` nor `Sync`, so it runs
// on the request's thread.
unsafe extern "C" {
    /// The persistent pool forgets the live handle if it holds it: it clears the
    /// pool entry's pointer before deleting the entry, so the entry's destructor
    /// leaves the handle, whose owners keep their references.
    safe fn swr_zend_pdo_forget_persistent(dbh: &Dbh);
}

/// `php_swoole_pgsql_handle_io_error`: the message of the connection's interrupted
/// I/O, if it was. The first time, the socket is shut down so the rest of the reply
/// is never read as another's, and the persistent pool lets go of the handle; PDO's
/// own owners keep it until their destructors `PQfinish` it.
pub(crate) fn io_error(dbh: &Dbh, conn: Conn) -> Option<Cow<'static, CStr>> {
    let (code, first) = take_io_error(conn)?;
    if first {
        let fd = conn.socket();
        if fd >= 0 {
            _ = swoole_rs_corebase::sys::shutdown(fd, libc::SHUT_RDWR);
            conn.consume_input();
        }
        swr_zend_pdo_forget_persistent(dbh);
    }
    Some(swoole_rs_error::strerror_text(
        code,
        swoole_rs_corebase::sys::strerror,
    ))
}

/// `zend_result`, as PDO reads `check_liveness`'s.
#[repr(i32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Liveness {
    Alive = 0,
    Dead = -1,
}

/// Whether `conn` is up: an interrupted connection is dead; a broken one resets,
/// by polling with the hook on.
fn liveness(conn: Conn) -> Liveness {
    if has_io_error(conn) {
        return Liveness::Dead;
    }
    if !conn.consume_input() || conn.status() == ConnStatusType::CONNECTION_BAD {
        if !blocking() {
            if !conn.reset_start() || conn.socket() < 0 {
                return Liveness::Dead;
            }
            conn.set_nonblocking(true);
            return if wait_connect(conn, Conn::reset_poll) {
                Liveness::Alive
            } else {
                Liveness::Dead
            };
        }
        conn.reset();
    }
    if conn.status() == ConnStatusType::CONNECTION_OK {
        Liveness::Alive
    } else {
        Liveness::Dead
    }
}

swoole_rs_zend::exports! {
/// `pdo_pgsql_check_liveness` (upstream's `php_swoole_pgsql_check_liveness`).
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_check_liveness(dbh: &Dbh) -> Liveness {
    driver::server(dbh).map_or(Liveness::Dead, liveness)
}
}
