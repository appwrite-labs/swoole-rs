//! PDO connection callbacks of the bundled ODBC driver.
use crate::diagnostics::pdo_odbc_error;
use std::ffi::c_int;
use swoole_rs_corebase::{CCast, sys::Text};
use swoole_rs_odbc_sys::SQLRETURN;
use swoole_rs_zend::pdo::Dbh;

use crate::{Connection, Handle, Statement, swoole_odbc_SQLEndTran, swoole_odbc_SQLExecDirect};
use swoole_rs_zend::{ZStr, sys::zend_long};

unsafe extern "C" {
    /// The connection in this ODBC driver's handle record.
    pub(crate) safe fn swr_odbc_connection(dbh: &Dbh) -> Connection;
    /// `SQL_ATTR_AUTOCOMMIT` accepts its integer value in the pointer argument.
    #[link_name = "SQLSetConnectAttr"]
    safe fn set_connect_attr(
        connection: Connection,
        attr: i32,
        value: usize,
        length: i32,
    ) -> SQLRETURN;
}

fn autocommit(dbh: &Dbh, on: bool, file: Text<'static>, line: c_int) -> bool {
    // SQL_ATTR_AUTOCOMMIT and SQL_IS_INTEGER from sqlext.h.
    if set_connect_attr(swr_odbc_connection(dbh), 102, usize::from(on), -6) == 0 {
        return true;
    }
    let what = if on {
        c"SQLSetConnectAttr AUTOCOMMIT = ON"
    } else {
        c"SQLSetConnectAttr AUTOCOMMIT = OFF"
    };
    pdo_odbc_error(dbh, None, Statement::NULL, what.into(), file, line);
    false
}

#[unsafe(no_mangle)]
extern "C" fn swr_odbc_begin(dbh: &Dbh, file: Text<'static>, line: c_int) -> bool {
    !dbh.auto_commit() || autocommit(dbh, false, file, line)
}

#[unsafe(no_mangle)]
extern "C" fn swr_odbc_finish(
    dbh: &Dbh,
    commit: bool,
    file: Text<'static>,
    end_line: c_int,
    reset_line: c_int,
) -> bool {
    // SQL_HANDLE_DBC, SQL_COMMIT (0) and SQL_ROLLBACK (1).
    let rc = swoole_odbc_SQLEndTran(2, Handle(swr_odbc_connection(dbh).0), i16::from(!commit));
    if rc != 0 {
        let what = if commit {
            c"SQLEndTran: Commit"
        } else {
            c"SQLEndTran: Rollback"
        };
        pdo_odbc_error(dbh, None, Statement::NULL, what.into(), file, end_line);
        if rc != 1 {
            // SQL_SUCCESS_WITH_INFO
            return false;
        }
    }
    !dbh.auto_commit()
        || (!commit && swr_odbc_connection(dbh).0.is_null())
        || autocommit(dbh, true, file, reset_line)
}

unsafe extern "C" {
    // SQLGetInfo writes at most capacity bytes. Both callers pass a capacity
    // within the borrowed array and request a string-valued information type.
    #[link_name = "SQLGetInfo"]
    safe fn get_info(
        connection: Connection,
        kind: u16,
        buffer: &mut [u8; 256],
        capacity: i16,
        length: &mut i16,
    ) -> SQLRETURN;
    // SQL_ATTR_CONNECTION_DEAD returns a SQLUINTEGER.
    #[link_name = "SQLGetConnectAttr"]
    safe fn get_connect_attr(
        connection: Connection,
        attr: i32,
        value: &mut u32,
        length: i32,
        used: Option<&mut i32>,
    ) -> SQLRETURN;
}

#[repr(i32)]
enum Liveness {
    Dead = -1,
    Live = 0,
}

#[repr(i32)]
enum Attribute {
    Error = -1,
    Unsupported = 0,
    Set = 1,
}

#[unsafe(no_mangle)]
extern "C" fn swr_odbc_check_liveness(dbh: &Dbh) -> Liveness {
    let connection = swr_odbc_connection(dbh);
    let mut dead = 0;
    if get_connect_attr(connection, 1209, &mut dead, 0, None) == 0 && dead == 1 {
        return Liveness::Dead;
    }
    // Drivers may omit CONNECTION_DEAD or return false for a dead connection.
    // Ask for SQL_DATA_SOURCE_READ_ONLY as the bundled driver does.
    let mut length = 0;
    if get_info(connection, 25, &mut [0; 256], 32, &mut length) == 0 && length != 0 {
        Liveness::Live
    } else {
        Liveness::Dead
    }
}

#[unsafe(no_mangle)]
extern "C" fn swr_odbc_get_attr(
    dbh: &Dbh,
    attr: swoole_rs_zend::sys::zend_long,
    value: &swoole_rs_zend::pdo::Slot,
    assume_utf8: bool,
    client_version: Text<'static>,
) -> Attribute {
    use swoole_rs_zend::{
        Owned, ZString,
        sys::{
            pdo_attribute_type_PDO_ATTR_AUTOCOMMIT as AUTOCOMMIT,
            pdo_attribute_type_PDO_ATTR_CLIENT_VERSION as CLIENT_VERSION,
            pdo_attribute_type_PDO_ATTR_DRIVER_SPECIFIC as DRIVER_SPECIFIC,
            pdo_attribute_type_PDO_ATTR_SERVER_INFO as SERVER_INFO,
            pdo_attribute_type_PDO_ATTR_SERVER_VERSION as SERVER_VERSION,
        },
    };
    let Ok(attr) = u32::try_from(attr) else {
        return Attribute::Unsupported;
    };
    let result = match attr {
        AUTOCOMMIT => Owned::bool(dbh.auto_commit()),
        n if n == DRIVER_SPECIFIC + 1 => Owned::bool(assume_utf8),
        CLIENT_VERSION => Owned::string(ZString::new(client_version.get().to_bytes())),
        SERVER_INFO | SERVER_VERSION => {
            // SQL_DBMS_NAME and SQL_DBMS_VER return strings.
            let kind = if attr == SERVER_INFO { 17 } else { 18 };
            let mut buffer = [0; 256];
            let mut length = 0;
            if !matches!(
                get_info(
                    swr_odbc_connection(dbh),
                    kind,
                    &mut buffer,
                    256,
                    &mut length
                ),
                0 | 1
            ) {
                return Attribute::Error;
            }
            let Ok(length) = usize::try_from(length) else {
                return Attribute::Error;
            };
            // SQLGetInfo reports the full length on truncation.
            Owned::string(ZString::new(
                buffer.split_at(length.min(buffer.len() - 1)).0,
            ))
        }
        _ => return Attribute::Unsupported,
    };
    value.set(result);
    Attribute::Set
}

impl Statement {
    const NULL: Self = Self(std::ptr::null_mut());
}

unsafe extern "C" {
    #[link_name = "SQLAllocHandle"]
    safe fn alloc_handle(kind: i16, parent: Handle, result: &mut Handle) -> SQLRETURN;
    #[link_name = "SQLRowCount"]
    safe fn row_count(statement: Statement, count: &mut swoole_rs_odbc_sys::SQLLEN) -> SQLRETURN;
    #[link_name = "SQLFreeHandle"]
    safe fn free_handle(kind: i16, handle: Handle) -> SQLRETURN;
}

/// `PDO::exec` allocates a statement for this call, including when it affects no rows.
#[unsafe(no_mangle)]
extern "C" fn swr_odbc_doer(
    dbh: &Dbh,
    sql: &ZStr,
    file: Text<'static>,
    alloc_line: c_int,
    exec_line: c_int,
    count_line: c_int,
) -> zend_long {
    const SQL_HANDLE_STMT: i16 = 3;
    let mut allocated = Handle(std::ptr::null_mut());
    if !matches!(
        alloc_handle(
            SQL_HANDLE_STMT,
            Handle(swr_odbc_connection(dbh).0),
            &mut allocated
        ),
        0 | 1
    ) {
        pdo_odbc_error(
            dbh,
            None,
            Statement::NULL,
            c"SQLAllocHandle: STMT".into(),
            file,
            alloc_line,
        );
        return -1;
    }
    let stmt = Statement(allocated.0);
    let query = sql.bytes_with_nul();
    let length = sql.bytes().len().c_cast::<i32>();
    let status = swoole_odbc_SQLExecDirect(stmt, crate::Text(query.as_ptr().cast_mut()), length);
    let mut count = -1;
    match status {
        100 => count = 0, // SQL_NO_DATA
        0 | 1 => {
            if !matches!(row_count(stmt, &mut count), 0 | 1) {
                pdo_odbc_error(dbh, None, stmt, c"SQLRowCount".into(), file, count_line);
            } else if count == -1 {
                count = 0;
            }
        }
        _ => pdo_odbc_error(dbh, None, stmt, c"SQLExecDirect".into(), file, exec_line),
    }
    free_handle(SQL_HANDLE_STMT, Handle(stmt.0));
    count
}

unsafe extern "C" {
    safe fn swr_odbc_set_utf8(dbh: &Dbh, value: bool);
    safe fn pdo_raise_impl_error(
        dbh: &Dbh,
        stmt: Option<&!>,
        sqlstate: Text<'static>,
        message: Text<'static>,
    );
}

swoole_rs_zend::exports! {
#[unsafe(no_mangle)]
extern "C" fn swr_odbc_set_attr(
    dbh: &Dbh,
    attr: zend_long,
    value: &swoole_rs_zend::Value,
    file: Text<'static>,
    line: c_int,
) -> bool {
    use swoole_rs_zend::sys::{
        pdo_attribute_type_PDO_ATTR_AUTOCOMMIT as AUTOCOMMIT,
        pdo_attribute_type_PDO_ATTR_DRIVER_SPECIFIC as DRIVER_SPECIFIC,
    };
    match u32::try_from(attr) {
        Ok(n) if n == DRIVER_SPECIFIC + 1 => {
            swoole_rs_zend::pdo::bool_param(value).is_some_and(|on| {
                swr_odbc_set_utf8(dbh, on);
                true
            })
        }
        Ok(AUTOCOMMIT) => {
            let Some(on) = swoole_rs_zend::pdo::bool_param(value) else {
                return false;
            };
            if dbh.in_txn() {
                swoole_rs_zend::bailout::defer(swoole_rs_zend::contain_with(|| pdo_raise_impl_error(
                    dbh,
                    None,
                    c"HY000".into(),
                    c"Cannot change autocommit mode while a transaction is already open".into(),
                )));
                return false;
            }
            if dbh.auto_commit() == on {
                return true;
            }
            dbh.set_auto_commit(on);
            autocommit(dbh, on, file, line)
        }
        _ => {
            crate::diagnostics::unknown_attribute(dbh);
            false
        }
    }
}
}

unsafe extern "C" {
    safe fn swr_odbc_free_record(dbh: &Dbh);
}

/// PDO invokes close on partially constructed handles as well.
#[unsafe(no_mangle)]
extern "C" fn swr_odbc_close(dbh: &Dbh, env: Handle) {
    let connection = swr_odbc_connection(dbh);
    if !connection.0.is_null() {
        swoole_odbc_SQLEndTran(2, Handle(connection.0), 1);
        crate::swoole_odbc_SQLDisconnect(connection);
        free_handle(2, Handle(connection.0));
    }
    free_handle(1, env);
    swr_odbc_free_record(dbh);
}

unsafe extern "C" {
    #[link_name = "SQLSetEnvAttr"]
    pub(crate) safe fn set_env_attr(env: Handle, attr: i32, value: usize, length: i32)
    -> SQLRETURN;
}

swoole_rs_zend::exports! {
/// Allocate the environment and connection into the driver's zeroed record.
/// Partial handles stay there for diagnostics and PDO's close callback.
#[unsafe(no_mangle)]
extern "C" fn swr_odbc_setup(
    dbh: &Dbh,
    options: Option<&swoole_rs_zend::Value>,
    env: &std::cell::Cell<Handle>,
    dbc: &std::cell::Cell<Connection>,
    file: Text<'static>,
    lines: &[i32; 6],
) -> bool {
    use crate::minit::{pdo_odbc_pool_mode, pdo_odbc_pool_on};
    use std::sync::atomic::Ordering;
    let fail = |what: &'static std::ffi::CStr, line| {
        pdo_odbc_error(dbh, None, Statement::NULL, what.into(), file, line);
        false
    };
    let mut allocated = Handle(std::ptr::null_mut());
    let rc = alloc_handle(1, allocated, &mut allocated);
    env.set(allocated);
    if !matches!(rc, 0 | 1) {
        return fail(c"SQLAllocHandle: ENV", lines[0]);
    }
    if !matches!(set_env_attr(env.get(), 200, 3, 0), 0 | 1) {
        return fail(c"SQLSetEnvAttr: ODBC3", lines[1]);
    }
    if pdo_odbc_pool_on.load(Ordering::Relaxed) != 0
        && set_env_attr(
            env.get(),
            202,
            pdo_odbc_pool_mode.load(Ordering::Relaxed),
            0,
        ) != 0
    {
        return fail(c"SQLSetEnvAttr: SQL_ATTR_CP_MATCH", lines[2]);
    }
    let mut allocated = Handle(std::ptr::null_mut());
    let rc = alloc_handle(2, env.get(), &mut allocated);
    dbc.set(Connection(allocated.0));
    if !matches!(rc, 0 | 1) {
        return fail(c"SQLAllocHandle: DBC", lines[3]);
    }
    if set_connect_attr(dbc.get(), 102, usize::from(dbh.auto_commit()), -6) != 0 {
        return fail(c"SQLSetConnectAttr AUTOCOMMIT", lines[4]);
    }
    let cursors = swoole_rs_zend::pdo::attr_long(options, 1000, 0).c_cast::<usize>();
    if set_connect_attr(dbc.get(), 110, cursors, -6) != 0 && cursors != 0 {
        return fail(c"SQLSetConnectAttr SQL_ODBC_CURSORS", lines[5]);
    }
    true
}
}

unsafe extern "C" {
    safe fn php_odbc_connstr_is_quoted(value: Text<'_>) -> bool;
    safe fn php_odbc_connstr_should_quote(value: Text<'_>) -> bool;
    safe fn php_odbc_connstr_estimate_quote_length(value: Text<'_>) -> usize;
    safe fn php_odbc_connstr_quote<'id>(
        out: swoole_rs_corebase::lent::Out<'id, u8>,
        value: Text<'_>,
        capacity: swoole_rs_corebase::lent::Count<'id>,
    ) -> usize;
    /// Copy the replacement using PDO's request or persistent allocator.
    safe fn swr_odbc_replace_source(dbh: &Dbh, source: Text<'_>);
}

fn append_credential(dsn: &mut Vec<u8>, key: &[u8], value: Text<'_>) {
    dsn.extend_from_slice(key);
    if php_odbc_connstr_is_quoted(value) || !php_odbc_connstr_should_quote(value) {
        dsn.extend_from_slice(value.get().to_bytes());
        return;
    }
    let mut quoted = vec![0; php_odbc_connstr_estimate_quote_length(value)];
    swoole_rs_corebase::lent::lend_out(&mut quoted, |out, count| {
        php_odbc_connstr_quote(out, value, count)
    });
    dsn.extend_from_slice(quoted.split(|&b| b == 0).next().unwrap_or_default());
}

fn with_credentials(
    source: &[u8],
    username: Option<Text<'_>>,
    password: Option<Text<'_>>,
) -> Option<std::ffi::CString> {
    let modern = swoole_rs_zend::sys::PHP_VERSION_ID >= 80400;
    let missing = |name: &[u8], upper: &[u8]| {
        !source.windows(name.len()).any(|key| {
            if modern {
                key.eq_ignore_ascii_case(name)
            } else {
                key == name || key == upper
            }
        })
    };
    let uid = username.filter(|v| (modern || !v.get().is_empty()) && missing(b"uid=", b"UID="));
    let pwd = password.filter(|v| (modern || !v.get().is_empty()) && missing(b"pwd=", b"PWD="));
    if if modern {
        uid.is_none() && pwd.is_none()
    } else {
        uid.is_none() || pwd.is_none()
    } {
        return None;
    }
    let base = if modern {
        source.strip_suffix(b";").unwrap_or(source)
    } else {
        source
    };
    // PHP formats the base with %s, even when it used the explicit length for matching.
    let mut bytes = base.split(|&b| b == 0).next().unwrap_or_default().to_vec();
    if let Some(uid) = uid {
        append_credential(&mut bytes, b";UID=", uid);
    }
    if let Some(pwd) = pwd {
        append_credential(&mut bytes, b";PWD=", pwd);
    }
    if modern {
        bytes.push(b';');
    }
    std::ffi::CString::new(bytes).ok()
}

swoole_rs_zend::exports! {
/// PDO lends its NUL-terminated source including the terminator. On 8.5 the
/// driver uses its explicit length for `SQLDriverConnect` and credential matching.
#[unsafe(no_mangle)]
extern "C" fn swr_odbc_connect<'a>(
    dbh: &Dbh,
    source: swoole_rs_corebase::lent::In<'a, u8>,
    size: swoole_rs_corebase::lent::Count<'a>,
    username: Option<Text<'_>>,
    password: Option<Text<'_>>,
    file: Text<'static>,
    line: i32,
) -> bool {
    let source = source.drain(size).bytes();
    let Ok(name) = std::ffi::CStr::from_bytes_until_nul(source) else {
        return false;
    };
    let sql_text = |v: Option<Text<'_>>| {
        crate::Text(v.map_or(std::ptr::null_mut(), |v| v.get().as_ptr().cast_mut().cast()))
    };
    let direct = name.to_bytes().contains(&b'=');
    let conn = swr_odbc_connection(dbh);
    let rc = if direct {
        let bytes = if swoole_rs_zend::sys::PHP_VERSION_ID >= 80500 {
            source.strip_suffix(&[0]).unwrap_or(source)
        } else {
            name.to_bytes()
        };
        let replacement = with_credentials(bytes, username, password);
        let (input, length) = if let Some(dsn) = replacement.as_deref() {
            if let Err(bailout) = swoole_rs_zend::contain_with(|| swr_odbc_replace_source(dbh, dsn.into())) {
                bailout.defer();
                return false;
            }
            (dsn.as_ptr().cast(), dsn.to_bytes().len())
        } else {
            (bytes.as_ptr(), bytes.len())
        };
        let mut out = [0u8; 1024];
        let mut used = 0;
        crate::swoole_odbc_SQLDriverConnect(
            conn,
            crate::Window(std::ptr::null_mut()),
            crate::Text(input.cast_mut()),
            length.c_cast::<i16>(),
            crate::Out(out.as_mut_ptr()),
            1023,
            crate::Out(&raw mut used),
            0,
        )
    } else {
        crate::swoole_odbc_SQLConnect(
            conn,
            sql_text(Some(name.into())),
            -3,
            sql_text(username),
            -3,
            sql_text(password),
            -3,
        )
    };
    if matches!(rc, 0 | 1) {
        return true;
    }
    let what = if direct {
        c"SQLDriverConnect"
    } else {
        c"SQLConnect"
    };
    pdo_odbc_error(dbh, None, Statement::NULL, what.into(), file, line);
    false
}
}
