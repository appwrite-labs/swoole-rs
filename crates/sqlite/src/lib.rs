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
pub mod minit;

use std::{
    cell::Cell,
    ffi::{c_char, c_int, c_void},
};
use swoole_rs_corebase::sys::Text;

/// `sqlite3`, only ever passed by pointer here.
#[repr(C)]
pub struct Sqlite3 {
    _opaque: [u8; 0],
}

/// `sqlite3_stmt`, only ever passed by pointer here.
#[repr(C)]
pub struct Sqlite3Stmt {
    _opaque: [u8; 0],
}

/// `sqlite3_exec`'s row callback.
pub type ExecCallback =
    Option<unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_char, *mut *mut c_char) -> c_int>;

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
    fn sqlite3_prepare_v2(
        db: *mut Sqlite3,
        sql: *const c_char,
        bytes: c_int,
        stmt: *mut *mut Sqlite3Stmt,
        tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_exec(
        db: *mut Sqlite3,
        sql: *const c_char,
        callback: ExecCallback,
        argument: *mut c_void,
        errmsg: *mut *mut c_char,
    ) -> c_int;
    fn sqlite3_close(db: *mut Sqlite3) -> c_int;
    fn sqlite3_close_v2(db: *mut Sqlite3) -> c_int;
    fn sqlite3_step(stmt: *mut Sqlite3Stmt) -> c_int;
}

/// `SQLITE_OPEN_FULLMUTEX`.
const SQLITE_OPEN_FULLMUTEX: c_int = 0x0001_0000;

thread_local! {
    /// `swoole_sqlite_blocking`, per thread as upstream's.
    static BLOCKING: Cell<bool> = const { Cell::new(true) };
}

/// `swoole_sqlite_set_blocking`: the runtime hook flag. Turning blocking off
/// needs a library built thread-safe; otherwise it warns and stays blocking.
#[unsafe(no_mangle)]
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

/// `swoole_sqlite3_prepare_v2`.
/// # Safety
/// As `sqlite3_prepare_v2`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_sqlite3_prepare_v2(
    db: *mut Sqlite3,
    sql: *const c_char,
    bytes: c_int,
    stmt: *mut *mut Sqlite3Stmt,
    tail: *mut *const c_char,
) -> c_int {
    // SAFETY: per the caller.
    offload(|| unsafe { sqlite3_prepare_v2(db, sql, bytes, stmt, tail) })
}

/// `swoole_sqlite3_exec`.
/// # Safety
/// As `sqlite3_exec`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_sqlite3_exec(
    db: *mut Sqlite3,
    sql: *const c_char,
    callback: ExecCallback,
    argument: *mut c_void,
    errmsg: *mut *mut c_char,
) -> c_int {
    // SAFETY: per the caller.
    offload(|| unsafe { sqlite3_exec(db, sql, callback, argument, errmsg) })
}

/// `swoole_sqlite3_close`.
/// # Safety
/// As `sqlite3_close`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_sqlite3_close(db: *mut Sqlite3) -> c_int {
    // SAFETY: per the caller.
    offload(|| unsafe { sqlite3_close(db) })
}

/// `swoole_sqlite3_close_v2`.
/// # Safety
/// As `sqlite3_close_v2`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_sqlite3_close_v2(db: *mut Sqlite3) -> c_int {
    // SAFETY: per the caller.
    offload(|| unsafe { sqlite3_close_v2(db) })
}

/// `swoole_sqlite3_step`.
/// # Safety
/// As `sqlite3_step`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_sqlite3_step(stmt: *mut Sqlite3Stmt) -> c_int {
    // SAFETY: per the caller.
    offload(|| unsafe { sqlite3_step(stmt) })
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
