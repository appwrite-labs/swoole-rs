//! Swoole's coroutine SQLite calls (upstream's `swoole_sqlite.cc`, built with
//! `--enable-swoole-sqlite`): the `sqlite3_*` replacements its bundled
//! `pdo_sqlite` driver calls. While the runtime hook is on and a coroutine
//! runs, each call runs on Swoole's thread pool and the coroutine waits for it
//! (native `async`, through the hooks' host). With `php`, MINIT ([`minit`])
//! registers the driver with PDO.
//!
//! The exports keep upstream's names: `native/hooks/php_swoole_sqlite.h` declares them and
//! the driver's macros call them. The extension links the system `libsqlite3`
//! itself (configure's `pkg-config sqlite3`); nothing here links it.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

#[cfg(feature = "php")]
mod driver;
#[cfg(feature = "php")]
pub mod minit;
#[cfg(any(feature = "php", test))]
mod scanner;

use std::{
    cell::Cell,
    ffi::{c_char, c_int, c_void},
};
use swoole_rs_corebase::sys::Text;

/// `sqlite3`, only ever passed by pointer here.
// `repr(C)`: SQLite's opaque connection.
#[repr(C)]
pub struct Sqlite3 {
    _opaque: [u8; 0],
}

/// `sqlite3_stmt`, only ever passed by pointer here.
// `repr(C)`: SQLite's opaque statement.
#[repr(C)]
pub struct Sqlite3Stmt {
    _opaque: [u8; 0],
}

/// The connection the driver gives up to `sqlite3_close`: null, or live and used by
/// nothing else during the call. Only the close exports' parameters make one, where the
/// C caller's contract stands for the constructor; it is not `Copy`, so it is closed
/// once.
#[repr(transparent)]
pub struct OwnedDb(*mut Sqlite3);

// The driver's other arguments, each typed as what SQLite requires of it. As with
// [`OwnedDb`], only the exports' parameters make one, and none is `Send`, so they
// reach the pool only through [`offload`].

/// A connection the driver handed over: open and live for as long as the value is used.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Db(*mut Sqlite3);

/// A statement the driver handed over: null, which `sqlite3_step` refuses with
/// `SQLITE_MISUSE`, or prepared and live for as long as the value is used.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Stmt(*mut Sqlite3Stmt);

/// SQL `sqlite3_prepare_v2` compiles: readable for the byte count passed beside it
/// when that is not negative, else up to its NUL.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Sql(*const c_char);

/// What `sqlite3_exec` hands the [`ExecCallback`] passed beside it, which accepts it.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Argument(*mut c_void);

/// The close a pool thread may run: the owner moves into it, and is closed at most once.
fn close(close: impl FnOnce(OwnedDb) -> c_int, db: OwnedDb) -> c_int {
    let mut call = Some((close, db));
    offload(|| call.take().map_or(0, |(close, db)| close(db)))
}

/// The row callback the driver passes `sqlite3_exec`: null, or one that accepts the
/// [`Argument`] passed beside it. Only SQLite calls it.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct ExecCallback(*const c_void);

unsafe extern "C" {
    /// The library's compile-time option: no arguments, no state.
    safe fn sqlite3_threadsafe() -> c_int;
    /// Reads the file name and the optional VFS name up to their NULs, and writes
    /// the new connection (or null) to `db`.
    safe fn sqlite3_open_v2(
        filename: Text<'_>,
        db: &mut *mut Sqlite3,
        flags: c_int,
        vfs: Option<Text<'_>>,
    ) -> c_int;
    /// Compiles the SQL, writing the statement (or null) to `stmt` and where it
    /// stopped to `tail`.
    safe fn sqlite3_prepare_v2(
        db: Db,
        sql: Sql,
        bytes: c_int,
        stmt: &mut *mut Sqlite3Stmt,
        tail: Option<&mut *const c_char>,
    ) -> c_int;
    /// Runs the SQL (none for a null one) up to its NUL, handing each row to `callback` with `argument`,
    /// and writes a message from `sqlite3_malloc` (or null) to `errmsg`.
    safe fn sqlite3_exec(
        db: Db,
        sql: Option<Text<'_>>,
        callback: ExecCallback,
        argument: Argument,
        errmsg: Option<&mut *mut c_char>,
    ) -> c_int;
    /// Closes the connection (nothing for a null one), or, while statements remain,
    /// refuses with `SQLITE_BUSY` and leaves it open: it takes the owner either way.
    safe fn sqlite3_close(db: OwnedDb) -> c_int;
    /// As `sqlite3_close`, deferring the close to the last statement's finalize.
    safe fn sqlite3_close_v2(db: OwnedDb) -> c_int;
    safe fn sqlite3_step(stmt: Stmt) -> c_int;
}

/// `SQLITE_OPEN_FULLMUTEX`.
const SQLITE_OPEN_FULLMUTEX: c_int = 0x0001_0000;

thread_local! {
    /// `swoole_sqlite_blocking`, per thread as upstream's.
    static BLOCKING: Cell<bool> = const { Cell::new(true) };
}

/// `swoole_sqlite_set_blocking`: the runtime hook flag. Turning blocking off
/// needs a library built thread-safe; otherwise it warns and stays blocking.
pub extern "C" fn swoole_sqlite_set_blocking(blocking: bool) {
    if !blocking && sqlite3_threadsafe() == 0 {
        swoole_rs_corebase::log::warning(
            c"swoole_sqlite_set_blocking",
            format_args!("hook sqlite coroutine failed because thread safe mode is single-thread."),
        );
        return;
    }
    BLOCKING.set(blocking);
}

/// The pool while the hook is on inside a coroutine, else none.
fn pool() -> Option<swoole_rs_hooks::stdio::Offload> {
    swoole_rs_hooks::stdio::offload().filter(|_| !BLOCKING.get())
}

/// `php_swoole_async(swoole_sqlite_blocking, call)`: the call's result, or 0
/// (upstream's untouched `result`) when the pool did not run it.
fn offload(call: impl FnMut() -> c_int) -> c_int {
    // SAFETY: native `async` inside a coroutine, or none; the call waits for the job.
    unsafe { swoole_rs_hooks::stdio::offloaded(pool(), 0, call) }
}

/// `swoole_sqlite3_open_v2`: inside a hooked coroutine the connection opens
/// serialized (`SQLITE_OPEN_FULLMUTEX`), since pool threads share it.
/// `sqlite3_open_v2`'s whole contract is the strings and the slot the types carry.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_sqlite3_open_v2(
    filename: Text<'_>,
    db: &mut *mut Sqlite3,
    flags: c_int,
    vfs: Option<Text<'_>>,
) -> c_int {
    let flags = if pool().is_some() {
        flags | SQLITE_OPEN_FULLMUTEX
    } else {
        flags
    };
    offload(|| sqlite3_open_v2(filename, db, flags, vfs))
}

/// `swoole_sqlite3_prepare_v2`: `sqlite3_prepare_v2`, whose whole contract the
/// arguments' types carry.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_sqlite3_prepare_v2(
    db: Db,
    sql: Sql,
    bytes: c_int,
    stmt: &mut *mut Sqlite3Stmt,
    mut tail: Option<&mut *const c_char>,
) -> c_int {
    offload(|| sqlite3_prepare_v2(db, sql, bytes, stmt, tail.as_deref_mut()))
}

/// `swoole_sqlite3_exec`: `sqlite3_exec`, whose whole contract the arguments' types carry.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_sqlite3_exec(
    db: Db,
    sql: Option<Text<'_>>,
    callback: ExecCallback,
    argument: Argument,
    mut errmsg: Option<&mut *mut c_char>,
) -> c_int {
    offload(|| sqlite3_exec(db, sql, callback, argument, errmsg.as_deref_mut()))
}

/// `swoole_sqlite3_close`. `sqlite3_close`'s whole contract is the owned connection
/// the type carries.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_sqlite3_close(db: OwnedDb) -> c_int {
    close(|db| sqlite3_close(db), db)
}

/// `swoole_sqlite3_close_v2`, as [`swoole_sqlite3_close`].
#[unsafe(no_mangle)]
pub extern "C" fn swoole_sqlite3_close_v2(db: OwnedDb) -> c_int {
    close(|db| sqlite3_close_v2(db), db)
}

/// `swoole_sqlite3_step`: `sqlite3_step`, whose whole contract the statement's type carries.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_sqlite3_step(stmt: Stmt) -> c_int {
    offload(|| sqlite3_step(stmt))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offload_calls_through_while_blocking() {
        BLOCKING.set(true);
        assert_eq!(offload(|| 7), 7);
    }

    #[test]
    fn offload_calls_through_without_host() {
        BLOCKING.set(false);
        assert!(pool().is_none());
        assert_eq!(offload(|| 5), 5);
    }
}
