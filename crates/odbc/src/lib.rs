//! Swoole's coroutine ODBC calls (upstream's `swoole_odbc.cc`, built with
//! `--with-swoole-odbc`): the `SQL*` replacements its bundled `pdo_odbc`
//! driver calls. While the runtime hook is on and a coroutine runs, each
//! blocking call runs on Swoole's thread pool and the coroutine waits for it
//! (C++ lends `swoole::coroutine::async`); the cheap ones call through.
//!
//! The exports keep upstream's names: `php_swoole_odbc.h` declares them and
//! the driver's macros call them.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

use std::{
    ffi::c_void,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use swoole_rs_odbc_sys::{
    RETCODE, SQLCHAR, SQLCloseCursor, SQLConnect, SQLDescribeCol, SQLDisconnect, SQLDriverConnect,
    SQLEndTran, SQLExecDirect, SQLExecute, SQLFreeHandle, SQLGetData, SQLGetDiagRec, SQLGetInfo,
    SQLHANDLE, SQLHDBC, SQLHSTMT, SQLHWND, SQLINTEGER, SQLLEN, SQLMoreResults, SQLPOINTER,
    SQLPrepare, SQLPutData, SQLRETURN, SQLRowCount, SQLSMALLINT, SQLULEN, SQLUSMALLINT,
};

/// `swr_v1_odbc_ops`: what C++ lends at MINIT.
#[repr(C)]
pub struct Ops {
    /// `swoole::coroutine::async`: run `job(context)` on the thread pool while
    /// the running coroutine waits.
    run_async: unsafe extern "C" fn(job: unsafe extern "C" fn(*mut c_void), context: *mut c_void),
    /// Whether a coroutine is running.
    in_coroutine: unsafe extern "C" fn() -> bool,
}

/// `SQL_ERROR`.
const SQL_ERROR: SQLRETURN = -1;

static OPS: OnceLock<&'static Ops> = OnceLock::new();
static BLOCKING: AtomicBool = AtomicBool::new(true);

/// MINIT: keep C++'s operations.
/// # Safety
/// `ops` is a static table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_odbc_init(ops: *const Ops) {
    // SAFETY: per the caller.
    if let Some(ops) = unsafe { ops.as_ref() } {
        OPS.get_or_init(|| ops);
    }
}

/// `swoole_odbc_set_blocking`: the runtime hook flag.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_odbc_set_blocking(blocking: bool) {
    BLOCKING.store(blocking, Ordering::Relaxed);
}

unsafe extern "C" fn trampoline(context: *mut c_void) {
    // SAFETY: `offload` passes its job, alive until `run_async` returns.
    let job = unsafe { &mut *context.cast::<&mut dyn FnMut()>() };
    job();
}

/// `php_swoole_async(swoole_odbc_blocking, call)`: the call's result, or
/// `SQL_ERROR` when the pool did not run it.
fn offload(call: impl FnOnce() -> SQLRETURN) -> SQLRETURN {
    let mut call = Some(call);
    let mut rc = SQL_ERROR;
    let mut job = || {
        if let Some(call) = call.take() {
            rc = call();
        }
    };
    let ops = OPS.get().filter(|_| !BLOCKING.load(Ordering::Relaxed));
    // SAFETY: C++ reads the running coroutine.
    match ops.filter(|ops| unsafe { (ops.in_coroutine)() }) {
        Some(ops) => {
            let mut job: &mut dyn FnMut() = &mut job;
            // SAFETY: the job outlives the call, which waits for it.
            unsafe { (ops.run_async)(trampoline, (&raw mut job).cast()) };
        }
        None => job(),
    }
    rc
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

/// `swoole_odbc_SQLGetInfo`: called through.
/// # Safety
/// As `SQLGetInfo`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_odbc_SQLGetInfo(
    connection: SQLHDBC,
    info_type: SQLUSMALLINT,
    value: SQLPOINTER,
    buffer_length: SQLSMALLINT,
    string_length: *mut SQLSMALLINT,
) -> SQLRETURN {
    // SAFETY: per the caller.
    unsafe { SQLGetInfo(connection, info_type, value, buffer_length, string_length) }
}

/// `swoole_odbc_SQLGetDiagRec`: called through.
/// # Safety
/// As `SQLGetDiagRec`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_odbc_SQLGetDiagRec(
    handle_type: SQLSMALLINT,
    handle: SQLHANDLE,
    record: SQLSMALLINT,
    sqlstate: *mut SQLCHAR,
    native_error: *mut SQLINTEGER,
    message: *mut SQLCHAR,
    buffer_length: SQLSMALLINT,
    text_length: *mut SQLSMALLINT,
) -> SQLRETURN {
    // SAFETY: per the caller.
    unsafe {
        SQLGetDiagRec(
            handle_type,
            handle,
            record,
            sqlstate,
            native_error,
            message,
            buffer_length,
            text_length,
        )
    }
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

/// `swoole_odbc_SQLCloseCursor`: called through.
/// # Safety
/// As `SQLCloseCursor`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_odbc_SQLCloseCursor(statement: SQLHSTMT) -> SQLRETURN {
    // SAFETY: per the caller.
    unsafe { SQLCloseCursor(statement) }
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

/// `swoole_odbc_SQLMoreResults`.
/// # Safety
/// As `SQLMoreResults`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_odbc_SQLMoreResults(statement: SQLHSTMT) -> SQLRETURN {
    // SAFETY: per the caller.
    offload(|| unsafe { SQLMoreResults(statement) })
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

/// `swoole_odbc_SQLRowCount`: called through.
/// # Safety
/// As `SQLRowCount`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_odbc_SQLRowCount(
    statement: SQLHSTMT,
    row_count: *mut SQLLEN,
) -> SQLRETURN {
    // SAFETY: per the caller.
    unsafe { SQLRowCount(statement, row_count) }
}

/// `swoole_odbc_SQLFreeHandle`: called through.
/// # Safety
/// As `SQLFreeHandle`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_odbc_SQLFreeHandle(
    handle_type: SQLSMALLINT,
    handle: SQLHANDLE,
) -> SQLRETURN {
    // SAFETY: per the caller.
    unsafe { SQLFreeHandle(handle_type, handle) }
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

/// `swoole_odbc_SQLDisconnect`.
/// # Safety
/// As `SQLDisconnect`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_odbc_SQLDisconnect(connection: SQLHDBC) -> SQLRETURN {
    // SAFETY: per the caller.
    offload(|| unsafe { SQLDisconnect(connection) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    static ASYNC_RUNS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn run_async(job: unsafe extern "C" fn(*mut c_void), context: *mut c_void) {
        ASYNC_RUNS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the job and context offload passed.
        unsafe { job(context) };
    }

    unsafe extern "C" fn in_coroutine() -> bool {
        true
    }

    static OPS_TABLE: Ops = Ops {
        run_async,
        in_coroutine,
    };

    #[test]
    fn offload_runs_on_the_pool_only_when_hooked() {
        // SAFETY: a static table.
        unsafe { swr_v1_odbc_init(&raw const OPS_TABLE) };
        swoole_odbc_set_blocking(true);
        assert_eq!(offload(|| 7), 7);
        assert_eq!(ASYNC_RUNS.load(Ordering::Relaxed), 0);
        swoole_odbc_set_blocking(false);
        assert_eq!(offload(|| 8), 8);
        assert_eq!(ASYNC_RUNS.load(Ordering::Relaxed), 1);
        swoole_odbc_set_blocking(true);
    }
}
