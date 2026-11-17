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

use pq_sys::{ConnStatusType, Oid, PGconn, PGresult, PostgresPollingStatusType};
use std::{
    cell::Cell,
    ffi::{c_char, c_int, c_void},
    ptr::{self, NonNull},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};
use swoole_rs_corebase::{CCast, CellExt, sys::Text};
use swoole_rs_hooks::wait_event::{Events, to_poll, wait_event};

/// How long one wait lasts before libpq is asked again: on macOS `poll` may
/// miss readiness, so a short timeout lets the caller retry.
const POLL_TIMEOUT: f64 = 0.1;

static BLOCKING: AtomicBool = AtomicBool::new(true);

/// `SW_ERROR_CO_TIMEDOUT`: the last error a timed-out wait leaves.
const CO_TIMEDOUT: c_int = 10018;

/// A connection the driver handed over: live, as libpq's own calls require,
/// for as long as the value is used. The exports take it in place of
/// `PGconn *`.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Conn(*mut PGconn);

/// The connection the driver gives up to `PQfinish`: null, or live and used by nothing
/// after the call. Only [`swoole_pgsql_finish`]'s parameter makes one, where the C
/// caller's contract stands for the constructor; it is not `Copy`, so it is finished
/// once.
#[repr(transparent)]
pub struct OwnedConn(*mut PGconn);

impl OwnedConn {
    /// The connection, live until the owner is finished.
    fn conn(&self) -> Conn {
        Conn(self.0)
    }
}

/// The options array `PQconninfo` made, owned until `PQconninfoFree` takes it.
#[repr(transparent)]
struct Conninfo(NonNull<ConnOption<'static>>);

/// `PQconninfoOption`, its strings typed as what libpq makes them: null, or
/// NUL-terminated and live as long as the array. A null keyword ends the array.
#[repr(C)]
struct ConnOption<'a> {
    keyword: Option<Text<'a>>,
    envvar: *const c_char,
    compiled: *const c_char,
    val: Option<Text<'a>>,
    label: *const c_char,
    dispchar: *const c_char,
    dispsize: c_int,
}
const _: () = assert!(
    size_of::<ConnOption<'static>>() == size_of::<pq_sys::PQconninfoOption>(),
    "PQconninfoOption's layout"
);

/// A result libpq returned, owned until `PQclear` takes it or it goes back to the
/// driver ([`PgResult::into_raw`]).
#[repr(transparent)]
struct PgResult(NonNull<PGresult>);

impl PgResult {
    /// The driver's `PGresult *`, which it clears.
    fn into_raw(result: Option<Self>) -> *mut PGresult {
        result.map_or(ptr::null_mut(), |result| result.0.as_ptr())
    }
}

// libpq's own declarations, with the connection typed as the handle it is
// (`Conn` has a `PGconn *`'s layout). The `safe` ones take only a live connection
// and plain values, read or drive that connection alone, and return raw pointers
// the caller dereferences or frees in its own unsafe block. `Conn` is neither
// `Send` nor `Sync`, so the calls stay on the connection's thread.
unsafe extern "C" {
    /// libpq's compile-time version: no arguments, no state.
    #[link_name = "PQlibVersion"]
    safe fn lib_version() -> c_int;
    safe fn PQsocket(conn: Conn) -> c_int;
    safe fn PQstatus(conn: Conn) -> ConnStatusType;
    safe fn PQconsumeInput(conn: Conn) -> c_int;
    safe fn PQisBusy(conn: Conn) -> c_int;
    safe fn PQsetnonblocking(conn: Conn, on: c_int) -> c_int;
    safe fn PQconnectPoll(conn: Conn) -> PostgresPollingStatusType;
    safe fn PQflush(conn: Conn) -> c_int;
    /// The next result, which the caller owns, or none when the request is done.
    safe fn PQgetResult(conn: Conn) -> Option<PgResult>;
    /// Free the result: it takes the owner.
    safe fn PQclear(result: PgResult);
    /// A new options array, or none when libpq can't allocate one.
    safe fn PQconninfo(conn: Conn) -> Option<Conninfo>;
    /// Free the array, which no borrow of its entries outlives: it takes the owner.
    safe fn PQconninfoFree(options: Conninfo);
    /// Close and free the connection (nothing for a null one): it takes the owner.
    safe fn PQfinish(conn: OwnedConn);
    /// A new connection, or null: libpq copies what it needs of `conninfo`, which it
    /// only reads, and runs no callback.
    safe fn PQconnectStart(conninfo: Text<'_>) -> *mut PGconn;
    /// Queue `query`, which libpq copies into the connection's output buffer. It
    /// parses no input, so it runs no notice callback.
    safe fn PQsendQuery(conn: Conn, query: Text<'_>) -> c_int;
    #[cfg(feature = "php")]
    safe fn PQresetStart(conn: Conn) -> c_int;
    #[cfg(feature = "php")]
    safe fn PQresetPoll(conn: Conn) -> PostgresPollingStatusType;
    #[cfg(feature = "php")]
    safe fn PQreset(conn: Conn);
}

impl Conn {
    fn socket(self) -> c_int {
        PQsocket(self)
    }

    fn status(self) -> ConnStatusType {
        PQstatus(self)
    }

    fn consume_input(self) -> bool {
        PQconsumeInput(self) != 0
    }

    fn is_busy(self) -> bool {
        PQisBusy(self) != 0
    }

    fn set_nonblocking(self, on: bool) {
        PQsetnonblocking(self, c_int::from(on));
    }

    fn connect_poll(self) -> PostgresPollingStatusType {
        PQconnectPoll(self)
    }

    /// The options set on the connection, as keyword and value bytes.
    fn options(self) -> Vec<(Vec<u8>, Vec<u8>)> {
        let Some(options) = PQconninfo(self) else {
            return Vec::new();
        };
        let pairs = options.pairs();
        PQconninfoFree(options);
        pairs
    }
}

impl Conninfo {
    /// Each option with a value, as keyword and value bytes.
    fn pairs<'s>(&'s self) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut pairs = Vec::new();
        let mut option = self.0.as_ptr().cast::<ConnOption<'s>>();
        loop {
            // SAFETY: an entry of libpq's array, at or before the null keyword that ends
            // it, live while the array is borrowed.
            let entry = unsafe { &*option };
            let Some(keyword) = entry.keyword else {
                break;
            };
            if let Some(value) = entry.val {
                pairs.push((
                    keyword.get().to_bytes().to_vec(),
                    value.get().to_bytes().to_vec(),
                ));
            }
            // The array continues to its terminator, where the loop ends.
            option = option.wrapping_add(1);
        }
        pairs
    }
}

thread_local! {
    /// `pgsql_io_errors`: each connection whose wait was interrupted, with the
    /// error and whether PDO has shut it down for it. Only interrupted I/O is
    /// recorded; PDO owns the connection's disposal and its eviction from the
    /// persistent pool. A thread holds few connections, so a list will do.
    static IO_ERRORS: Cell<Vec<(Conn, c_int, bool)>> = const { Cell::new(Vec::new()) };
}

/// `f` of the interrupted I/O list, taken out and put back after: `f` only
/// searches and edits the list, calling neither libpq nor PHP.
fn with_io_errors<R>(f: impl FnOnce(&mut Vec<(Conn, c_int, bool)>) -> R) -> R {
    IO_ERRORS.with(|errors| errors.with_taken(f))
}

/// The connection's interrupted I/O, if any: its error, and whether this is
/// the first time it is asked for (the caller then shuts the connection down).
#[cfg(feature = "php")]
fn take_io_error(conn: Conn) -> Option<(c_int, bool)> {
    with_io_errors(|errors| {
        let (_, code, handled) = errors.iter_mut().find(|(c, _, _)| *c == conn)?;
        Some((*code, !std::mem::replace(handled, true)))
    })
}

#[cfg(feature = "php")]
fn has_io_error(conn: Conn) -> bool {
    with_io_errors(|errors| errors.iter().any(|(c, _, _)| *c == conn))
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

/// libpq's version, as `phpinfo()` shows it.
#[must_use]
pub fn version() -> String {
    version_text(lib_version())
}

/// `swoole_pgsql_socket_poll`: wait for `event` on the connection's socket;
/// whether it became ready before the timeout, or `None` when the wait failed. A
/// failed coroutine wait (cancelled, or refused by the reactor) is recorded against
/// the connection.
fn socket_poll(conn: Conn, event: Events) -> Option<bool> {
    let fd = conn.socket();
    if blocking() {
        let mut poll = libc::pollfd {
            fd,
            // `translate_events_to_poll`.
            events: to_poll(event),
            revents: 0,
        };
        let timeout = (POLL_TIMEOUT * 1000.0).c_cast::<c_int>();
        let result = swoole_rs_corebase::sys::poll(core::slice::from_mut(&mut poll), timeout);
        return match result {
            Ok(ready) => Some(ready > 0),
            Err(error) if error.raw_os_error() == Some(libc::EINTR) => Some(false),
            Err(_) => None,
        };
    }
    // The hook is on, so a coroutine runs the request, as upstream requires.
    if wait_event(fd, event, POLL_TIMEOUT).is_ok() {
        return Some(true);
    }
    let code = swoole_rs_corebase::log::last_error();
    if code == CO_TIMEDOUT {
        return Some(false);
    }
    with_io_errors(|errors| {
        if !errors.iter().any(|(c, _, _)| *c == conn) {
            errors.push((conn, if code == 0 { libc::EIO } else { code }, false));
        }
    });
    None
}

/// `swoole_pgsql_flush`: flush the outgoing request, waiting while it would block;
/// whether all of it went.
fn flush(conn: Conn) -> bool {
    loop {
        match PQflush(conn) {
            1 if socket_poll(conn, Events::WRITE).is_some() => {}
            0 => return true,
            _ => return false,
        }
    }
}

/// `swoole_pgsql_wait_result`: read until libpq holds a whole result.
fn wait_result(conn: Conn) -> bool {
    while conn.is_busy() {
        if socket_poll(conn, Events::READ).is_none() || !conn.consume_input() {
            return false;
        }
    }
    true
}

/// `swoole_pgsql_get_result`: wait for the results and keep the last one.
/// Without the hook one poll precedes `PQgetResult`, which may then block.
fn get_result(conn: Conn) -> *mut PGresult {
    let blocking = blocking();
    if blocking && socket_poll(conn, Events::READ).is_none() {
        return ptr::null_mut();
    }
    let mut last = None;
    while blocking || wait_result(conn) {
        let Some(result) = PQgetResult(conn) else {
            return PgResult::into_raw(last);
        };
        if let Some(previous) = last.replace(result) {
            PQclear(previous);
        }
    }
    if let Some(previous) = last {
        PQclear(previous);
    }
    ptr::null_mut()
}

/// A sent request's result: flush, then wait for it.
fn finish(conn: Conn, sent: c_int) -> *mut PGresult {
    if sent == 0 || !flush(conn) {
        return ptr::null_mut();
    }
    get_result(conn)
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
            b"connect_timeout" => timeout = swoole_rs_corebase::strtol(value).0,
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

/// `swoole_pgsql_wait_connect`: drive `poll_connection` to its end. libpq
/// leaves `connect_timeout` to the caller here; the whole target list gets it
/// once per target, so libpq can still fail over.
fn wait_connect(conn: Conn, poll_connection: fn(Conn) -> PostgresPollingStatusType) -> bool {
    let options = conn.options();
    let (timeout, targets) = connect_budget(
        options.iter().map(|(k, v)| (k.as_slice(), v.as_slice())),
        lib_version(),
    );
    let start = Instant::now();
    let mut status = PostgresPollingStatusType::PGRES_POLLING_WRITING;
    loop {
        let event = match status {
            PostgresPollingStatusType::PGRES_POLLING_READING => Events::READ,
            PostgresPollingStatusType::PGRES_POLLING_WRITING => Events::WRITE,
            other => {
                swoole_rs_corebase::log::warning(
                    c"swoole_pgsql_wait_connect",
                    format_args!("unexpected PQconnectPoll() status[{}]", other as c_int),
                );
                return false;
            }
        };
        let Some(ready) = socket_poll(conn, event) else {
            break;
        };
        if timeout > 0 && start.elapsed().as_secs() / targets >= timeout {
            break;
        }
        // libpq sends its first packet from these states and assumes the socket is writable.
        if !ready
            && matches!(
                conn.status(),
                ConnStatusType::CONNECTION_STARTED | ConnStatusType::CONNECTION_MADE
            )
        {
            continue;
        }
        status = poll_connection(conn);
        if matches!(
            status,
            PostgresPollingStatusType::PGRES_POLLING_OK
                | PostgresPollingStatusType::PGRES_POLLING_FAILED
        ) {
            break;
        }
    }
    conn.status() == ConnStatusType::CONNECTION_OK
}

/// `swoole_pgsql_close_prepared`: `PQsendClosePrepared`, then its result.
/// Only a driver built against libpq 17 or later (`HAVE_PQCLOSEPREPARED`)
/// calls it, so the call is looked up rather than linked: an older libpq
/// still loads.
/// `PQclosePrepared`'s whole contract is the live connection and the name's string,
/// which the types carry.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_pgsql_close_prepared(conn: Conn, stmt_name: Text<'_>) -> *mut PGresult {
    /// `RTLD_DEFAULT`, a handle `dlsym` always takes.
    #[repr(transparent)]
    struct Global(*mut c_void);
    /// The NUL-terminated name `PQsendClosePrepared`: the only value made below.
    #[repr(transparent)]
    struct Name(*const c_char);
    /// As `PQsendQuery`, it asks for a live connection and a string, queues the
    /// request and runs no callback, so it is safe to call with both.
    type Send = extern "C" fn(Conn, Text<'_>) -> c_int;
    unsafe extern "C" {
        /// libc's lookup, which reads the name up to its NUL: libpq's
        /// `PQsendClosePrepared`, the function `Send` declares, or null.
        #[link_name = "dlsym"]
        safe fn lookup(handle: Global, name: Name) -> Option<Send>;
    }
    let Some(send) = lookup(
        Global(libc::RTLD_DEFAULT),
        Name(c"PQsendClosePrepared".as_ptr()),
    ) else {
        return ptr::null_mut();
    };
    if send(conn, stmt_name) == 0 {
        return ptr::null_mut();
    }
    get_result(conn)
}

/// `swoole_pgsql_connectdb`: `PQconnectStart`, then poll the handshake to its
/// end or its `connect_timeout`. `PQconnectdb`'s whole contract is the string the
/// type carries.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_pgsql_connectdb(conninfo: Text<'_>) -> *mut PGconn {
    let started = PQconnectStart(conninfo);
    if started.is_null() {
        return started;
    }
    // Live until the driver's `PQfinish`.
    let conn = Conn(started);
    if conn.socket() < 0 {
        return started;
    }
    conn.set_nonblocking(!blocking() && !swoole_rs_corebase::coroutine::current().is_null());
    wait_connect(conn, Conn::connect_poll);
    started
}

/// `swoole_pgsql_finish`: forget the connection's interrupted I/O, then `PQfinish`.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_pgsql_finish(conn: OwnedConn) {
    let finished = conn.conn();
    with_io_errors(|errors| errors.retain(|(c, _, _)| *c != finished));
    PQfinish(conn);
}

/// `swoole_pgsql_exec`: `PQsendQuery`, then its result. `PQexec`'s whole contract is
/// the live connection and the query's string, which the types carry.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_pgsql_exec(conn: Conn, query: Text<'_>) -> *mut PGresult {
    finish(conn, PQsendQuery(conn, query))
}

/// A parameter array the driver handed over: null, or readable for the parameter
/// count passed beside it. As with [`Conn`], only the exports' parameters make one.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Params<T>(*const T);

/// One parameter's value: null for SQL NULL, else readable up to its NUL in text
/// format, or for its length in the lengths array beside it in binary format.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Value(*const c_char);

// libpq's own declarations of the parameterised sends, each argument typed as what
// libpq requires of it. As `PQsendQuery`, each copies the statement and parameters
// into the connection's output buffer and parses no input, so it runs no notice
// callback; it refuses a null name or statement with an error on the connection.
unsafe extern "C" {
    safe fn PQsendPrepare(
        conn: Conn,
        stmt_name: Option<Text<'_>>,
        query: Option<Text<'_>>,
        n_params: c_int,
        param_types: Params<Oid>,
    ) -> c_int;
    safe fn PQsendQueryPrepared(
        conn: Conn,
        stmt_name: Option<Text<'_>>,
        n_params: c_int,
        param_values: Params<Value>,
        param_lengths: Params<c_int>,
        param_formats: Params<c_int>,
        result_format: c_int,
    ) -> c_int;
    safe fn PQsendQueryParams(
        conn: Conn,
        command: Option<Text<'_>>,
        n_params: c_int,
        param_types: Params<Oid>,
        param_values: Params<Value>,
        param_lengths: Params<c_int>,
        param_formats: Params<c_int>,
        result_format: c_int,
    ) -> c_int;
}

/// Each `swoole_pgsql_<call>` sends `<send>` on the connection, then waits for its
/// result, as libpq's blocking call would. `<send>`'s whole contract is what the
/// arguments' types carry.
macro_rules! sent {
    ($($name:ident => $send:ident, like $blocking:literal ($($arg:ident: $ty:ty),*);)*) => {$(
        #[doc = concat!("`", stringify!($name), "`: `", stringify!($send), "`, then its result, as `", $blocking, "`.")]
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(conn: Conn, $($arg: $ty),*) -> *mut PGresult {
            finish(conn, $send(conn, $($arg),*))
        }
    )*};
}

sent! {
    swoole_pgsql_prepare => PQsendPrepare, like "PQprepare" (
        stmt_name: Option<Text<'_>>, query: Option<Text<'_>>, n_params: c_int,
        param_types: Params<Oid>);
    swoole_pgsql_exec_prepared => PQsendQueryPrepared, like "PQexecPrepared" (
        stmt_name: Option<Text<'_>>, n_params: c_int, param_values: Params<Value>,
        param_lengths: Params<c_int>, param_formats: Params<c_int>, result_format: c_int);
    swoole_pgsql_exec_params => PQsendQueryParams, like "PQexecParams" (
        command: Option<Text<'_>>, n_params: c_int, param_types: Params<Oid>,
        param_values: Params<Value>, param_lengths: Params<c_int>,
        param_formats: Params<c_int>, result_format: c_int);
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
    fn events_map_to_poll() {
        assert_eq!(to_poll(Events::READ), libc::POLLIN);
        assert_eq!(to_poll(Events::WRITE), libc::POLLOUT);
        assert_eq!(
            to_poll(Events::READ | Events::WRITE),
            libc::POLLIN | libc::POLLOUT
        );
    }
}
