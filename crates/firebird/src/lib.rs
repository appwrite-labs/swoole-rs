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
pub mod attribute;
#[cfg(feature = "php")]
pub mod connection;
#[cfg(feature = "php")]
pub mod minit;
pub mod preprocess;
#[cfg(feature = "php")]
pub mod statement;
#[cfg(feature = "php")]
pub mod transaction;

use std::{
    cell::Cell,
    ffi::{c_int, c_short, c_ushort},
};
use swoole_rs_firebird_sys::{
    Bytes, Handle, ISC_SCHAR, ISC_STATUS, Out, Quad, Sqlda, Status, Teb, TebCount, VersionArg,
    VersionCallback, fb_ping, isc_attach_database, isc_blob_info, isc_close_blob,
    isc_commit_retaining, isc_commit_transaction, isc_create_blob, isc_detach_database,
    isc_dsql_allocate_statement, isc_dsql_execute, isc_dsql_execute2, isc_dsql_fetch,
    isc_dsql_free_statement, isc_dsql_prepare, isc_dsql_set_cursor_name, isc_dsql_sql_info,
    isc_get_segment, isc_open_blob, isc_put_segment, isc_rollback_transaction, isc_start_multiple,
    isc_version,
};

thread_local! {
    /// `swoole_firebird_blocking`, per thread as upstream's.
    static BLOCKING: Cell<bool> = const { Cell::new(true) };
}

/// `swoole_firebird_set_blocking`: the runtime hook flag.
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

/// Each `swoole_<call>` runs `<call>`, whose whole contract the arguments' types
/// carry, through [`offload`], returning `NOT_RUN` (or the fallback given) when the
/// pool did not run it.
macro_rules! offloaded {
    ($($name:ident => $call:ident($($arg:ident: $ty:ty),*) $(-> $ret:ty = $fallback:expr)?;)*) => {$(
        #[doc = concat!("`", stringify!($name), "`: `", stringify!($call), "` off the coroutine.")]
        #[unsafe(no_mangle)]
        pub extern "C" fn $name($($arg: $ty),*) -> offloaded!(@ret $($ret)?) {
            offload(offloaded!(@fallback $($fallback)?), || $call($($arg),*))
        }
    )*};
    (@ret) => { ISC_STATUS };
    (@ret $ret:ty) => { $ret };
    (@fallback) => { NOT_RUN };
    (@fallback $fallback:expr) => { $fallback };
}

offloaded! {
    swoole_isc_attach_database => isc_attach_database(
        status: Status, name_length: c_short, name: Bytes,
        db: Handle, dpb_length: c_short, dpb: Bytes);
    swoole_isc_detach_database => isc_detach_database(status: Status, db: Handle);
    swoole_isc_dsql_execute => isc_dsql_execute(
        status: Status, tr: Handle, stmt: Handle,
        dialect: c_ushort, sqlda: Sqlda);
    swoole_isc_dsql_execute2 => isc_dsql_execute2(
        status: Status, tr: Handle, stmt: Handle,
        dialect: c_ushort, in_sqlda: Sqlda, out_sqlda: Sqlda);
    swoole_isc_dsql_sql_info => isc_dsql_sql_info(
        status: Status, stmt: Handle, items_length: c_short,
        items: Bytes, buffer_length: c_short, buffer: Out<ISC_SCHAR>);
    swoole_isc_dsql_free_statement => isc_dsql_free_statement(
        status: Status, stmt: Handle, option: c_ushort);
    swoole_isc_commit_retaining => isc_commit_retaining(status: Status, tr: Handle);
    swoole_isc_commit_transaction => isc_commit_transaction(status: Status, tr: Handle);
    swoole_isc_rollback_transaction => isc_rollback_transaction(status: Status, tr: Handle);
    swoole_isc_dsql_allocate_statement => isc_dsql_allocate_statement(
        status: Status, db: Handle, stmt: Handle);
    swoole_isc_dsql_prepare => isc_dsql_prepare(
        status: Status, tr: Handle, stmt: Handle, length: c_ushort,
        statement: Bytes, dialect: c_ushort, sqlda: Sqlda);
    swoole_isc_dsql_fetch => isc_dsql_fetch(
        status: Status, stmt: Handle, dialect: c_ushort, sqlda: Sqlda);
    swoole_isc_open_blob => isc_open_blob(
        status: Status, db: Handle, tr: Handle,
        blob: Handle, id: Quad);
    swoole_isc_blob_info => isc_blob_info(
        status: Status, blob: Handle, items_length: c_short,
        items: Bytes, buffer_length: c_short, buffer: Out<ISC_SCHAR>);
    swoole_isc_get_segment => isc_get_segment(
        status: Status, blob: Handle, length: Out<c_ushort>,
        buffer_length: c_ushort, buffer: Out<ISC_SCHAR>);
    swoole_isc_put_segment => isc_put_segment(
        status: Status, blob: Handle, length: c_ushort, buffer: Bytes);
    swoole_isc_create_blob => isc_create_blob(
        status: Status, db: Handle, tr: Handle,
        blob: Handle, id: Quad);
    swoole_isc_close_blob => isc_close_blob(status: Status, blob: Handle);
    swoole_isc_dsql_set_cursor_name => isc_dsql_set_cursor_name(
        status: Status, stmt: Handle, name: Bytes, kind: c_ushort);
    swoole_fb_ping => fb_ping(status: Status, db: Handle);
    swoole_isc_version => isc_version(
        db: Handle, callback: VersionCallback, arg: VersionArg) -> c_int = -1;
}

/// `swoole_isc_start_transaction`: one database's transaction, as the variadic
/// `isc_start_transaction` builds it for `isc_start_multiple`. The driver passes a
/// count of one, and one database is all this takes, so the count goes unread.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_isc_start_transaction(
    status: Status,
    tr: Handle,
    _count: c_short,
    db: Handle,
    tpb_length: usize,
    tpb: Bytes,
) -> ISC_STATUS {
    let teb = Teb {
        database: db,
        // Fewer than the TPB's bytes when it is longer than an `int`, as the
        // variadic call's `int` read made of it.
        tpb_length: c_int::try_from(tpb_length).unwrap_or(c_int::MAX),
        tpb,
    };
    offload(NOT_RUN, || {
        isc_start_multiple(status, tr, TebCount::One, &teb)
    })
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
