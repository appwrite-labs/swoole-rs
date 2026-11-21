//! The connection's methods (`oci_driver.c`): transactions and the liveness
//! check. The handle is still C's `pdo_oci_db_handle`, read through accessors.
use crate::{
    Error, Live, Svc, Sword,
    error::{OCI_HTYPE_ERROR, OCIErrorGet, here, report},
    swoole_oci_ping, swoole_oci_trans_commit, swoole_oci_trans_rollback,
};
use std::ffi::CStr;
use swoole_rs_zend::pdo::Dbh;

/// `OCI_SUCCESS`.
const OCI_SUCCESS: Sword = 0;
/// ORA-01010, invalid OCI operation: a server too old for `OCIPing` answered.
const INVALID_OPERATION: i32 = 1010;

unsafe extern "C" {
    /// The handle's service context (`H->svc`), live while the handle is.
    safe fn swr_oci_svc(dbh: &Dbh) -> Live<Svc>;
    /// The handle's error handle (`H->err`), live while the handle is.
    safe fn swr_oci_err(dbh: &Dbh) -> Live<Error>;
    /// `H->attached`.
    safe fn swr_oci_attached(dbh: &Dbh) -> bool;
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
}
