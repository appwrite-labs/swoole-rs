//! Database-handle methods for the bundled PDO SQLite driver.
use crate::{
    Argument, Db, ExecCallback, Sql, Sqlite3, Sqlite3Stmt, swoole_sqlite3_exec,
    swoole_sqlite3_open_v2, swoole_sqlite3_prepare_v2,
};
use std::{
    ffi::{CStr, c_int},
    ptr,
};
use swoole_rs_corebase::{CCast, sys::Text};
use swoole_rs_zend::{
    Owned, Value, ZStr, ZString,
    pdo::{self, Attribute, Dbh, Slot},
    string::ZBuf,
    sys::{
        pdo_attribute_type_PDO_ATTR_CLIENT_VERSION as CLIENT_VERSION,
        pdo_attribute_type_PDO_ATTR_CURSOR as CURSOR,
        pdo_attribute_type_PDO_ATTR_DRIVER_SPECIFIC as DRIVER_SPECIFIC,
        pdo_attribute_type_PDO_ATTR_SERVER_VERSION as SERVER_VERSION,
        pdo_attribute_type_PDO_ATTR_TIMEOUT as TIMEOUT,
        pdo_cursor_type_PDO_CURSOR_FWDONLY as FORWARD, zend_long, zend_string,
    },
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
    safe fn swr_pdo_sqlite_set_transaction_mode(dbh: &Dbh, mode: c_int);
    safe fn sqlite3_libversion() -> Text<'static>;
    safe fn sqlite3_busy_timeout(db: Db, milliseconds: c_int) -> c_int;
    safe fn sqlite3_extended_result_codes(db: Db, enabled: c_int) -> c_int;
    safe fn swr_sqlite_open_basedir_active() -> bool;
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

const EXTENDED_RESULT_CODES: u32 = DRIVER_SPECIFIC + 2;
const TRANSACTION_MODE: u32 = DRIVER_SPECIFIC + 5;

#[unsafe(no_mangle)]
extern "C" fn swr_pdo_sqlite_get_attribute(dbh: &Dbh, attr: zend_long, result: &Slot) -> Attribute {
    let value = match u32::try_from(attr) {
        Ok(CLIENT_VERSION | SERVER_VERSION) => {
            Owned::string(ZString::new(sqlite3_libversion().get().to_bytes()))
        }
        Ok(TRANSACTION_MODE) => Owned::long(swr_pdo_sqlite_transaction_mode(dbh).into()),
        _ => return Attribute::Unknown,
    };
    result.set(value);
    Attribute::Set
}

#[unsafe(no_mangle)]
extern "C" fn swr_pdo_sqlite_set_attribute(dbh: &Dbh, attr: zend_long, value: &Value) -> bool {
    let Ok(attr @ (TIMEOUT | EXTENDED_RESULT_CODES | TRANSACTION_MODE)) = u32::try_from(attr)
    else {
        return false;
    };
    let Some(value) = pdo::long_param(value) else {
        return false;
    };
    match attr {
        TIMEOUT => {
            set_timeout(swr_pdo_sqlite_connection(dbh), value);
        }
        EXTENDED_RESULT_CODES => {
            sqlite3_extended_result_codes(swr_pdo_sqlite_connection(dbh), value.c_cast());
        }
        _ if (0..=2).contains(&value) => swr_pdo_sqlite_set_transaction_mode(dbh, value.c_cast()),
        _ => return false,
    }
    true
}

fn set_timeout(db: Db, seconds: zend_long) {
    sqlite3_busy_timeout(db, seconds.c_cast::<c_int>().wrapping_mul(1000));
}

swoole_rs_zend::exports! {
#[unsafe(no_mangle)]
extern "C" fn swr_pdo_sqlite_prepare(dbh: &Dbh, sql: &ZStr, options: Option<&Value>, stmt: &mut *mut Sqlite3Stmt) -> bool {
    let cursor = pdo::attr_long(options, CURSOR, FORWARD.into());
    if swoole_rs_zend::bailout::pending() { return false; }
    if cursor == FORWARD.into() {
        let sql_ptr = Sql(sql.bytes_with_nul().as_ptr().cast());
        let result = swoole_sqlite3_prepare_v2(
            swr_pdo_sqlite_connection(dbh), sql_ptr, sql.bytes().len().c_cast(), stmt, None,
        );
        if result == 0 { return true; }
    }
    swr_pdo_sqlite_record_error(dbh);
    false
}

#[unsafe(no_mangle)]
extern "C" fn swr_pdo_sqlite_open(filename: Text<'_>, db: &mut *mut Sqlite3, options: Option<&Value>) -> bool {
    // SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE, with URI filenames enabled
    // when PHP permits them. Conversion can invoke PHP's error handler.
    let mut flags = pdo::attr_long(options, DRIVER_SPECIFIC, 0x02 | 0x04).c_cast::<c_int>();
    if swoole_rs_zend::bailout::pending() { return false; }
    if !swr_sqlite_open_basedir_active() { flags |= 0x40; }
    swoole_sqlite3_open_v2(filename, db, flags, None) == 0
}

#[unsafe(no_mangle)]
extern "C" fn swr_pdo_sqlite_initial_timeout(db: Db, options: Option<&Value>) {
    let seconds = pdo::attr_long(options, TIMEOUT, 60);
    if !swoole_rs_zend::bailout::pending() { set_timeout(db, seconds); }
}
}
