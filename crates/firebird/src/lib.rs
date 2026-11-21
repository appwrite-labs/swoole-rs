//! Swoole's coroutine Firebird calls (upstream's `swoole_firebird.cc`, built
//! with `--with-swoole-firebird`): the `isc_*` replacements its bundled
//! `pdo_firebird` driver calls. While the runtime hook is on and a coroutine
//! runs, each call runs on Swoole's thread pool and the coroutine waits for it
//! (native `async`, through the hooks' host). With `php`, MINIT ([`minit`])
//! registers the driver with PDO.
//!
//! The exports keep upstream's names: `native/hooks/php_swoole_firebird.h` declares them
//! and the driver's macros call them.
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
    ffi::{c_char, c_int, c_short, c_ushort, c_void},
};
use swoole_rs_firebird_sys::{
    ISC_QUAD, ISC_SCHAR, ISC_STATUS, ISC_VERSION_CALLBACK, XSQLDA, fb_ping, isc_attach_database,
    isc_blob_handle, isc_blob_info, isc_close_blob, isc_commit_retaining, isc_commit_transaction,
    isc_create_blob, isc_db_handle, isc_detach_database, isc_dsql_allocate_statement,
    isc_dsql_execute, isc_dsql_execute2, isc_dsql_fetch, isc_dsql_free_statement, isc_dsql_prepare,
    isc_dsql_set_cursor_name, isc_dsql_sql_info, isc_get_segment, isc_open_blob, isc_put_segment,
    isc_rollback_transaction, isc_start_transaction, isc_stmt_handle, isc_tr_handle, isc_version,
};

thread_local! {
    /// `swoole_firebird_blocking`, per thread as upstream's.
    static BLOCKING: Cell<bool> = const { Cell::new(true) };
}

/// `swoole_firebird_set_blocking`: the runtime hook flag.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_firebird_set_blocking(blocking: bool) {
    BLOCKING.set(blocking);
}

/// `php_swoole_async(swoole_firebird_blocking, call)`: the call's result, or
/// `fallback` when the pool did not run it.
fn offload<R>(fallback: R, call: impl FnMut() -> R) -> R {
    let offload = swoole_rs_hooks::stdio::offload().filter(|_| !BLOCKING.get());
    // SAFETY: native `async` inside a coroutine, or none; the call waits for the job.
    unsafe { swoole_rs_hooks::stdio::offloaded(offload, fallback, call) }
}

/// What a wrapper returns when the pool could not run the call.
const NOT_RUN: ISC_STATUS = -1;

/// Each `swoole_<call>` runs `<call>` through [`offload`].
macro_rules! offloaded {
    ($($name:ident => $call:ident($($arg:ident: $ty:ty),*);)*) => {$(
        #[doc = concat!("`", stringify!($name), "`: `", stringify!($call), "` off the coroutine.")]
        /// # Safety
        /// As the Firebird call.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name($($arg: $ty),*) -> ISC_STATUS {
            // SAFETY: per the caller.
            offload(NOT_RUN, || unsafe { $call($($arg),*) })
        }
    )*};
}

offloaded! {
    swoole_isc_attach_database => isc_attach_database(
        status: *mut ISC_STATUS, name_length: c_short, name: *const ISC_SCHAR,
        db: *mut isc_db_handle, dpb_length: c_short, dpb: *const ISC_SCHAR);
    swoole_isc_detach_database => isc_detach_database(status: *mut ISC_STATUS, db: *mut isc_db_handle);
    swoole_isc_dsql_execute => isc_dsql_execute(
        status: *mut ISC_STATUS, tr: *mut isc_tr_handle, stmt: *mut isc_stmt_handle,
        dialect: c_ushort, sqlda: *const XSQLDA);
    swoole_isc_dsql_execute2 => isc_dsql_execute2(
        status: *mut ISC_STATUS, tr: *mut isc_tr_handle, stmt: *mut isc_stmt_handle,
        dialect: c_ushort, in_sqlda: *const XSQLDA, out_sqlda: *const XSQLDA);
    swoole_isc_dsql_sql_info => isc_dsql_sql_info(
        status: *mut ISC_STATUS, stmt: *mut isc_stmt_handle, items_length: c_short,
        items: *const ISC_SCHAR, buffer_length: c_short, buffer: *mut ISC_SCHAR);
    swoole_isc_dsql_free_statement => isc_dsql_free_statement(
        status: *mut ISC_STATUS, stmt: *mut isc_stmt_handle, option: c_ushort);
    swoole_isc_commit_retaining => isc_commit_retaining(status: *mut ISC_STATUS, tr: *mut isc_tr_handle);
    swoole_isc_commit_transaction => isc_commit_transaction(status: *mut ISC_STATUS, tr: *mut isc_tr_handle);
    swoole_isc_rollback_transaction => isc_rollback_transaction(status: *mut ISC_STATUS, tr: *mut isc_tr_handle);
    swoole_isc_dsql_allocate_statement => isc_dsql_allocate_statement(
        status: *mut ISC_STATUS, db: *mut isc_db_handle, stmt: *mut isc_stmt_handle);
    swoole_isc_dsql_prepare => isc_dsql_prepare(
        status: *mut ISC_STATUS, tr: *mut isc_tr_handle, stmt: *mut isc_stmt_handle, length: c_ushort,
        statement: *const ISC_SCHAR, dialect: c_ushort, sqlda: *mut XSQLDA);
    swoole_isc_dsql_fetch => isc_dsql_fetch(
        status: *mut ISC_STATUS, stmt: *mut isc_stmt_handle, dialect: c_ushort, sqlda: *const XSQLDA);
    swoole_isc_open_blob => isc_open_blob(
        status: *mut ISC_STATUS, db: *mut isc_db_handle, tr: *mut isc_tr_handle,
        blob: *mut isc_blob_handle, id: *mut ISC_QUAD);
    swoole_isc_blob_info => isc_blob_info(
        status: *mut ISC_STATUS, blob: *mut isc_blob_handle, items_length: c_short,
        items: *const ISC_SCHAR, buffer_length: c_short, buffer: *mut ISC_SCHAR);
    swoole_isc_get_segment => isc_get_segment(
        status: *mut ISC_STATUS, blob: *mut isc_blob_handle, length: *mut c_ushort,
        buffer_length: c_ushort, buffer: *mut ISC_SCHAR);
    swoole_isc_put_segment => isc_put_segment(
        status: *mut ISC_STATUS, blob: *mut isc_blob_handle, length: c_ushort, buffer: *const ISC_SCHAR);
    swoole_isc_create_blob => isc_create_blob(
        status: *mut ISC_STATUS, db: *mut isc_db_handle, tr: *mut isc_tr_handle,
        blob: *mut isc_blob_handle, id: *mut ISC_QUAD);
    swoole_isc_close_blob => isc_close_blob(status: *mut ISC_STATUS, blob: *mut isc_blob_handle);
    swoole_isc_dsql_set_cursor_name => isc_dsql_set_cursor_name(
        status: *mut ISC_STATUS, stmt: *mut isc_stmt_handle, name: *const ISC_SCHAR, kind: c_ushort);
    swoole_fb_ping => fb_ping(status: *mut ISC_STATUS, db: *mut isc_db_handle);
}

/// `swoole_isc_start_transaction`: one database's transaction, its TPB passed
/// on through the variadic `isc_start_transaction`.
/// # Safety
/// As `isc_start_transaction` with one database.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_isc_start_transaction(
    status: *mut ISC_STATUS,
    tr: *mut isc_tr_handle,
    count: c_short,
    db: *mut isc_db_handle,
    tpb_length: usize,
    tpb: *mut c_char,
) -> ISC_STATUS {
    // SAFETY: per the caller.
    offload(NOT_RUN, || unsafe {
        isc_start_transaction(status, tr, count, db, tpb_length, tpb)
    })
}

/// `swoole_isc_version`.
/// # Safety
/// As `isc_version`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_isc_version(
    db: *mut isc_db_handle,
    callback: ISC_VERSION_CALLBACK,
    arg: *mut c_void,
) -> c_int {
    // SAFETY: per the caller.
    offload(-1, || unsafe { isc_version(db, callback, arg) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offload_calls_through_while_blocking() {
        swoole_firebird_set_blocking(true);
        assert_eq!(offload(NOT_RUN, || 0), 0);
    }

    #[test]
    fn offload_calls_through_without_host() {
        swoole_firebird_set_blocking(false);
        assert_eq!(offload(NOT_RUN, || 5), 5);
    }
}
