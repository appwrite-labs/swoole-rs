//! Swoole's coroutine ODBC calls (upstream's `swoole_odbc.cc`, built with
//! `--with-swoole-odbc`): the `SQL*` replacements its bundled `pdo_odbc`
//! driver calls. While the runtime hook is on and a coroutine runs, each
//! blocking call runs on Swoole's thread pool and the coroutine waits for it
//! (native `async`, through the hooks' host); the cheap ones call through.
//! With `php`, MINIT ([`minit`]) registers the driver with PDO.
//!
//! The exports keep upstream's names: `native/hooks/php_swoole_odbc.h` declares them and
//! the driver's macros call them. Upstream's pass-throughs (`SQLGetInfo`,
//! `SQLGetDiagRec`, `SQLCloseCursor`, `SQLRowCount`, `SQLFreeHandle`), which only
//! traced, are not redirected: the driver calls ODBC for those.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

#[cfg(feature = "php")]
pub mod minit;

use std::sync::atomic::{AtomicBool, Ordering};
use swoole_rs_odbc_sys::{
    RETCODE, SQLCHAR, SQLConnect, SQLDescribeCol, SQLDriverConnect, SQLEndTran, SQLExecDirect,
    SQLExecute, SQLGetData, SQLHANDLE, SQLHDBC, SQLHSTMT, SQLHWND, SQLINTEGER, SQLLEN, SQLPOINTER,
    SQLPrepare, SQLPutData, SQLRETURN, SQLSMALLINT, SQLULEN, SQLUSMALLINT,
};

/// A connection handle the driver handed over: live, as ODBC's own calls
/// require, for as long as the value is used. [`swoole_odbc_SQLDisconnect`]
/// takes one in place of the `SQLHDBC` it has the layout of. Nothing here makes
/// one, and it is neither `Send` nor `Sync`: it reaches the pool only through
/// [`offload`].
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Connection(SQLHDBC);

// ODBC's own declaration of `SQLDisconnect`, typed as the handle it takes. It
// reads no bound buffer and runs no callback.
unsafe extern "C" {
    #[link_name = "SQLDisconnect"]
    safe fn disconnect(connection: Connection) -> SQLRETURN;
}

/// `SQL_ERROR`.
const SQL_ERROR: SQLRETURN = -1;

static BLOCKING: AtomicBool = AtomicBool::new(true);

/// `swoole_odbc_set_blocking`: the runtime hook flag.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_odbc_set_blocking(blocking: bool) {
    BLOCKING.store(blocking, Ordering::Relaxed);
}

/// `php_swoole_async(swoole_odbc_blocking, call)`: the call's result, or
/// `SQL_ERROR` when the pool did not run it.
fn offload(call: impl FnMut() -> SQLRETURN) -> SQLRETURN {
    let offload = swoole_rs_hooks::stdio::offload().filter(|_| !BLOCKING.load(Ordering::Relaxed));
    // SAFETY: native `async` inside a coroutine, or none; the call waits for the job.
    unsafe { swoole_rs_hooks::stdio::offloaded(offload, SQL_ERROR, call) }
}

/// `swoole_odbc_SQLConnect`.
/// # Safety
/// As `SQLConnect`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_odbc_SQLConnect(
    connection: SQLHDBC,
    server_name: *mut SQLCHAR,
    name_length1: SQLSMALLINT,
    user_name: *mut SQLCHAR,
    name_length2: SQLSMALLINT,
    authentication: *mut SQLCHAR,
    name_length3: SQLSMALLINT,
) -> RETCODE {
    offload(|| {
        // SAFETY: per the caller.
        unsafe {
            SQLConnect(
                connection,
                server_name,
                name_length1,
                user_name,
                name_length2,
                authentication,
                name_length3,
            )
        }
    })
}

/// `swoole_odbc_SQLDriverConnect`.
/// # Safety
/// As `SQLDriverConnect`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_odbc_SQLDriverConnect(
    hdbc: SQLHDBC,
    hwnd: SQLHWND,
    conn_str_in: *mut SQLCHAR,
    conn_str_in_length: SQLSMALLINT,
    conn_str_out: *mut SQLCHAR,
    conn_str_out_max: SQLSMALLINT,
    conn_str_out_length: *mut SQLSMALLINT,
    driver_completion: SQLUSMALLINT,
) -> SQLRETURN {
    offload(|| {
        // SAFETY: per the caller.
        unsafe {
            SQLDriverConnect(
                hdbc,
                hwnd,
                conn_str_in,
                conn_str_in_length,
                conn_str_out,
                conn_str_out_max,
                conn_str_out_length,
                driver_completion,
            )
        }
    })
}

/// `swoole_odbc_SQLExecDirect`.
/// # Safety
/// As `SQLExecDirect`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_odbc_SQLExecDirect(
    statement: SQLHSTMT,
    text: *mut SQLCHAR,
    length: SQLINTEGER,
) -> SQLRETURN {
    // SAFETY: per the caller.
    offload(|| unsafe { SQLExecDirect(statement, text, length) })
}

/// `swoole_odbc_SQLPrepare`.
/// # Safety
/// As `SQLPrepare`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_odbc_SQLPrepare(
    statement: SQLHSTMT,
    text: *mut SQLCHAR,
    length: SQLINTEGER,
) -> SQLRETURN {
    // SAFETY: per the caller.
    offload(|| unsafe { SQLPrepare(statement, text, length) })
}

/// `swoole_odbc_SQLExecute`.
/// # Safety
/// As `SQLExecute`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_odbc_SQLExecute(statement: SQLHSTMT) -> SQLRETURN {
    // SAFETY: per the caller.
    offload(|| unsafe { SQLExecute(statement) })
}

/// `swoole_odbc_SQLPutData`.
/// # Safety
/// As `SQLPutData`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_odbc_SQLPutData(
    statement: SQLHSTMT,
    data: SQLPOINTER,
    length_or_indicator: SQLLEN,
) -> SQLRETURN {
    // SAFETY: per the caller.
    offload(|| unsafe { SQLPutData(statement, data, length_or_indicator) })
}

/// `swoole_odbc_SQLGetData`.
/// # Safety
/// As `SQLGetData`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_odbc_SQLGetData(
    statement: SQLHSTMT,
    column: SQLUSMALLINT,
    target_type: SQLSMALLINT,
    target: SQLPOINTER,
    buffer_length: SQLLEN,
    length_or_indicator: *mut SQLLEN,
) -> SQLRETURN {
    offload(|| {
        // SAFETY: per the caller.
        unsafe {
            SQLGetData(
                statement,
                column,
                target_type,
                target,
                buffer_length,
                length_or_indicator,
            )
        }
    })
}

/// `swoole_odbc_SQLDescribeCol`.
/// # Safety
/// As `SQLDescribeCol`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_odbc_SQLDescribeCol(
    statement: SQLHSTMT,
    column: SQLUSMALLINT,
    name: *mut SQLCHAR,
    buffer_length: SQLSMALLINT,
    name_length: *mut SQLSMALLINT,
    data_type: *mut SQLSMALLINT,
    column_size: *mut SQLULEN,
    decimal_digits: *mut SQLSMALLINT,
    nullable: *mut SQLSMALLINT,
) -> SQLRETURN {
    offload(|| {
        // SAFETY: per the caller.
        unsafe {
            SQLDescribeCol(
                statement,
                column,
                name,
                buffer_length,
                name_length,
                data_type,
                column_size,
                decimal_digits,
                nullable,
            )
        }
    })
}

/// `swoole_odbc_SQLEndTran`.
/// # Safety
/// As `SQLEndTran`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_odbc_SQLEndTran(
    handle_type: SQLSMALLINT,
    handle: SQLHANDLE,
    completion: SQLSMALLINT,
) -> SQLRETURN {
    // SAFETY: per the caller.
    offload(|| unsafe { SQLEndTran(handle_type, handle, completion) })
}

/// `swoole_odbc_SQLDisconnect`: `SQLDisconnect`'s whole contract is the live
/// connection the type carries.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_odbc_SQLDisconnect(connection: Connection) -> SQLRETURN {
    offload(|| disconnect(connection))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offload_calls_through_while_blocking() {
        swoole_odbc_set_blocking(true);
        assert_eq!(offload(|| 7), 7);
    }

    #[test]
    fn offload_calls_through_without_a_coroutine() {
        swoole_odbc_set_blocking(false);
        assert_eq!(offload(|| 8), 8);
        swoole_odbc_set_blocking(true);
    }
}
