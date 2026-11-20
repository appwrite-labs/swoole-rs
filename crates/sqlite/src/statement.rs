//! Cursor execution, fetching and result columns for the bundled PDO SQLite statement.
use crate::{Db, Stmt, sqlite3_changes, swoole_sqlite3_step};
use std::{cell::Cell, ffi::c_int, mem::MaybeUninit};
use swoole_rs_corebase::sys::Text;
use swoole_rs_zend::{
    Owned, OwnedArray, ZStr, ZString,
    pdo::{Slot, Stmt as PdoStmt},
    sys::{
        _zend_known_string_id_ZEND_STR_DOUBLE as DOUBLE,
        _zend_known_string_id_ZEND_STR_INTEGER as INTEGER,
        _zend_known_string_id_ZEND_STR_NULL_LOWERCASE as NULL_NAME,
        _zend_known_string_id_ZEND_STR_STRING as STRING, pdo_param_type_PDO_PARAM_INT as INT,
        pdo_param_type_PDO_PARAM_NULL as NULL, pdo_param_type_PDO_PARAM_STR as STR, zval,
    },
};

// SQLite storage classes shared by fetch and getColumnMeta.
const SQLITE_INTEGER: c_int = 1;
const SQLITE_FLOAT: c_int = 2;
const SQLITE_TEXT: c_int = 3;
const SQLITE_BLOB: c_int = 4;
const SQLITE_NULL: c_int = 5;

#[unsafe(no_mangle)]
extern "C" fn swr_sqlite_column_value(stmt: Stmt, column: c_int, output: &mut MaybeUninit<zval>) {
    unsafe extern "C" {
        safe fn sqlite3_column_type(stmt: Stmt, column: c_int) -> c_int;
        safe fn sqlite3_column_int64(stmt: Stmt, column: c_int) -> i64;
        safe fn sqlite3_column_double(stmt: Stmt, column: c_int) -> f64;
        safe fn swr_sqlite_column_string(stmt: Stmt, column: c_int, blob: bool) -> ZString;
    }
    let value = match sqlite3_column_type(stmt, column) {
        SQLITE_NULL => Owned::null(),
        SQLITE_INTEGER => {
            let number = sqlite3_column_int64(stmt, column);
            #[cfg(target_pointer_width = "32")]
            let value = i32::try_from(number).map_or_else(
                |_| Owned::string(swr_sqlite_column_string(stmt, column, false)),
                |n| Owned::long(n.into()),
            );
            #[cfg(target_pointer_width = "64")]
            let value = Owned::long(number);
            value
        }
        SQLITE_FLOAT => Owned::double(sqlite3_column_double(stmt, column)),
        kind => Owned::string(swr_sqlite_column_string(stmt, column, kind == SQLITE_BLOB)),
    };
    output.write(value.into_zval());
}

#[unsafe(no_mangle)]
extern "C" fn swr_sqlite_column_meta(
    output: &Slot,
    kind: c_int,
    declared: Option<Text<'_>>,
    table: Option<Text<'_>>,
) {
    unsafe extern "C" {
        /// Called only with Zend's known string IDs below.
        safe fn swr_sqlite_known_type(id: u32) -> &'static ZStr;
    }
    let mut meta = OwnedArray::default();
    let mut flags = OwnedArray::default();
    let types = match kind {
        SQLITE_INTEGER => Some((INTEGER, INT)),
        SQLITE_FLOAT => Some((DOUBLE, STR)),
        SQLITE_TEXT => Some((STRING, STR)),
        SQLITE_BLOB => {
            flags.push_bytes(b"blob");
            Some((STRING, STR))
        }
        SQLITE_NULL => Some((NULL_NAME, NULL)),
        _ => None,
    };
    if let Some((name, param)) = types {
        meta.insert(
            b"native_type",
            Owned::string(swr_sqlite_known_type(name).to_owned()),
        );
        meta.insert_long(b"pdo_type", param.into());
    }
    if let Some(declared) = declared {
        meta.insert_bytes(b"sqlite:decl_type", Some(declared.get().to_bytes()));
    }
    if let Some(table) = table {
        meta.insert_bytes(b"table", Some(table.get().to_bytes()));
    }
    meta.insert(b"flags", flags.into_owned());
    output.set(meta.into_owned());
}

// The bundled statement is zero-initialized. Read the bits afresh after each
// coroutine-aware step, since another PHP call may have accessed the cursor.
const PREFETCHED: u8 = 1;
const DONE: u8 = 2;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;

unsafe extern "C" {
    safe fn sqlite3_reset(stmt: Stmt) -> c_int;
    safe fn sqlite3_data_count(stmt: Stmt) -> c_int;
    safe fn sqlite3_column_count(stmt: Stmt) -> c_int;
    safe fn php_pdo_stmt_set_column_count(stmt: &PdoStmt, columns: c_int);
    safe fn swr_sqlite_stmt_error(stmt: &PdoStmt);
}

fn step_error(pdo: &PdoStmt, stmt: Stmt, code: c_int) -> bool {
    if code == 1 {
        // SQLITE_ERROR
        sqlite3_reset(stmt);
    }
    swr_sqlite_stmt_error(pdo);
    false
}

#[unsafe(no_mangle)]
extern "C" fn swr_sqlite_before_bind(stmt: Stmt, cursor: &Cell<u8>, executed: bool) {
    if executed && cursor.get() & DONE == 0 {
        sqlite3_reset(stmt);
        cursor.update(|bits| bits | DONE);
    }
}

#[unsafe(no_mangle)]
extern "C" fn swr_sqlite_execute(
    pdo: &PdoStmt,
    stmt: Stmt,
    db: Db,
    cursor: &Cell<u8>,
    executed: bool,
) -> bool {
    if executed && cursor.get() & DONE == 0 {
        sqlite3_reset(stmt);
    }
    cursor.update(|bits| bits & !DONE);
    match swoole_sqlite3_step(stmt) {
        SQLITE_ROW => {
            cursor.update(|bits| bits | PREFETCHED);
            php_pdo_stmt_set_column_count(pdo, sqlite3_data_count(stmt));
            true
        }
        SQLITE_DONE => {
            php_pdo_stmt_set_column_count(pdo, sqlite3_column_count(stmt));
            pdo.set_row_count(sqlite3_changes(db).into());
            sqlite3_reset(stmt);
            cursor.update(|bits| bits | DONE);
            true
        }
        code => step_error(pdo, stmt, code),
    }
}

#[unsafe(no_mangle)]
extern "C" fn swr_sqlite_fetch(pdo: &PdoStmt, stmt: Stmt, cursor: &Cell<u8>) -> bool {
    if stmt.0.is_null() {
        return false;
    }
    if cursor.get() & PREFETCHED != 0 {
        cursor.update(|bits| bits & !PREFETCHED);
        return true;
    }
    if cursor.get() & DONE != 0 {
        return false;
    }
    match swoole_sqlite3_step(stmt) {
        SQLITE_ROW => true,
        SQLITE_DONE => {
            cursor.update(|bits| bits | DONE);
            sqlite3_reset(stmt);
            false
        }
        code => step_error(pdo, stmt, code),
    }
}
