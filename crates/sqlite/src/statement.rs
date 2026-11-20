//! Result-column values and metadata for the bundled PDO SQLite statement.
use crate::Stmt;
use std::{ffi::c_int, mem::MaybeUninit};
use swoole_rs_corebase::sys::Text;
use swoole_rs_zend::{
    Owned, OwnedArray, ZStr, ZString,
    pdo::Slot,
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
