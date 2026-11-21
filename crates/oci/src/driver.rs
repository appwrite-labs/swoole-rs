//! The connection's methods (`oci_driver.c`): transactions, the liveness
//! check, `getAttribute` and `setAttribute`. The handle is still C's `pdo_oci_db_handle`, read through accessors.
use crate::{
    Error, Live, Session, Svc, Sword, Text,
    error::{OCI_HTYPE_ERROR, OCIErrorGet, here, report},
    swoole_oci_ping, swoole_oci_trans_commit, swoole_oci_trans_rollback,
};
use std::ffi::CStr;
use swoole_rs_corebase::CCast;
use swoole_rs_zend::{
    Owned, Value, ZString,
    pdo::{self, Attribute, Dbh, Slot},
    sys::{
        pdo_attribute_type_PDO_ATTR_AUTOCOMMIT as AUTOCOMMIT,
        pdo_attribute_type_PDO_ATTR_CLIENT_VERSION as CLIENT_VERSION,
        pdo_attribute_type_PDO_ATTR_DRIVER_SPECIFIC as DRIVER_SPECIFIC,
        pdo_attribute_type_PDO_ATTR_PREFETCH as PREFETCH,
        pdo_attribute_type_PDO_ATTR_SERVER_INFO as SERVER_INFO,
        pdo_attribute_type_PDO_ATTR_SERVER_VERSION as SERVER_VERSION, zend_long,
    },
};

/// `OCI_SUCCESS`.
const OCI_SUCCESS: Sword = 0;
/// `OCI_HTYPE_SVCCTX`.
const OCI_HTYPE_SVCCTX: u32 = 3;
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
/// `OCI_HTYPE_SESSION`.
const OCI_HTYPE_SESSION: u32 = 9;
/// `php_pdo_oci_int.h`'s attributes, from `PDO_OCI_ATTR_ACTION`.
const ACTION: u32 = DRIVER_SPECIFIC;
const CLIENT_INFO: u32 = DRIVER_SPECIFIC + 1;
const CLIENT_IDENTIFIER: u32 = DRIVER_SPECIFIC + 2;
const MODULE: u32 = DRIVER_SPECIFIC + 3;
const CALL_TIMEOUT: u32 = DRIVER_SPECIFIC + 4;
/// `PDO_OCI_PREFETCH_DEFAULT`, rows.
const PREFETCH_DEFAULT: u32 = 100;
/// `PDO_OCI_PREFETCH_ROWSIZE`: the row length a prefetch's memory limit assumes.
const PREFETCH_ROWSIZE: u32 = 1024;
/// `char verstr[15]`: a version string is cut to 14 characters.
const VERSION_LIMIT: usize = 14;
/// ORA-01010, invalid OCI operation: a server too old for `OCIPing` answered.
const INVALID_OPERATION: i32 = 1010;

unsafe extern "C" {
    /// The handle's service context (`H->svc`), live while the handle is.
    safe fn swr_oci_svc(dbh: &Dbh) -> Live<Svc>;
    /// The handle's error handle (`H->err`), live while the handle is.
    safe fn swr_oci_err(dbh: &Dbh) -> Live<Error>;
    /// `H->attached`.
    safe fn swr_oci_attached(dbh: &Dbh) -> bool;
    /// `H->prefetch`.
    safe fn swr_oci_prefetch(dbh: &Dbh) -> u32;
    /// `H->prefetch = rows`.
    safe fn swr_oci_set_prefetch(dbh: &Dbh, rows: u32);
    /// The handle's session (`H->session`), live while the handle is.
    safe fn swr_oci_session(dbh: &Dbh) -> Live<Session>;
    /// OCI's own declaration for a text attribute of a session: OCI copies
    /// `size` bytes of `value`.
    #[link_name = "OCIAttrSet"]
    safe fn OCIAttrSetSession(
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
