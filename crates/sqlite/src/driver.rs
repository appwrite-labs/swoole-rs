//! Database-handle methods for the bundled PDO SQLite driver.
use crate::{Argument, Db, ExecCallback, swoole_sqlite3_exec};
use std::{
    ffi::{CStr, c_int},
    ptr,
};
use swoole_rs_corebase::sys::Text;
use swoole_rs_zend::{
    ZStr,
    pdo::Dbh,
    string::ZBuf,
    sys::{zend_long, zend_string},
};

#[unsafe(no_mangle)]
extern "C" fn pdo_sqlite_sqlstate(code: c_int, state: &mut [u8; 6]) {
    *state = *match code {
        0 => b"00000\0",  // SQLITE_OK
        9 => b"01002\0",  // SQLITE_INTERRUPT
        12 => b"42S02\0", // SQLITE_NOTFOUND
        18 => b"22001\0", // SQLITE_TOOBIG
        19 => b"23000\0", // SQLITE_CONSTRAINT
        22 => b"HYC00\0", // SQLITE_NOLFS
        _ => b"HY000\0",
    };
}

/// The caller checks SQLite's length limit and handles PHP 8.5's NUL diagnostic.
/// Earlier PHP versions quote the bytes before the first NUL, as SQLite's %q does.
#[unsafe(no_mangle)]
extern "C" fn pdo_sqlite_quote_literal(value: Text<'_>) -> *mut zend_string {
    let bytes = value.get().to_bytes();
    let mut parts = bytes.split(|&byte| byte == b'\'');
    let first = parts.next().unwrap_or_default();
    let mut result = ZBuf::with_capacity(bytes.len() + parts.clone().count() + 2);
    result.extend_from_slice(b"'");
    result.extend_from_slice(first);
    for part in parts {
        result.extend_from_slice(b"''");
        result.extend_from_slice(part);
    }
    result.extend_from_slice(b"'");
    result.into_zstring().into_raw()
}

unsafe extern "C" {
    safe fn swr_pdo_sqlite_connection(dbh: &Dbh) -> Db;
    safe fn swr_pdo_sqlite_transaction_mode(dbh: &Dbh) -> c_int;
    safe fn swr_pdo_sqlite_record_error(dbh: &Dbh);
    safe fn sqlite3_changes(db: Db) -> c_int;
    safe fn sqlite3_get_autocommit(db: Db) -> c_int;
}

/// Execute on the coroutine hook and leave the driver's diagnostic on failure.
fn execute(dbh: &Dbh, sql: &CStr) -> Option<Db> {
    let db = swr_pdo_sqlite_connection(dbh);
    let rc = swoole_sqlite3_exec(
        db,
        Some(sql.into()),
        ExecCallback(ptr::null()),
        Argument(ptr::null_mut()),
        None,
    );
    if rc == 0 {
        Some(db)
    } else {
        swr_pdo_sqlite_record_error(dbh);
        None
    }
}

#[unsafe(no_mangle)]
extern "C" fn swr_pdo_sqlite_exec(dbh: &Dbh, sql: &ZStr) -> zend_long {
    execute(dbh, sql.c_str()).map_or(-1, |db| sqlite3_changes(db).into())
}

#[unsafe(no_mangle)]
extern "C" fn swr_pdo_sqlite_begin(dbh: &Dbh) -> bool {
    let sql = match swr_pdo_sqlite_transaction_mode(dbh) {
        0 => c"BEGIN DEFERRED TRANSACTION",
        1 => c"BEGIN IMMEDIATE TRANSACTION",
        2 => c"BEGIN EXCLUSIVE TRANSACTION",
        _ => c"BEGIN",
    };
    execute(dbh, sql).is_some()
}

#[unsafe(no_mangle)]
extern "C" fn swr_pdo_sqlite_commit(dbh: &Dbh) -> bool {
    execute(dbh, c"COMMIT").is_some()
}

#[unsafe(no_mangle)]
extern "C" fn swr_pdo_sqlite_rollback(dbh: &Dbh) -> bool {
    execute(dbh, c"ROLLBACK").is_some()
}

#[unsafe(no_mangle)]
extern "C" fn swr_pdo_sqlite_in_transaction(dbh: &Dbh) -> bool {
    let db = swr_pdo_sqlite_connection(dbh);
    !db.0.is_null() && sqlite3_get_autocommit(db) == 0
}
