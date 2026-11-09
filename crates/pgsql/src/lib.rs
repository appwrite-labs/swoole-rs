//! Swoole's coroutine libpq (upstream's `swoole_pgsql.cc`, built with
//! `--enable-swoole-pgsql`): the `PQconnectdb`, `PQprepare`, `PQexec*`
//! replacements its bundled `pdo_pgsql` driver calls. Each sends the request,
//! flushes and waits for the result on the connection's socket: with `poll(2)`
//! while blocking, otherwise by parking the coroutine (`System::wait_event`).
//! With `php`, MINIT ([`minit`]) registers the driver with PDO.
//!
//! The exports keep upstream's names: `native/hooks/php_swoole_pgsql.h` declares them and
//! the driver's macros call them.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

#[cfg(feature = "php")]
pub mod minit;
#[cfg(feature = "php")]
pub mod pdo;

use pq_sys::{
    ConnStatusType, Oid, PGconn, PGresult, PQclear, PQconnectPoll, PQconnectStart, PQconninfo,
    PQconninfoFree, PQconsumeInput, PQfinish, PQflush, PQgetResult, PQisBusy, PQlibVersion,
    PQsendPrepare, PQsendQuery, PQsendQueryParams, PQsendQueryPrepared, PQsetnonblocking, PQsocket,
    PQstatus, PostgresPollingStatusType,
};
use std::{
    cell::RefCell,
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

/// `SW_EVENT_READ` and `SW_EVENT_WRITE`.
const READ: c_int = 1 << 9;
const WRITE: c_int = 1 << 10;

/// How long one wait lasts before libpq is asked again: on macOS `poll` may
/// miss readiness, so a short timeout lets the caller retry.
const POLL_TIMEOUT: f64 = 0.1;

static BLOCKING: AtomicBool = AtomicBool::new(true);

/// `SW_ERROR_CO_TIMEDOUT`: the last error a timed-out wait leaves.
const CO_TIMEDOUT: c_int = 10018;

thread_local! {
    /// `pgsql_io_errors`: each connection whose wait was interrupted, with the
    /// error and whether PDO has shut it down for it. Only interrupted I/O is
    /// recorded; PDO owns the connection's disposal and its eviction from the
    /// persistent pool. A thread holds few connections, so a list will do.
    static IO_ERRORS: RefCell<Vec<(*mut PGconn, c_int, bool)>> = const { RefCell::new(Vec::new()) };
}

/// The connection's interrupted I/O, if any: its error, and whether this is
/// the first time it is asked for (the caller then shuts the connection down).
#[cfg(feature = "php")]
fn take_io_error(conn: *mut PGconn) -> Option<(c_int, bool)> {
    IO_ERRORS.with_borrow_mut(|errors| {
        let (_, code, handled) = errors.iter_mut().find(|(c, _, _)| *c == conn)?;
        Some((*code, !std::mem::replace(handled, true)))
    })
}

#[cfg(feature = "php")]
fn has_io_error(conn: *mut PGconn) -> bool {
    IO_ERRORS.with_borrow(|errors| errors.iter().any(|(c, _, _)| *c == conn))
}

// SAFETY: crates/coroutine's.
unsafe extern "C" {
    fn swr_v1_coroutine_current() -> *mut c_void;
}

/// `swoole_pgsql_set_blocking`: the runtime hook flag.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_pgsql_set_blocking(blocking: bool) {
    BLOCKING.store(blocking, Ordering::Relaxed);
}

fn blocking() -> bool {
    BLOCKING.load(Ordering::Relaxed)
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
    let mut n = 0;
    for (slot, &byte) in out.iter_mut().zip(text.as_bytes().iter().take(len - 1)) {
        *slot = byte;
        n += 1;
    }
    if let Some(terminator) = out.get_mut(n) {
        *terminator = 0;
    }
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
/// 1 ready, 0 timed out, -1 failed. A failed coroutine wait (cancelled, or
/// refused by the reactor) is recorded against the connection.
/// # Safety
/// `conn` is a live connection; with the hook on, in a coroutine.
unsafe fn socket_poll(conn: *mut PGconn, event: c_int) -> c_int {
    // SAFETY: per the caller.
    let fd = unsafe { PQsocket(conn) };
    if blocking() {
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
        // SAFETY: one live pollfd.
        let result = unsafe { libc::poll(&raw mut poll, 1, timeout) };
        let interrupted =
            result < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR);
        return if interrupted { 0 } else { result };
    }
    // SAFETY: per the caller, with the thread's reactor running.
    if unsafe { swoole_rs_hooks::wait_event::system_wait_event(fd, event, POLL_TIMEOUT) } >= 0 {
        return 1;
    }
    let code = swoole_rs_corebase::log::last_error();
    if code == CO_TIMEDOUT {
        return 0;
    }
    IO_ERRORS.with_borrow_mut(|errors| {
        if !errors.iter().any(|(c, _, _)| *c == conn) {
            errors.push((conn, if code == 0 { libc::EIO } else { code }, false));
        }
    });
    -1
}

/// `swoole_pgsql_flush`: flush the outgoing request, waiting while it would block.
/// # Safety
/// As [`socket_poll`].
unsafe fn flush(conn: *mut PGconn) -> c_int {
    loop {
        // SAFETY: per the caller.
        let flushed = unsafe { PQflush(conn) };
        if flushed != 1 {
            return flushed;
        }
        // SAFETY: as above.
        if unsafe { socket_poll(conn, WRITE) } < 0 {
            return -1;
        }
    }
}

/// `swoole_pgsql_wait_result`: read until libpq holds a whole result.
/// # Safety
/// As [`socket_poll`].
unsafe fn wait_result(conn: *mut PGconn) -> bool {
    // SAFETY: per the caller.
    while unsafe { PQisBusy(conn) } != 0 {
        // SAFETY: as above.
        if unsafe { socket_poll(conn, READ) } < 0 {
            return false;
        }
        // SAFETY: as above.
        if unsafe { PQconsumeInput(conn) } == 0 {
            return false;
        }
    }
    true
}

/// `swoole_pgsql_get_result`: wait for the results and keep the last one.
/// Without the hook one poll precedes `PQgetResult`, which may then block.
/// # Safety
/// As [`socket_poll`].
unsafe fn get_result(conn: *mut PGconn) -> *mut PGresult {
    let blocking = blocking();
    // SAFETY: per the caller.
    if blocking && unsafe { socket_poll(conn, READ) } < 0 {
        return ptr::null_mut();
    }
    let mut last = ptr::null_mut();
    // SAFETY: as above.
    while blocking || unsafe { wait_result(conn) } {
        // SAFETY: as above.
        let result = unsafe { PQgetResult(conn) };
        if result.is_null() {
            return last;
        }
        // SAFETY: a result libpq returned, or null.
        unsafe { PQclear(last) };
        last = result;
    }
    // SAFETY: as above.
    unsafe { PQclear(last) };
    ptr::null_mut()
}

/// A sent request's result: flush, then wait for it.
/// # Safety
/// As [`socket_poll`].
unsafe fn finish(conn: *mut PGconn, sent: c_int) -> *mut PGresult {
    // SAFETY: per the caller.
    if sent == 0 || unsafe { flush(conn) } == -1 {
        return ptr::null_mut();
    }
    // SAFETY: as above.
    unsafe { get_result(conn) }
}

/// `strtol(value, NULL, 10)`, saturating.
fn strtol(value: &[u8]) -> i64 {
    let text = value.trim_ascii_start();
    let (negative, digits) = match text.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, text),
    };
    let mut n: i64 = 0;
    for &digit in digits.iter().take_while(|b| b.is_ascii_digit()) {
        n = n.saturating_mul(10).saturating_add(i64::from(digit - b'0'));
    }
    if negative { -n } else { n }
}

/// The connect deadline from a connection's options: `connect_timeout` in
/// seconds (0 for none) and the number of targets that share it. libpq
/// before 17 treats a timeout of 1 as 2.
fn connect_budget<'a>(
    options: impl IntoIterator<Item = (&'a [u8], &'a [u8])>,
    lib_version: c_int,
) -> (u64, u64) {
    let count = |value: &[u8]| value.split(|&b| b == b',').count() as u64;
    let mut timeout = 0;
    let (mut hosts, mut addresses) = (1, 0);
    for (keyword, value) in options {
        if value.is_empty() {
            continue;
        }
        match keyword {
            b"connect_timeout" => timeout = strtol(value),
            b"host" => hosts = count(value),
            b"hostaddr" => addresses = count(value),
            _ => {}
        }
    }
    if timeout == 1 && lib_version < 170_000 {
        timeout = 2;
    }
    let targets = if addresses == 0 { hosts } else { addresses };
    (u64::try_from(timeout).unwrap_or(0), targets)
}

/// [`connect_budget`] of a live connection.
/// # Safety
/// `conn` is a live connection.
unsafe fn conn_budget(conn: *mut PGconn) -> (u64, u64) {
    // SAFETY: per the caller.
    let options = unsafe { PQconninfo(conn) };
    let mut pairs = Vec::new();
    let mut option = options;
    // SAFETY: libpq's array, ended by a null keyword.
    while !option.is_null() && !unsafe { (*option).keyword }.is_null() {
        // SAFETY: a live option.
        let entry = unsafe { *option };
        if !entry.val.is_null() {
            // SAFETY: NUL-terminated strings libpq owns until PQconninfoFree.
            let keyword = unsafe { CStr::from_ptr(entry.keyword) }.to_bytes().to_vec();
            // SAFETY: as above.
            let value = unsafe { CStr::from_ptr(entry.val) }.to_bytes().to_vec();
            pairs.push((keyword, value));
        }
        // SAFETY: the array continues to its terminator.
        option = unsafe { option.add(1) };
    }
    if !options.is_null() {
        // SAFETY: the array PQconninfo returned.
        unsafe { PQconninfoFree(options) };
    }
    // SAFETY: libpq's version.
    let version = unsafe { PQlibVersion() };
    connect_budget(
        pairs.iter().map(|(k, v)| (k.as_slice(), v.as_slice())),
        version,
    )
}

/// `swoole_pgsql_wait_connect`: drive `poll_connection` to its end. libpq
/// leaves `connect_timeout` to the caller here; the whole target list gets it
/// once per target, so libpq can still fail over.
/// # Safety
/// `conn` is a live connection with a socket.
unsafe fn wait_connect(
    conn: *mut PGconn,
    poll_connection: unsafe extern "C" fn(*mut PGconn) -> PostgresPollingStatusType,
) -> bool {
    // SAFETY: per the caller.
    let (timeout, targets) = unsafe { conn_budget(conn) };
    let start = Instant::now();
    let mut status = PostgresPollingStatusType::PGRES_POLLING_WRITING;
    loop {
        let event = match status {
            PostgresPollingStatusType::PGRES_POLLING_READING => READ,
            PostgresPollingStatusType::PGRES_POLLING_WRITING => WRITE,
            other => {
                swoole_rs_corebase::log::warning(
                    c"swoole_pgsql_wait_connect",
                    format_args!("unexpected PQconnectPoll() status[{}]", other as c_int),
                );
                return false;
            }
        };
        // SAFETY: as above.
        let polled = unsafe { socket_poll(conn, event) };
        if polled < 0 || (timeout > 0 && start.elapsed().as_secs() / targets >= timeout) {
            break;
        }
        // SAFETY: as above.
        let state = unsafe { PQstatus(conn) };
        // libpq sends its first packet from these states and assumes the socket is writable.
        if polled == 0
            && matches!(
                state,
                ConnStatusType::CONNECTION_STARTED | ConnStatusType::CONNECTION_MADE
            )
        {
            continue;
        }
        // SAFETY: as above.
        status = unsafe { poll_connection(conn) };
        if matches!(
            status,
            PostgresPollingStatusType::PGRES_POLLING_OK
                | PostgresPollingStatusType::PGRES_POLLING_FAILED
        ) {
            break;
        }
    }
    // SAFETY: as above.
    let status = unsafe { PQstatus(conn) };
    status == ConnStatusType::CONNECTION_OK
}

/// `swoole_pgsql_close_prepared`: `PQsendClosePrepared`, then its result.
/// Only a driver built against libpq 17 or later (`HAVE_PQCLOSEPREPARED`)
/// calls it, so the call is looked up rather than linked: an older libpq
/// still loads.
/// # Safety
/// As `PQclosePrepared`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_pgsql_close_prepared(
    conn: *mut PGconn,
    stmt_name: *const c_char,
) -> *mut PGresult {
    type Send = unsafe extern "C" fn(*mut PGconn, *const c_char) -> c_int;
    // SAFETY: a NUL-terminated name.
    let send = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"PQsendClosePrepared".as_ptr()) };
    if send.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: libpq's `PQsendClosePrepared`, of this type.
    let send = unsafe { std::mem::transmute::<*mut c_void, Send>(send) };
    // SAFETY: per the caller.
    if unsafe { send(conn, stmt_name) } == 0 {
        return ptr::null_mut();
    }
    // SAFETY: as above.
    unsafe { get_result(conn) }
}

/// `swoole_pgsql_connectdb`: `PQconnectStart`, then poll the handshake to its
/// end or its `connect_timeout`.
/// # Safety
/// `conninfo` is a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_pgsql_connectdb(conninfo: *const c_char) -> *mut PGconn {
    // SAFETY: per the caller.
    let conn = unsafe { PQconnectStart(conninfo) };
    // SAFETY: a live connection.
    if conn.is_null() || unsafe { PQsocket(conn) } < 0 {
        return conn;
    }
    // SAFETY: reads the running coroutine.
    let nonblocking = !blocking() && !unsafe { swr_v1_coroutine_current() }.is_null();
    // SAFETY: a live connection.
    unsafe { PQsetnonblocking(conn, c_int::from(nonblocking)) };
    // SAFETY: as above.
    unsafe { wait_connect(conn, PQconnectPoll) };
    conn
}

/// `swoole_pgsql_finish`: forget the connection's interrupted I/O, then `PQfinish`.
/// # Safety
/// As `PQfinish`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_pgsql_finish(conn: *mut PGconn) {
    IO_ERRORS.with_borrow_mut(|errors| errors.retain(|(c, _, _)| *c != conn));
    // SAFETY: per the caller.
    unsafe { PQfinish(conn) };
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
    fn connect_budget_counts_targets_and_libpq_rounding() {
        let opts = |list: &'static [(&'static str, &'static str)]| {
            list.iter().map(|(k, v)| (k.as_bytes(), v.as_bytes()))
        };
        assert_eq!(connect_budget(opts(&[]), 170_000), (0, 1));
        assert_eq!(
            connect_budget(opts(&[("connect_timeout", "3")]), 170_000),
            (3, 1)
        );
        assert_eq!(
            connect_budget(opts(&[("connect_timeout", "1")]), 160_000),
            (2, 1)
        );
        assert_eq!(
            connect_budget(opts(&[("connect_timeout", "1")]), 170_000),
            (1, 1)
        );
        assert_eq!(
            connect_budget(opts(&[("connect_timeout", "-5")]), 170_000),
            (0, 1)
        );
        assert_eq!(
            connect_budget(
                opts(&[("connect_timeout", "2"), ("host", "a,b,c")]),
                170_000
            ),
            (2, 3)
        );
        assert_eq!(
            connect_budget(
                opts(&[
                    ("host", "a,b"),
                    ("hostaddr", "127.0.0.1"),
                    ("connect_timeout", "")
                ]),
                170_000
            ),
            (0, 1)
        );
    }

    #[test]
    fn strtol_reads_the_leading_number() {
        assert_eq!(strtol(b" 12s"), 12);
        assert_eq!(strtol(b"+7"), 7);
        assert_eq!(strtol(b"-3"), -3);
        assert_eq!(strtol(b"x"), 0);
    }

    #[test]
    fn events_map_to_poll() {
        assert_eq!(to_poll(READ), libc::POLLIN);
        assert_eq!(to_poll(WRITE), libc::POLLOUT);
        assert_eq!(to_poll(READ | WRITE), libc::POLLIN | libc::POLLOUT);
    }
}
