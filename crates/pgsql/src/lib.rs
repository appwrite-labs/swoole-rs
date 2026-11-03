//! Swoole's coroutine libpq (upstream's `swoole_pgsql.cc`, built with
//! `--enable-swoole-pgsql`): the `PQconnectdb`, `PQprepare`, `PQexec*`
//! replacements its bundled `pdo_pgsql` driver calls. Each sends the request,
//! flushes and waits for the result on the connection's socket: with `poll(2)`
//! while blocking, otherwise by parking the coroutine (C++ lends that wait).
//!
//! The exports keep upstream's names: `php_swoole_pgsql.h` declares them and
//! the driver's macros call them.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

use pq_sys::{
    Oid, PGconn, PGresult, PQclear, PQconnectPoll, PQconnectStart, PQconsumeInput, PQflush,
    PQgetResult, PQisBusy, PQlibVersion, PQsendPrepare, PQsendQuery, PQsendQueryParams,
    PQsendQueryPrepared, PQsetnonblocking, PQsocket, PostgresPollingStatusType,
};
use std::{
    ffi::{c_char, c_int},
    ptr,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

/// `SW_EVENT_READ` and `SW_EVENT_WRITE`.
const READ: c_int = 1 << 9;
const WRITE: c_int = 1 << 10;

/// How long one wait lasts before libpq is asked again: on macOS `poll` may
/// miss readiness, so a short timeout lets the caller retry.
const POLL_TIMEOUT: f64 = 0.1;

/// `swr_v1_pgsql_ops`: what C++ lends at MINIT.
#[repr(C)]
pub struct Ops {
    /// Park the coroutine until `event` on `fd` or `timeout`: 1 ready, 0
    /// timed out, -1 failed.
    wait: unsafe extern "C" fn(fd: c_int, event: c_int, timeout: f64) -> c_int,
    /// Whether a coroutine is running.
    in_coroutine: unsafe extern "C" fn() -> bool,
}

static OPS: OnceLock<&'static Ops> = OnceLock::new();
static BLOCKING: AtomicBool = AtomicBool::new(true);

/// MINIT: keep C++'s operations.
/// # Safety
/// `ops` is a static table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_pgsql_init(ops: *const Ops) {
    // SAFETY: per the caller.
    if let Some(ops) = unsafe { ops.as_ref() } {
        OPS.get_or_init(|| ops);
    }
}

/// `swoole_pgsql_set_blocking`: the runtime hook flag.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_pgsql_set_blocking(blocking: bool) {
    BLOCKING.store(blocking, Ordering::Relaxed);
}

fn blocking() -> bool {
    BLOCKING.load(Ordering::Relaxed) || OPS.get().is_none()
}

/// libpq's version as `major.minor` (10 and later) or `major.minor.revision`.
fn version_text(version: c_int) -> String {
    let major = version / 10000;
    if major >= 10 {
        format!("{major}.{}", version % 10000)
    } else {
        format!("{major}.{}.{}", version / 100 % 100, version % 100)
    }
}

/// `swoole_libpq_version`: the version into `buf`, cut and NUL-terminated as
/// `snprintf` does.
/// # Safety
/// `buf` holds `len` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_libpq_version(buf: *mut c_char, len: usize) {
    if buf.is_null() || len == 0 {
        return;
    }
    // SAFETY: libpq's version.
    let text = version_text(unsafe { PQlibVersion() });
    // SAFETY: per the caller.
    let out = unsafe { std::slice::from_raw_parts_mut(buf.cast::<u8>(), len) };
    let n = text.len().min(len - 1);
    out[..n].copy_from_slice(&text.as_bytes()[..n]);
    out[n] = 0;
}

/// `translate_events_to_poll`.
fn to_poll(event: c_int) -> i16 {
    let mut events = 0;
    if event & READ != 0 {
        events |= libc::POLLIN;
    }
    if event & WRITE != 0 {
        events |= libc::POLLOUT;
    }
    events
}

/// `swoole_pgsql_socket_poll`: wait for `event` on the connection's socket;
/// 1 ready, 0 timed out, -1 failed. In a coroutine, `check_nonblock` keeps
/// reading until libpq has a whole result.
/// # Safety
/// `conn` is a live connection.
unsafe fn socket_poll(conn: *mut PGconn, event: c_int, check_nonblock: bool) -> c_int {
    // SAFETY: per the caller.
    let fd = unsafe { PQsocket(conn) };
    let ops = match OPS.get() {
        Some(ops) if !blocking() => ops,
        _ => {
            let mut poll = libc::pollfd {
                fd,
                events: to_poll(event),
                revents: 0,
            };
            #[expect(
                clippy::cast_possible_truncation,
                reason = "upstream's (int) of the milliseconds"
            )]
            let timeout = (POLL_TIMEOUT * 1000.0) as c_int;
            loop {
                // SAFETY: one live pollfd.
                let result = unsafe { libc::poll(&raw mut poll, 1, timeout) };
                if result >= 0
                    || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR)
                {
                    return result;
                }
            }
        }
    };
    // SAFETY: C++ parks the running coroutine.
    let mut result = unsafe { (ops.wait)(fd, event, POLL_TIMEOUT) };
    if !(check_nonblock && event == READ) {
        return result;
    }
    loop {
        // SAFETY: per the caller.
        if unsafe { PQconsumeInput(conn) } == 0 {
            // The wait's own outcome decides between a timeout and a failure.
            return if result == 1 { -1 } else { result };
        }
        // SAFETY: as above.
        if unsafe { PQisBusy(conn) } == 0 {
            return result;
        }
        // SAFETY: as above.
        result = unsafe { (ops.wait)(fd, event, POLL_TIMEOUT) };
    }
}

/// `swoole_pgsql_flush`: flush the outgoing request, waiting while it would block.
/// # Safety
/// `conn` is a live connection.
unsafe fn flush(conn: *mut PGconn) -> c_int {
    loop {
        // SAFETY: per the caller.
        if unsafe { socket_poll(conn, WRITE, false) } < 0 {
            return -1;
        }
        // SAFETY: as above.
        let flushed = unsafe { PQflush(conn) };
        if flushed != 1 {
            return flushed;
        }
    }
}

/// `swoole_pgsql_get_result`: wait for the result, then keep the last one.
/// # Safety
/// `conn` is a live connection.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_pgsql_get_result(conn: *mut PGconn) -> *mut PGresult {
    // SAFETY: per the caller.
    if unsafe { socket_poll(conn, READ, true) } < 0 {
        return ptr::null_mut();
    }
    let mut last = ptr::null_mut();
    loop {
        // SAFETY: as above.
        let result = unsafe { PQgetResult(conn) };
        if result.is_null() {
            return last;
        }
        // SAFETY: a result libpq returned, or null.
        unsafe { PQclear(last) };
        last = result;
    }
}

/// A sent request's result: flush, then wait for it.
/// # Safety
/// `conn` is a live connection.
unsafe fn finish(conn: *mut PGconn, sent: c_int) -> *mut PGresult {
    // SAFETY: per the caller.
    if sent == 0 || unsafe { flush(conn) } == -1 {
        return ptr::null_mut();
    }
    // SAFETY: as above.
    unsafe { swr_v1_pgsql_get_result(conn) }
}

/// `swoole_pgsql_connectdb`: `PQconnectStart`, then poll the handshake to its end.
/// # Safety
/// `conninfo` is a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_pgsql_connectdb(conninfo: *const c_char) -> *mut PGconn {
    // SAFETY: per the caller.
    let conn = unsafe { PQconnectStart(conninfo) };
    if conn.is_null() {
        return conn;
    }
    // SAFETY: a live connection.
    if unsafe { PQsocket(conn) } < 0 {
        return conn;
    }
    // SAFETY: C++ reads the running coroutine.
    let nonblocking = !blocking() && OPS.get().is_some_and(|ops| unsafe { (ops.in_coroutine)() });
    // SAFETY: a live connection.
    unsafe { PQsetnonblocking(conn, c_int::from(nonblocking)) };
    loop {
        // SAFETY: as above.
        let event = match unsafe { PQconnectPoll(conn) } {
            PostgresPollingStatusType::PGRES_POLLING_OK
            | PostgresPollingStatusType::PGRES_POLLING_FAILED => break,
            PostgresPollingStatusType::PGRES_POLLING_READING => READ,
            PostgresPollingStatusType::PGRES_POLLING_WRITING => WRITE,
            // Upstream aborts: libpq never answers ACTIVE here.
            PostgresPollingStatusType::PGRES_POLLING_ACTIVE => std::process::abort(),
        };
        // SAFETY: as above.
        if unsafe { socket_poll(conn, event, false) } < 0 {
            break;
        }
    }
    conn
}

/// `swoole_pgsql_prepare`: `PQsendPrepare`, then its result.
/// # Safety
/// As `PQprepare`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_pgsql_prepare(
    conn: *mut PGconn,
    stmt_name: *const c_char,
    query: *const c_char,
    n_params: c_int,
    param_types: *const Oid,
) -> *mut PGresult {
    // SAFETY: per the caller.
    let sent = unsafe { PQsendPrepare(conn, stmt_name, query, n_params, param_types) };
    // SAFETY: as above.
    unsafe { finish(conn, sent) }
}

/// `swoole_pgsql_exec_prepared`: `PQsendQueryPrepared`, then its result.
/// # Safety
/// As `PQexecPrepared`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_pgsql_exec_prepared(
    conn: *mut PGconn,
    stmt_name: *const c_char,
    n_params: c_int,
    param_values: *const *const c_char,
    param_lengths: *const c_int,
    param_formats: *const c_int,
    result_format: c_int,
) -> *mut PGresult {
    // SAFETY: per the caller.
    let sent = unsafe {
        PQsendQueryPrepared(
            conn,
            stmt_name,
            n_params,
            param_values,
            param_lengths,
            param_formats,
            result_format,
        )
    };
    // SAFETY: as above.
    unsafe { finish(conn, sent) }
}

/// `swoole_pgsql_exec`: `PQsendQuery`, then its result.
/// # Safety
/// As `PQexec`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_pgsql_exec(
    conn: *mut PGconn,
    query: *const c_char,
) -> *mut PGresult {
    // SAFETY: per the caller.
    let sent = unsafe { PQsendQuery(conn, query) };
    // SAFETY: as above.
    unsafe { finish(conn, sent) }
}

/// `swoole_pgsql_exec_params`: `PQsendQueryParams`, then its result.
/// # Safety
/// As `PQexecParams`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_pgsql_exec_params(
    conn: *mut PGconn,
    command: *const c_char,
    n_params: c_int,
    param_types: *const Oid,
    param_values: *const *const c_char,
    param_lengths: *const c_int,
    param_formats: *const c_int,
    result_format: c_int,
) -> *mut PGresult {
    // SAFETY: per the caller.
    let sent = unsafe {
        PQsendQueryParams(
            conn,
            command,
            n_params,
            param_types,
            param_values,
            param_lengths,
            param_formats,
            result_format,
        )
    };
    // SAFETY: as above.
    unsafe { finish(conn, sent) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_print_as_libpq_numbers_them() {
        assert_eq!(version_text(170_002), "17.2");
        assert_eq!(version_text(100_000), "10.0");
        assert_eq!(version_text(90_624), "9.6.24");
    }

    #[test]
    fn events_map_to_poll() {
        assert_eq!(to_poll(READ), libc::POLLIN);
        assert_eq!(to_poll(WRITE), libc::POLLOUT);
        assert_eq!(to_poll(READ | WRITE), libc::POLLIN | libc::POLLOUT);
    }
}
