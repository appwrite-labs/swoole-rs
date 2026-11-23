//! Database-handle methods for the bundled PDO SQLite driver.
use crate::{
    Argument, Db, ExecCallback, Sql, Sqlite3, Sqlite3Stmt, sqlite3_changes, swoole_sqlite3_exec,
    swoole_sqlite3_open_v2, swoole_sqlite3_prepare_v2,
};
use std::{
    ffi::{CStr, CString, c_char, c_int},
    ptr,
};
use swoole_rs_corebase::{CCast, sys::Text};
use swoole_rs_zend::{
    E_WARNING, Owned, Value, ZStr, ZString, bailout,
    pdo::{self, Attribute, Dbh, Slot},
    string::ZBuf,
    sys::{
        PHP_VERSION_ID, pdo_attribute_type_PDO_ATTR_CLIENT_VERSION as CLIENT_VERSION,
        pdo_attribute_type_PDO_ATTR_CURSOR as CURSOR,
        pdo_attribute_type_PDO_ATTR_DRIVER_SPECIFIC as DRIVER_SPECIFIC,
        pdo_attribute_type_PDO_ATTR_SERVER_VERSION as SERVER_VERSION,
        pdo_attribute_type_PDO_ATTR_TIMEOUT as TIMEOUT,
        pdo_cursor_type_PDO_CURSOR_FWDONLY as FORWARD,
        pdo_error_mode_PDO_ERRMODE_EXCEPTION as ERRMODE_EXCEPTION,
        pdo_error_mode_PDO_ERRMODE_WARNING as ERRMODE_WARNING, pdo_param_type, zend_long,
        zend_string,
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

/// `sqlite3_mprintf("'%q'", value)`: the bytes before the first NUL, quoted.
fn quote_literal(value: &CStr) -> ZString {
    let bytes = value.to_bytes();
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
    result.into_zstring()
}

swoole_rs_zend::exports! {
/// `quoter`. PHP 8.5 refuses a NUL, as `PDO::$errorMode` says; earlier versions quote
/// the bytes before it.
#[unsafe(no_mangle)]
extern "C" fn swr_pdo_sqlite_quote(dbh: &Dbh, unquoted: &ZStr, _kind: pdo_param_type) -> *mut zend_string {
    // (INT_MAX - 3) / 2: SQLite's %q doubles every byte and adds the quotes.
    if unquoted.bytes().len() > 0x3fff_fffe {
        return ptr::null_mut();
    }
    if PHP_VERSION_ID >= 80500 && unquoted.bytes().contains(&0) {
        let message = c"SQLite PDO::quote does not support null bytes";
        match dbh.error_mode() {
            ERRMODE_EXCEPTION => dbh.throw_exception(0, message),
            ERRMODE_WARNING => bailout::defer(swoole_rs_zend::report(E_WARNING, message)),
            _ => {}
        }
        return ptr::null_mut();
    }
    quote_literal(unquoted.c_str()).into_raw()
}
}

/// `last_id`: the connection's last rowid, as decimal text.
#[unsafe(no_mangle)]
extern "C" fn swr_pdo_sqlite_last_insert_id(dbh: &Dbh, _name: Option<&ZStr>) -> *mut zend_string {
    unsafe extern "C" {
        safe fn sqlite3_last_insert_rowid(db: Db) -> i64;
    }
    let rowid = sqlite3_last_insert_rowid(swr_pdo_sqlite_connection(dbh));
    ZString::new(rowid.to_string().as_bytes()).into_raw()
}

/// `fetch_err`: the driver's code and message, once SQLite has reported an error.
#[unsafe(no_mangle)]
extern "C" fn swr_pdo_sqlite_error_info(info: &Slot, code: u32, message: Option<Text<'_>>) {
    if code != 0 {
        info.push_error_info(Some(code), message.map(|message| message.get().to_bytes()));
    }
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
    safe fn sqlite3_get_autocommit(db: Db) -> c_int;
    /// `expand_filepath(path, real_path)`: the absolute form of `path` in `real_path`, a
    /// `MAXPATHLEN` buffer, or null. It reads this thread's working directory and runs
    /// no PHP code.
    safe fn expand_filepath(path: Text<'_>, real_path: &mut [u8; PATH_BUFFER]) -> *mut c_char;
    /// `php_check_open_basedir_ex(path, warn)` into `refused`, inside `zend_try`: with
    /// `warn`, a refusal is an `E_WARNING`. 1 if it bailed out.
    safe fn swr_zend_try_open_basedir(path: Text<'_>, warn: c_int, refused: &mut bool) -> c_int;
}

/// At least `MAXPATHLEN`, which is `PATH_MAX`: 4096 on Linux, 1024 on macOS.
const PATH_BUFFER: usize = 4096;

/// `make_filename_safe`: the file SQLite may open for a data source, or none when
/// `open_basedir` refuses it. A `file:` URI is refused whenever `open_basedir` is set,
/// since SQLite would resolve it; a path is made absolute and checked, warning when
/// refused; an empty name and `:memory:` open no file. A bailout is deferred.
fn local_file<'a>(source: &'a CStr, expanded: &'a mut [u8; PATH_BUFFER]) -> Option<&'a CStr> {
    let bytes = source.to_bytes();
    if bytes
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case(b"file:"))
    {
        return (!swr_sqlite_open_basedir_active()).then_some(source);
    }
    if bytes.is_empty() || bytes == b":memory:" {
        return Some(source);
    }
    if expand_filepath(source.into(), expanded).is_null() {
        return None;
    }
    let path = CStr::from_bytes_until_nul(expanded).ok()?;
    let mut refused = true;
    if let Err(bailed) = bailout::trapped(swr_zend_try_open_basedir(path.into(), 1, &mut refused)) {
        bailed.defer();
        return None;
    }
    (!refused).then_some(path)
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

/// The factory's work once it has allocated the connection record: open the data
/// source's file, with the options' flags and busy timeout, and set the handle's flags.
/// A refused file throws; a failed open records SQLite's error.
#[unsafe(no_mangle)]
extern "C" fn swr_pdo_sqlite_open(dbh: &Dbh, source: Option<Text<'_>>, db: &mut *mut Sqlite3, options: Option<&Value>) -> bool {
    let mut expanded = [0; PATH_BUFFER];
    let Some(filename) = source.and_then(|source| local_file(source.get(), &mut expanded)) else {
        if !bailout::pending() {
            let mut message = b"open_basedir prohibits opening ".to_vec();
            message.extend_from_slice(source.map_or(b"(null)", |source| source.get().to_bytes()));
            dbh.throw_exception(0, &CString::new(message).unwrap_or_default());
        }
        return false;
    };
    // SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE, with URI filenames enabled
    // when PHP permits them. Conversion can invoke PHP's error handler.
    let mut flags = pdo::attr_long(options, DRIVER_SPECIFIC, 0x02 | 0x04).c_cast::<c_int>();
    if bailout::pending() { return false; }
    if !swr_sqlite_open_basedir_active() { flags |= 0x40; }
    if swoole_sqlite3_open_v2(filename.into(), db, flags, None) != 0 {
        swr_pdo_sqlite_record_error(dbh);
        return false;
    }
    let seconds = pdo::attr_long(options, TIMEOUT, 60);
    if bailout::pending() { return false; }
    set_timeout(swr_pdo_sqlite_connection(dbh), seconds);
    // Every parameter event but PDO_PARAM_EVT_EXEC_PRE is skipped; a quoted byte
    // takes at most two.
    dbh.set_driver_flags(0x7f ^ (1 << 2), 2);
    true
}
}
