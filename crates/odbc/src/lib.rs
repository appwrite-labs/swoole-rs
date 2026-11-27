//! Swoole's coroutine ODBC calls (upstream's `swoole_odbc.cc`, built with
//! `--with-swoole-odbc`): the `SQL*` replacements its bundled `pdo_odbc`
//! driver calls. While the runtime hook is on and a coroutine runs, each
//! blocking call runs on Swoole's thread pool and the coroutine waits for it
//! (native `async`, through the hooks' host); the cheap ones call through.
//! With `php`, MINIT ([`minit`]) registers the driver with PDO.
//!
//! The exports keep upstream's names: `native/hooks/php_swoole_odbc.h` declares them and
//! the driver's macros call them. Upstream's pass-throughs (`SQLGetInfo`,
//! `SQLGetDiagRec`, `SQLRowCount`), which only
//! traced, are not redirected: the driver calls ODBC for those.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

#[cfg(feature = "php")]
mod diagnostics;
#[cfg(feature = "php")]
mod driver;
#[cfg(feature = "php")]
pub mod minit;
#[cfg(feature = "php")]
mod statement;

use std::sync::atomic::{AtomicBool, Ordering};
use swoole_rs_odbc_sys::{
    SQLCHAR, SQLHANDLE, SQLHDBC, SQLHSTMT, SQLHWND, SQLINTEGER, SQLLEN, SQLPOINTER, SQLRETURN,
    SQLSMALLINT, SQLULEN, SQLUSMALLINT,
};

// The driver's arguments, each typed as what ODBC requires of it. Nothing here makes
// one: only the exports' parameters do, where the C caller's contract (the ODBC call
// each forwards to, as `php_swoole_odbc.h` declares) stands for the constructor. None
// is `Send` or `Sync`, so they reach the pool only through [`offload`].

/// A connection handle the driver handed over: allocated and live, as ODBC's own
/// calls require, for as long as the value is used.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Connection(SQLHDBC);

/// A statement handle the driver handed over: live, with the parameter buffers it
/// bound live, as ODBC's own calls require, for as long as the value is used.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Statement(SQLHSTMT);

/// An environment, statement or connection handle, live and of the handle type passed beside it.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Handle(SQLHANDLE);

/// The parent window `SQLDriverConnect` may prompt over: null or a live window.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Window(SQLHWND);

/// A string the driver handed over: null where the call allows none, else readable
/// for the length passed beside it, or up to its NUL when that length is `SQL_NTS`.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Text(*mut SQLCHAR);

/// A value `SQLPutData` sends: readable for the length passed beside it in the C
/// type the parameter was bound with.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Data(SQLPOINTER);

/// Where the call writes its `T`s: null where the call allows none, else writable for
/// the capacity passed beside it, or for one `T` when none is.
#[repr(transparent)]
pub struct Out<T>(*mut T);

impl<T> Clone for Out<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Out<T> {}

/// [`Out`] for a target whose C type the call is told beside it.
pub type Target = Out<std::ffi::c_void>;

// ODBC's own declarations, each argument typed as what the call requires of it. None
// keeps an argument past the call (`SQLExecute` reads only the buffers the statement
// bound, which its type keeps live) or runs a callback.
unsafe extern "C" {
    #[link_name = "SQLConnect"]
    safe fn connect(
        connection: Connection,
        server_name: Text,
        name_length1: SQLSMALLINT,
        user_name: Text,
        name_length2: SQLSMALLINT,
        authentication: Text,
        name_length3: SQLSMALLINT,
    ) -> SQLRETURN;
    #[link_name = "SQLDriverConnect"]
    safe fn driver_connect(
        connection: Connection,
        window: Window,
        conn_str_in: Text,
        conn_str_in_length: SQLSMALLINT,
        conn_str_out: Out<SQLCHAR>,
        conn_str_out_max: SQLSMALLINT,
        conn_str_out_length: Out<SQLSMALLINT>,
        driver_completion: SQLUSMALLINT,
    ) -> SQLRETURN;
    #[link_name = "SQLExecDirect"]
    safe fn exec_direct(statement: Statement, text: Text, length: SQLINTEGER) -> SQLRETURN;
    #[link_name = "SQLPrepare"]
    safe fn prepare(statement: Statement, text: Text, length: SQLINTEGER) -> SQLRETURN;
    #[link_name = "SQLExecute"]
    safe fn execute(statement: Statement) -> SQLRETURN;
    #[link_name = "SQLPutData"]
    safe fn put_data(statement: Statement, data: Data, length_or_indicator: SQLLEN) -> SQLRETURN;
    #[link_name = "SQLGetData"]
    safe fn get_data(
        statement: Statement,
        column: SQLUSMALLINT,
        target_type: SQLSMALLINT,
        target: Target,
        buffer_length: SQLLEN,
        length_or_indicator: Out<SQLLEN>,
    ) -> SQLRETURN;
    #[link_name = "SQLDescribeCol"]
    safe fn describe_col(
        statement: Statement,
        column: SQLUSMALLINT,
        name: Out<SQLCHAR>,
        buffer_length: SQLSMALLINT,
        name_length: Out<SQLSMALLINT>,
        data_type: Out<SQLSMALLINT>,
        column_size: Out<SQLULEN>,
        decimal_digits: Out<SQLSMALLINT>,
        nullable: Out<SQLSMALLINT>,
    ) -> SQLRETURN;
    #[link_name = "SQLEndTran"]
    safe fn end_tran(
        handle_type: SQLSMALLINT,
        handle: Handle,
        completion: SQLSMALLINT,
    ) -> SQLRETURN;
    #[link_name = "SQLFreeHandle"]
    safe fn free_handle(kind: i16, handle: Handle) -> SQLRETURN;
    #[link_name = "SQLCloseCursor"]
    safe fn close_cursor(statement: Statement) -> SQLRETURN;
    #[link_name = "SQLDisconnect"]
    safe fn disconnect(connection: Connection) -> SQLRETURN;
}

/// `SQL_ERROR`.
const SQL_ERROR: SQLRETURN = -1;

static BLOCKING: AtomicBool = AtomicBool::new(true);

/// `swoole_odbc_set_blocking`: the runtime hook flag.
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

/// `swoole_odbc_SQLConnect`: `SQLConnect`, whose whole contract the arguments' types carry.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_odbc_SQLConnect(
    connection: Connection,
    server_name: Text,
    name_length1: SQLSMALLINT,
    user_name: Text,
    name_length2: SQLSMALLINT,
    authentication: Text,
    name_length3: SQLSMALLINT,
) -> SQLRETURN {
    offload(|| {
        connect(
            connection,
            server_name,
            name_length1,
            user_name,
            if user_name.0.is_null() {
                0
            } else {
                name_length2
            },
            authentication,
            if authentication.0.is_null() {
                0
            } else {
                name_length3
            },
        )
    })
}

/// `swoole_odbc_SQLDriverConnect`: `SQLDriverConnect`, as [`swoole_odbc_SQLConnect`].
#[unsafe(no_mangle)]
pub extern "C" fn swoole_odbc_SQLDriverConnect(
    connection: Connection,
    window: Window,
    conn_str_in: Text,
    conn_str_in_length: SQLSMALLINT,
    conn_str_out: Out<SQLCHAR>,
    conn_str_out_max: SQLSMALLINT,
    conn_str_out_length: Out<SQLSMALLINT>,
    driver_completion: SQLUSMALLINT,
) -> SQLRETURN {
    offload(|| {
        driver_connect(
            connection,
            window,
            conn_str_in,
            conn_str_in_length,
            conn_str_out,
            conn_str_out_max,
            conn_str_out_length,
            driver_completion,
        )
    })
}

/// `swoole_odbc_SQLExecDirect`: `SQLExecDirect`, as [`swoole_odbc_SQLConnect`].
#[unsafe(no_mangle)]
pub extern "C" fn swoole_odbc_SQLExecDirect(
    statement: Statement,
    text: Text,
    length: SQLINTEGER,
) -> SQLRETURN {
    offload(|| exec_direct(statement, text, length))
}

/// `swoole_odbc_SQLPrepare`: `SQLPrepare`, as [`swoole_odbc_SQLConnect`].
#[unsafe(no_mangle)]
pub extern "C" fn swoole_odbc_SQLPrepare(
    statement: Statement,
    text: Text,
    length: SQLINTEGER,
) -> SQLRETURN {
    offload(|| prepare(statement, text, length))
}

/// `swoole_odbc_SQLExecute`: `SQLExecute`, as [`swoole_odbc_SQLConnect`].
#[unsafe(no_mangle)]
pub extern "C" fn swoole_odbc_SQLExecute(statement: Statement) -> SQLRETURN {
    offload(|| execute(statement))
}

/// Closing an unread cursor can wait for the remaining rows.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_odbc_SQLCloseCursor(statement: Statement) -> SQLRETURN {
    offload(|| close_cursor(statement))
}

const SQL_HANDLE_STMT: SQLSMALLINT = 3;

/// Only statement destruction can wait for the server after the cursor closes.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_odbc_SQLFreeHandle(kind: SQLSMALLINT, handle: Handle) -> SQLRETURN {
    if kind == SQL_HANDLE_STMT {
        offload(|| free_handle(kind, handle))
    } else {
        free_handle(kind, handle)
    }
}

/// `swoole_odbc_SQLPutData`: `SQLPutData`, as [`swoole_odbc_SQLConnect`].
#[unsafe(no_mangle)]
pub extern "C" fn swoole_odbc_SQLPutData(
    statement: Statement,
    data: Data,
    length_or_indicator: SQLLEN,
) -> SQLRETURN {
    offload(|| put_data(statement, data, length_or_indicator))
}

/// `swoole_odbc_SQLGetData`: `SQLGetData`, as [`swoole_odbc_SQLConnect`].
#[unsafe(no_mangle)]
pub extern "C" fn swoole_odbc_SQLGetData(
    statement: Statement,
    column: SQLUSMALLINT,
    target_type: SQLSMALLINT,
    target: Target,
    buffer_length: SQLLEN,
    length_or_indicator: Out<SQLLEN>,
) -> SQLRETURN {
    offload(|| {
        get_data(
            statement,
            column,
            target_type,
            target,
            buffer_length,
            length_or_indicator,
        )
    })
}

/// `swoole_odbc_SQLDescribeCol`: `SQLDescribeCol`, as [`swoole_odbc_SQLConnect`].
#[unsafe(no_mangle)]
pub extern "C" fn swoole_odbc_SQLDescribeCol(
    statement: Statement,
    column: SQLUSMALLINT,
    name: Out<SQLCHAR>,
    buffer_length: SQLSMALLINT,
    name_length: Out<SQLSMALLINT>,
    data_type: Out<SQLSMALLINT>,
    column_size: Out<SQLULEN>,
    decimal_digits: Out<SQLSMALLINT>,
    nullable: Out<SQLSMALLINT>,
) -> SQLRETURN {
    offload(|| {
        describe_col(
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
    })
}

/// `swoole_odbc_SQLEndTran`: `SQLEndTran`, as [`swoole_odbc_SQLConnect`].
#[unsafe(no_mangle)]
pub extern "C" fn swoole_odbc_SQLEndTran(
    handle_type: SQLSMALLINT,
    handle: Handle,
    completion: SQLSMALLINT,
) -> SQLRETURN {
    offload(|| end_tran(handle_type, handle, completion))
}

/// `swoole_odbc_SQLDisconnect`: `SQLDisconnect`, as [`swoole_odbc_SQLConnect`].
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
