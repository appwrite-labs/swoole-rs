//! The connection's methods (`oci_driver.c`): transactions, the liveness
//! check, `getAttribute`, `setAttribute`, `exec`, `quote` and closing. The
//! connection's record is [`crate::handle::Handle`], read through its accessors.
use crate::{
    Any, Env, Error, Live, Session, Snapshot, Stmt, Svc, Sword, Text,
    error::{OCI_HTYPE_ERROR, OCIErrorGet, here, report},
    handle::{
        release, swr_oci_attached, swr_oci_charset, swr_oci_env, swr_oci_err, swr_oci_prefetch,
        swr_oci_server, swr_oci_session, swr_oci_set_prefetch, swr_oci_svc,
    },
    swoole_oci_ping, swoole_oci_server_detach, swoole_oci_stmt_execute, swoole_oci_stmt_prepare,
    swoole_oci_trans_commit, swoole_oci_trans_rollback,
};
use std::{ffi::CStr, marker::PhantomData, ptr};
use swoole_rs_corebase::CCast;
use swoole_rs_zend::{
    E_WARNING, Owned, Value, ZStr, ZString,
    pdo::{self, Attribute, Dbh, Slot},
    sys::{
        pdo_attribute_type_PDO_ATTR_AUTOCOMMIT as AUTOCOMMIT,
        pdo_attribute_type_PDO_ATTR_CLIENT_VERSION as CLIENT_VERSION,
        pdo_attribute_type_PDO_ATTR_DRIVER_SPECIFIC as DRIVER_SPECIFIC,
        pdo_attribute_type_PDO_ATTR_PREFETCH as PREFETCH,
        pdo_attribute_type_PDO_ATTR_SERVER_INFO as SERVER_INFO,
        pdo_attribute_type_PDO_ATTR_SERVER_VERSION as SERVER_VERSION, zend_long, zend_string,
    },
};

/// `OCI_SUCCESS`.
const OCI_SUCCESS: Sword = 0;
/// `OCI_HTYPE_SVCCTX`.
pub(crate) const OCI_HTYPE_SVCCTX: u32 = 3;
/// `OCI_HTYPE_SVCCTX` as `OCIServerRelease`'s `ub1` takes it.
const SVCCTX_UB1: u8 = 3;
/// `OCI_ATTR_CALL_TIMEOUT`.
const OCI_ATTR_CALL_TIMEOUT: u32 = 531;
/// `OCI_ATTR_MODULE`, `OCI_ATTR_ACTION`, `OCI_ATTR_CLIENT_INFO` and
/// `OCI_ATTR_CLIENT_IDENTIFIER`.
const OCI_ATTR_MODULE: u32 = 366;
const OCI_ATTR_ACTION: u32 = 367;
const OCI_ATTR_CLIENT_INFO: u32 = 368;
const OCI_ATTR_CLIENT_IDENTIFIER: u32 = 278;
/// `OCI_HTYPE_ENV`, `OCI_HTYPE_SERVER`.
const OCI_HTYPE_ENV: u32 = 1;
pub(crate) const OCI_HTYPE_SERVER: u32 = 8;
/// `OCI_HTYPE_STMT`.
const OCI_HTYPE_STMT: u32 = 4;
/// `OCI_ATTR_ROW_COUNT` and `OCI_ATTR_STMT_TYPE`.
const OCI_ATTR_ROW_COUNT: u32 = 9;
const OCI_ATTR_STMT_TYPE: u32 = 24;
/// `OCI_STMT_SELECT`.
const OCI_STMT_SELECT: u16 = 1;
/// `OCI_NTV_SYNTAX`: the server's own SQL dialect.
const OCI_NTV_SYNTAX: u32 = 1;
/// `OCI_COMMIT_ON_SUCCESS`.
const OCI_COMMIT_ON_SUCCESS: u32 = 0x20;
/// `OCI_SUCCESS_WITH_INFO`.
const OCI_SUCCESS_WITH_INFO: Sword = 1;
/// `OCI_HTYPE_SESSION`.
pub(crate) const OCI_HTYPE_SESSION: u32 = 9;
/// `php_pdo_oci_int.h`'s attributes, from `PDO_OCI_ATTR_ACTION`.
const ACTION: u32 = DRIVER_SPECIFIC;
const CLIENT_INFO: u32 = DRIVER_SPECIFIC + 1;
const CLIENT_IDENTIFIER: u32 = DRIVER_SPECIFIC + 2;
const MODULE: u32 = DRIVER_SPECIFIC + 3;
const CALL_TIMEOUT: u32 = DRIVER_SPECIFIC + 4;
/// `PDO_OCI_PREFETCH_DEFAULT`, rows.
pub(crate) const PREFETCH_DEFAULT: u32 = 100;
/// `PDO_OCI_PREFETCH_ROWSIZE`: the row length a prefetch's memory limit assumes.
const PREFETCH_ROWSIZE: u32 = 1024;
/// `char verstr[15]`: a version string is cut to 14 characters.
const VERSION_LIMIT: usize = 14;
/// ORA-01010, invalid OCI operation: a server too old for `OCIPing` answered.
const INVALID_OPERATION: i32 = 1010;

unsafe extern "C" {
    /// OCI's own declaration, with the user memory it can also hand out typed
    /// as the null it is here: a new handle of `kind` under `parent`, written
    /// to `handle` (null when it fails).
    pub(crate) safe fn OCIHandleAlloc(
        parent: Live<Env>,
        handle: &mut crate::Handle,
        kind: u32,
        extra: usize,
        user_memory: Option<&'static crate::error::Nothing>,
    ) -> Sword;
    /// OCI's own declaration for a handle this module frees, of `kind`.
    safe fn OCIHandleFree(handle: Live<Any>, kind: u32) -> Sword;
    /// OCI's own declaration for a statement's 2- or 4-byte attribute, written
    /// in native order to the start of `value` (a `u32`, aligned for either),
    /// with the size typed as the null it is here.
    #[link_name = "OCIAttrGet"]
    pub(crate) safe fn OCIAttrGetStmt(
        stmt: Live<Stmt>,
        kind: u32,
        value: &mut u32,
        size: Option<&'static crate::error::Nothing>,
        attribute: u32,
        err: Live<Error>,
    ) -> Sword;
    /// OCI's own declaration for a text attribute of a session: OCI copies
    /// `size` bytes of `value`.
    #[link_name = "OCIAttrSet"]
    pub(crate) safe fn OCIAttrSetSession(
        session: Live<Session>,
        kind: u32,
        value: Text,
        size: u32,
        attribute: u32,
        err: Live<Error>,
    ) -> Sword;
    /// OCI's own declaration for a 4-byte attribute of a service context, which
    /// OCI copies.
    #[expect(
        clashing_extern_declarations,
        reason = "OCIAttrSet takes its handle and value as void *: each declaration types them as one use passes them"
    )]
    #[link_name = "OCIAttrSet"]
    safe fn OCIAttrSetSvc(
        svc: Live<Svc>,
        kind: u32,
        value: &u32,
        size: u32,
        attribute: u32,
        err: Live<Error>,
    ) -> Sword;
    /// OCI's own declaration for a service context attribute that is another
    /// handle (`OCI_ATTR_SERVER`, `OCI_ATTR_SESSION`), which OCI keeps.
    #[expect(
        clashing_extern_declarations,
        reason = "OCIAttrSet takes its handle and value as void *: each declaration types them as one use passes them"
    )]
    #[link_name = "OCIAttrSet"]
    pub(crate) safe fn OCIAttrSetHandle(
        svc: Live<Svc>,
        kind: u32,
        value: Live<Any>,
        size: u32,
        attribute: u32,
        err: Live<Error>,
    ) -> Sword;
    /// OCI's own declaration for a service context: the server's banner (cut to
    /// `size`, NUL included) and its packed version.
    safe fn OCIServerRelease(
        svc: Live<Svc>,
        err: Live<Error>,
        banner: &mut [u8; 512],
        size: u32,
        kind: u8,
        version: &mut u32,
    ) -> Sword;
    /// OCI's own declaration: the client library's version.
    safe fn OCIClientVersion(
        major: &mut Sword,
        minor: &mut Sword,
        update: &mut Sword,
        patch: &mut Sword,
        port_update: &mut Sword,
    );
    /// OCI's own declaration for a 4-byte attribute of a service context, with
    /// the size it can also return typed as the null it is here.
    #[link_name = "OCIAttrGet"]
    safe fn OCIAttrGetSvc(
        svc: Live<Svc>,
        kind: u32,
        value: &mut u32,
        size: Option<&'static crate::error::Nothing>,
        attribute: u32,
        err: Live<Error>,
    ) -> Sword;
}

/// `OCITransCommit` or `OCITransRollback` (`what`), reporting a failure.
fn finish(
    dbh: &Dbh,
    what: &CStr,
    call: extern "C" fn(Live<Svc>, Live<Error>, u32) -> Sword,
) -> bool {
    let err = swr_oci_err(dbh);
    let status = call(swr_oci_svc(dbh), err, 0);
    if status != OCI_SUCCESS {
        report(dbh, err, what, status, here!());
        return false;
    }
    true
}

swoole_rs_zend::exports! {
/// `oci_handle_begin`: with Oracle, a transaction needs no start.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_begin(_dbh: &Dbh) -> bool {
    true
}

/// `oci_handle_commit`.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_commit(dbh: &Dbh) -> bool {
    finish(dbh, c"OCITransCommit", swoole_oci_trans_commit)
}

/// `oci_handle_rollback`.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_rollback(dbh: &Dbh) -> bool {
    finish(dbh, c"OCITransRollback", swoole_oci_trans_rollback)
}

/// `pdo_oci_check_liveness`: whether an attached connection's server still
/// answers `OCIPing`. An ORA-01010 is a server too old for it, which answered.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_alive(dbh: &Dbh) -> bool {
    if !swr_oci_attached(dbh) {
        return false;
    }
    let err = swr_oci_err(dbh);
    if swoole_oci_ping(swr_oci_svc(dbh), err, 0) == OCI_SUCCESS {
        return true;
    }
    let mut code = 0;
    // A failed lookup leaves the code 0, which is not ORA-01010, as in C.
    let _status = OCIErrorGet(err, 1, None, &mut code, None, 0, OCI_HTYPE_ERROR);
    code == INVALID_OPERATION
}

/// `oci_handle_get_attribute`.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_get_attribute(dbh: &Dbh, attr: zend_long, result: &Slot) -> Attribute {
    let Ok(attr) = u32::try_from(attr) else {
        return Attribute::Unknown;
    };
    let value = match attr {
        SERVER_VERSION | SERVER_INFO => {
            let mut banner = [0u8; 512];
            let mut version = 0u32;
            let status = OCIServerRelease(
                swr_oci_svc(dbh),
                swr_oci_err(dbh),
                &mut banner,
                512,
                SVCCTX_UB1,
                &mut version,
            );
            let text = if status != OCI_SUCCESS {
                b"<<Unknown>>".to_vec()
            } else if attr == SERVER_INFO {
                CStr::from_bytes_until_nul(&banner).map_or(&banner[..], CStr::to_bytes).to_vec()
            } else {
                let mut text = format!(
                    "{}.{}.{}.{}.{}",
                    (version >> 24) & 0xFF,
                    (version >> 20) & 0x0F,
                    (version >> 12) & 0xFF,
                    (version >> 8) & 0x0F,
                    version & 0xFF
                )
                .into_bytes();
                text.truncate(VERSION_LIMIT);
                text
            };
            Owned::string(ZString::new(&text))
        }
        CLIENT_VERSION => {
            let (mut major, mut minor, mut update, mut patch, mut port) = (0, 0, 0, 0, 0);
            OCIClientVersion(&mut major, &mut minor, &mut update, &mut patch, &mut port);
            let mut text = format!("{major}.{minor}.{update}.{patch}.{port}").into_bytes();
            text.truncate(VERSION_LIMIT);
            Owned::string(ZString::new(&text))
        }
        AUTOCOMMIT => Owned::bool(dbh.auto_commit()),
        PREFETCH => Owned::long(swr_oci_prefetch(dbh).into()),
        CALL_TIMEOUT => {
            let err = swr_oci_err(dbh);
            if !call_timeout_supported() {
                report(dbh, err, c"Unsupported attribute type", OCI_SUCCESS, here!());
                return Attribute::Unknown;
            }
            let mut timeout = 0u32;
            let status = OCIAttrGetSvc(
                swr_oci_svc(dbh),
                OCI_HTYPE_SVCCTX,
                &mut timeout,
                None,
                OCI_ATTR_CALL_TIMEOUT,
                err,
            );
            if status != OCI_SUCCESS {
                report(dbh, err, c"OCIAttrGet: OCI_ATTR_CALL_TIMEOUT", status, here!());
                return Attribute::Unknown;
            }
            Owned::long(timeout.into())
        }
        _ => return Attribute::Unknown,
    };
    result.set(value);
    Attribute::Set
}

/// `oci_handle_set_attribute`.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_set_attribute(dbh: &Dbh, attr: zend_long, value: &Value) -> bool {
    let Ok(attr) = u32::try_from(attr) else {
        return false;
    };
    match attr {
        AUTOCOMMIT => {
            let Some(on) = pdo::bool_param(value) else {
                return false;
            };
            if dbh.in_txn() {
                // Assume they want to commit whatever is outstanding.
                if !finish(dbh, c"OCITransCommit", swoole_oci_trans_commit) {
                    return false;
                }
                dbh.set_in_txn(false);
            }
            dbh.set_auto_commit(on);
            true
        }
        PREFETCH => {
            let Some(rows) = pdo::long_param(value) else {
                return false;
            };
            swr_oci_set_prefetch(dbh, sanitize_prefetch(rows));
            true
        }
        ACTION => set_text(dbh, value, OCI_ATTR_ACTION, c"OCIAttrSet: OCI_ATTR_ACTION"),
        CLIENT_INFO => set_text(dbh, value, OCI_ATTR_CLIENT_INFO, c"OCIAttrSet: OCI_ATTR_CLIENT_INFO"),
        CLIENT_IDENTIFIER => set_text(
            dbh,
            value,
            OCI_ATTR_CLIENT_IDENTIFIER,
            c"OCIAttrSet: OCI_ATTR_CLIENT_IDENTIFIER",
        ),
        MODULE => set_text(dbh, value, OCI_ATTR_MODULE, c"OCIAttrSet: OCI_ATTR_MODULE"),
        CALL_TIMEOUT => {
            if !call_timeout_supported() {
                let err = swr_oci_err(dbh);
                report(dbh, err, c"Unsupported attribute type", OCI_SUCCESS, here!());
                return false;
            }
            let Some(timeout) = pdo::long_param(value) else {
                return false;
            };
            // `(ub4) lval`, as C truncates it.
            let timeout: u32 = timeout.c_cast();
            let err = swr_oci_err(dbh);
            let status = OCIAttrSetSvc(
                swr_oci_svc(dbh),
                OCI_HTYPE_SVCCTX,
                &timeout,
                0,
                OCI_ATTR_CALL_TIMEOUT,
                err,
            );
            if status != OCI_SUCCESS {
                report(dbh, err, c"OCIAttrSet: OCI_ATTR_CALL_TIMEOUT", status, here!());
                return false;
            }
            true
        }
        _ => false,
    }
}
}

/// A session's text attribute (`OCI_ATTR_ACTION` and the like) set to the value
/// as a string, which fails with no OCI call when the conversion threw.
fn set_text(dbh: &Dbh, value: &Value, attribute: u32, what: &CStr) -> bool {
    let Some(text) = value.try_string() else {
        return false;
    };
    let bytes = text.bytes();
    let err = swr_oci_err(dbh);
    let status = OCIAttrSetSession(
        swr_oci_session(dbh),
        OCI_HTYPE_SESSION,
        Text::of(bytes),
        bytes.len().c_cast(),
        attribute,
        err,
    );
    if status != OCI_SUCCESS {
        report(dbh, err, what, status, here!());
        return false;
    }
    true
}

/// `pdo_oci_sanitize_prefetch`: a row count, 0 for a negative one and the
/// default for one whose memory limit would overflow `ub4`.
fn sanitize_prefetch(rows: zend_long) -> u32 {
    if rows < 0 {
        0
    } else {
        u32::try_from(rows)
            .ok()
            .filter(|&rows| rows <= u32::MAX / PREFETCH_ROWSIZE)
            .unwrap_or(PREFETCH_DEFAULT)
    }
}

/// `OCI_MAJOR_VERSION >= 18`, which `OCI_ATTR_CALL_TIMEOUT` needs. C checked the
/// headers it was built with; this asks the client library it loaded, which
/// ships with them. Older clients report "Unsupported attribute type" instead.
/// C recorded that with whatever status the handle's last call left (0 after a
/// successful one), which Rust doesn't keep; this records `OCI_SUCCESS`.
fn call_timeout_supported() -> bool {
    let (mut major, mut minor, mut update, mut patch, mut port) = (0, 0, 0, 0, 0);
    OCIClientVersion(&mut major, &mut minor, &mut update, &mut patch, &mut port);
    major >= 18
}

swoole_rs_zend::exports! {
/// `oci_handle_doer`: `sql` prepared and run on a statement handle of its own,
/// returning the rows it changed, or -1. A `SELECT` is refused with a warning.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_exec(dbh: &Dbh, sql: &ZStr) -> zend_long {
    let err = swr_oci_err(dbh);
    let stmt = Statement::alloc(swr_oci_env(dbh));
    let bytes = sql.bytes();
    let status = swoole_oci_stmt_prepare(
        stmt.0,
        err,
        Text::of(bytes),
        bytes.len().c_cast(),
        OCI_NTV_SYNTAX,
        0,
    );
    if status != OCI_SUCCESS {
        report(dbh, err, c"OCIStmtPrepare", status, here!());
        return -1;
    }
    let mut kind = 0u32;
    // C ignores this status too: a failed read leaves the type 0, no SELECT.
    let _status = OCIAttrGetStmt(stmt.0, OCI_HTYPE_STMT, &mut kind, None, OCI_ATTR_STMT_TYPE, err);
    let kind = kind.to_ne_bytes();
    if u16::from_ne_bytes([kind[0], kind[1]]) == OCI_STMT_SELECT {
        drop(stmt);
        swoole_rs_zend::bailout::defer(swoole_rs_zend::report(
            E_WARNING,
            c"issuing a SELECT query here is invalid",
        ));
        return -1;
    }
    let mode = if dbh.auto_commit() && !dbh.in_txn() {
        OCI_COMMIT_ON_SUCCESS
    } else {
        0
    };
    let none = Live::<Snapshot>(ptr::null_mut(), PhantomData);
    let status = swoole_oci_stmt_execute(swr_oci_svc(dbh), stmt.0, err, 1, 0, none, none, mode);
    if status != OCI_SUCCESS {
        report(dbh, err, c"OCIStmtExecute", status, here!());
    }
    if status != OCI_SUCCESS && status != OCI_SUCCESS_WITH_INFO {
        return -1;
    }
    let mut rows = 0u32;
    // C returns whatever this left in the count, as here (0 when it fails).
    let _status = OCIAttrGetStmt(stmt.0, OCI_HTYPE_STMT, &mut rows, None, OCI_ATTR_ROW_COUNT, err);
    rows.into()
}

/// `oci_handle_quoter`: `unquoted` in single quotes, each quote doubled.
/// C scanned with `strchr` and copied with `strncpy`, so the bytes after a NUL
/// come out as NULs, and only the quotes before it are doubled.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_quote(_dbh: &Dbh, unquoted: &ZStr, _kind: u32) -> *mut zend_string {
    ZString::new(&quote(unquoted.bytes())).into_raw()
}

/// `oci_handle_closer` but for freeing the handle itself, which C does after
/// this: any outstanding work rolled back, then the session, service context,
/// server (detached first while attached), error handle and, when the handle
/// made its own for a charset, environment freed. A factory that failed part
/// way leaves some of them null, which are skipped.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_close(dbh: &Dbh) {
    let (svc, err, session, server) =
        (swr_oci_svc(dbh), swr_oci_err(dbh), swr_oci_session(dbh), swr_oci_server(dbh));
    let free = |handle: Live<Any>, kind| {
        if !handle.is_null() {
            // OCI fails to free only a null or foreign handle, which these aren't.
            let _status = OCIHandleFree(handle, kind);
        }
    };
    if !svc.is_null() {
        // Rolling back outstanding work is best effort, as in C.
        let _status = swoole_oci_trans_rollback(svc, err, 0);
    }
    free(session.any(), OCI_HTYPE_SESSION);
    free(svc.any(), OCI_HTYPE_SVCCTX);
    if !server.is_null() && swr_oci_attached(dbh) {
        let status = swoole_oci_server_detach(server, err, 0);
        if status != OCI_SUCCESS {
            report(dbh, err, c"OCIServerDetach", status, here!());
        }
    }
    free(server.any(), OCI_HTYPE_SERVER);
    free(err.any(), OCI_HTYPE_ERROR);
    if swr_oci_charset(dbh) != 0 {
        free(swr_oci_env(dbh).any(), OCI_HTYPE_ENV);
    }
    release(dbh);
}
}

/// [`swr_oci_quote`]'s bytes.
fn quote(unquoted: &[u8]) -> Vec<u8> {
    if unquoted.is_empty() {
        return b"''".to_vec();
    }
    let end = unquoted
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(unquoted.len());
    let (text, rest) = unquoted.split_at(end);
    let mut quoted = Vec::with_capacity(unquoted.len() + 2);
    quoted.push(b'\'');
    for &byte in text {
        quoted.push(byte);
        if byte == b'\'' {
            quoted.push(b'\'');
        }
    }
    quoted.resize(quoted.len() + rest.len(), 0);
    quoted.push(b'\'');
    quoted
}

/// A statement handle [`swr_oci_exec`] allocated, freed when dropped.
struct Statement(Live<Stmt>);

impl Statement {
    /// `OCIHandleAlloc(env, OCI_HTYPE_STMT)`: as in C, a failure leaves a null
    /// handle, which every OCI call rejects.
    fn alloc(env: Live<Env>) -> Self {
        let mut handle = ptr::null_mut();
        // A failure leaves the handle null, which the calls on it report.
        let _status = OCIHandleAlloc(env, &mut handle, OCI_HTYPE_STMT, 0, None);
        Self(Live(handle, PhantomData))
    }
}

impl Drop for Statement {
    fn drop(&mut self) {
        // Freeing a handle fails only for a null one, which needs nothing.
        let _status = OCIHandleFree(self.0.any(), OCI_HTYPE_STMT);
    }
}
