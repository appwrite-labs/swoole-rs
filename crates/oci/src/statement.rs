//! The statement's methods (`oci_statement.c`): so far `getColumnMeta`,
//! fetching a row, executing, and closing the statement's handles. The
//! statement is still C's `pdo_oci_stmt`, read through accessors.
use crate::{
    Any, Error, Live, Stmt, Sword,
    driver::OCIAttrGetStmt,
    error::{Nothing, here, stmt_call},
    swoole_oci_stmt_execute, swoole_oci_stmt_fetch, swoole_oci_stmt_fetch2,
};
use std::{ffi::CStr, marker::PhantomData, ptr};
use swoole_rs_corebase::CCast;
use swoole_rs_zend::{
    OwnedArray,
    pdo::{Dbh, Slot, Stmt as PdoStmt},
    sys::{
        pdo_param_type_PDO_PARAM_LOB as PARAM_LOB, pdo_param_type_PDO_PARAM_STR as PARAM_STR,
        zend_long,
    },
};

/// A column's parameter descriptor (`OCIParam`).
pub(crate) enum Param {}

/// `OCI_SUCCESS`, `OCI_SUCCESS_WITH_INFO`, `OCI_NEED_DATA` and `OCI_NO_DATA`.
const OCI_SUCCESS: Sword = 0;
const OCI_SUCCESS_WITH_INFO: Sword = 1;
const OCI_NEED_DATA: Sword = 99;
const OCI_NO_DATA: Sword = 100;
/// Where C reported a fetch's errors: `bug44301.phpt` matches the file, so they
/// keep upstream's `oci_statement.c` lines, as the ODBC port kept its own.
const UPSTREAM_STATEMENT: &CStr = c"thirdparty/pdo_oci/oci_statement.c";
/// `OCI_ATTR_STMT_TYPE`, `OCI_ATTR_PARAM_COUNT` and `OCI_ATTR_ROW_COUNT`.
const OCI_ATTR_STMT_TYPE: u32 = 24;
const OCI_ATTR_PARAM_COUNT: u32 = 18;
const OCI_ATTR_ROW_COUNT: u32 = 9;
/// `OCI_STMT_SELECT`.
const OCI_STMT_SELECT: u16 = 1;
/// `OCI_STMT_SCROLLABLE_READONLY` and `OCI_COMMIT_ON_SUCCESS`, `OCIStmtExecute`'s modes.
const OCI_STMT_SCROLLABLE_READONLY: u32 = 0x08;
const OCI_COMMIT_ON_SUCCESS: u32 = 0x20;
/// `OCI_FETCH_NEXT` and `OCI_HTYPE_ERROR`.
const OCI_FETCH_NEXT: u16 = 0x02;
const OCI_HTYPE_ERROR: u32 = 2;
/// `OCI_HTYPE_STMT` and `OCI_DTYPE_PARAM`.
const OCI_HTYPE_STMT: u32 = 4;
const OCI_DTYPE_PARAM: u32 = 53;
/// The parameter attributes read here.
const OCI_ATTR_DATA_TYPE: u32 = 2;
const OCI_ATTR_PRECISION: u32 = 5;
const OCI_ATTR_SCALE: u32 = 6;
const OCI_ATTR_IS_NULL: u32 = 7;
const OCI_ATTR_CHARSET_FORM: u32 = 32;
/// `SQLCS_NCHAR`: a national character set column.
const SQLCS_NCHAR: u8 = 2;
/// The `SQLT_*` data types `getColumnMeta` names.
const SQLT_CHR: u16 = 1;
const SQLT_NUM: u16 = 2;
const SQLT_FLT: u16 = 4;
const SQLT_LNG: u16 = 8;
const SQLT_VCS: u16 = 9;
const SQLT_DAT: u16 = 12;
const SQLT_BFLOAT: u16 = 21;
const SQLT_BDOUBLE: u16 = 22;
const SQLT_BIN: u16 = 23;
const SQLT_LBI: u16 = 24;
const SQLT_AFC: u16 = 96;
const SQLT_IBFLOAT: u16 = 100;
const SQLT_IBDOUBLE: u16 = 101;
const SQLT_RDD: u16 = 104;
const SQLT_CLOB: u16 = 112;
const SQLT_BLOB: u16 = 113;
const SQLT_BFILE: u16 = 114;
const SQLT_TIMESTAMP: u16 = 187;
const SQLT_TIMESTAMP_TZ: u16 = 188;
const SQLT_INTERVAL_YM: u16 = 189;
const SQLT_INTERVAL_DS: u16 = 190;
const SQLT_TIMESTAMP_LTZ: u16 = 232;

unsafe extern "C" {
    /// Discard PDO's cached column descriptions before replacing the OCI defines.
    safe fn php_pdo_stmt_set_column_count(stmt: &PdoStmt, columns: i32);
    /// The statement's OCI handle (`S->stmt`), live while the statement is.
    pub(crate) safe fn swr_oci_stmt_handle(stmt: &PdoStmt) -> Live<Stmt>;
    /// `S->stmt_type`: the statement's `OCI_ATTR_STMT_TYPE`, 0 until read.
    safe fn swr_oci_stmt_type(stmt: &PdoStmt) -> u16;
    /// `S->stmt_type = kind`.
    safe fn swr_oci_set_stmt_type(stmt: &PdoStmt, kind: u16);
    /// Whether the statement runs as a scrollable cursor (`S->exec_type`).
    safe fn swr_oci_stmt_scrollable(stmt: &PdoStmt) -> bool;
    /// `S->have_blobs`: a LOB parameter is bound.
    safe fn swr_oci_stmt_have_blobs(stmt: &PdoStmt) -> bool;
    /// OCI's own declaration for the handles this frees, of `kind`.
    #[link_name = "OCIHandleFree"]
    safe fn OCIHandleFreeStmt(handle: Live<Any>, kind: u32) -> Sword;
    /// The statement's own error handle (`S->err`), live while the statement is.
    pub(crate) safe fn swr_oci_stmt_err(stmt: &PdoStmt) -> Live<Error>;
    /// OCI's own declaration: column `position`'s parameter descriptor, written
    /// to `param` (null when it fails).
    pub(crate) safe fn OCIParamGet(
        stmt: Live<Stmt>,
        kind: u32,
        err: Live<Error>,
        param: &mut crate::Handle,
        position: u32,
    ) -> Sword;
    /// OCI's own declaration for a parameter's 1-, 2- or 4-byte attribute,
    /// written in native order to the start of `value` (a `u32`, aligned for
    /// each), with the size typed as the null it is here.
    #[link_name = "OCIAttrGet"]
    pub(crate) safe fn OCIAttrGetParam(
        param: Live<Param>,
        kind: u32,
        value: &mut u32,
        size: Option<&'static Nothing>,
        attribute: u32,
        err: Live<Error>,
    ) -> Sword;
    /// OCI's own declaration for the descriptor this frees.
    safe fn OCIDescriptorFree(descriptor: Live<Any>, kind: u32) -> Sword;
}

/// A column's parameter descriptor, freed when dropped.
pub(crate) struct Descriptor(pub(crate) Live<Param>);

impl Drop for Descriptor {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // Freeing fails only for a null or foreign descriptor.
            let _status = OCIDescriptorFree(self.0.any(), OCI_DTYPE_PARAM);
        }
    }
}

/// `decl_type` and `native_type`'s shared name for a declared type, or None for
/// one this doesn't know, which C reports by number.
fn type_name(dtype: u16, precision: u16, scale: i8, nchar: bool) -> Option<&'static [u8]> {
    Some(match dtype {
        SQLT_TIMESTAMP => b"TIMESTAMP",
        SQLT_TIMESTAMP_TZ => b"TIMESTAMP WITH TIMEZONE",
        SQLT_TIMESTAMP_LTZ => b"TIMESTAMP WITH LOCAL TIMEZONE",
        SQLT_INTERVAL_YM => b"INTERVAL YEAR TO MONTH",
        SQLT_INTERVAL_DS => b"INTERVAL DAY TO SECOND",
        SQLT_DAT => b"DATE",
        // A nonzero precision with scale -127 is a FLOAT.
        SQLT_FLT | SQLT_NUM if scale == -127 && precision != 0 => b"FLOAT",
        SQLT_FLT | SQLT_NUM => b"NUMBER",
        SQLT_LNG => b"LONG",
        SQLT_BIN => b"RAW",
        SQLT_LBI => b"LONG RAW",
        SQLT_CHR | SQLT_VCS if nchar => b"NVARCHAR2",
        SQLT_CHR | SQLT_VCS => b"VARCHAR2",
        SQLT_AFC if nchar => b"NCHAR",
        SQLT_AFC => b"CHAR",
        SQLT_BLOB => b"BLOB",
        SQLT_CLOB if nchar => b"NCLOB",
        SQLT_CLOB => b"CLOB",
        SQLT_BFILE => b"BFILE",
        SQLT_RDD => b"ROWID",
        SQLT_BFLOAT | SQLT_IBFLOAT => b"BINARY_FLOAT",
        SQLT_BDOUBLE | SQLT_IBDOUBLE => b"BINARY_DOUBLE",
        _ => return None,
    })
}

swoole_rs_zend::exports! {
/// `oci_stmt_col_meta` once C checked the statement and column: column
/// `colno`'s declared and native type, PDO type, scale and flags. As in C, an
/// OCI call that records an Oracle error leaves the keys added so far, with
/// no `flags`, and still reports success.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_column_meta(stmt: &PdoStmt, dbh: &Dbh, colno: u32, output: &Slot) {
    let mut meta = OwnedArray::default();
    let mut flags = OwnedArray::default();
    let (handle, err) = (swr_oci_stmt_handle(stmt), swr_oci_stmt_err(stmt));
    let call = |what: &CStr, status| stmt_call(dbh, stmt, err, what, status, here!()) != 0;
    let mut raw = ptr::null_mut();
    // C passed `colno + 1` as a `ub4`, which wraps (PDO rejects a negative index first).
    let status = OCIParamGet(handle, OCI_HTYPE_STMT, err, &mut raw, colno.wrapping_add(1));
    let param = Descriptor(Live(raw, PhantomData));
    let get = |attribute| {
        let mut value = 0u32;
        let status = OCIAttrGetParam(param.0, OCI_DTYPE_PARAM, &mut value, None, attribute, err);
        (status, value.to_ne_bytes())
    };
    let done = 'read: {
        if call(c"OCIParamGet", status) {
            break 'read false;
        }
        let (status, bytes) = get(OCI_ATTR_DATA_TYPE);
        if call(c"OCIAttrGet: \"OCI_ATTR_DATA_TYPE\"", status) {
            break 'read false;
        }
        let dtype = u16::from_ne_bytes([bytes[0], bytes[1]]);
        let (status, bytes) = get(OCI_ATTR_PRECISION);
        if call(c"OCIAttrGet: \"OCI_ATTR_PRECISION\"", status) {
            break 'read false;
        }
        let precision = u16::from_ne_bytes([bytes[0], bytes[1]]);
        let (status, bytes) = get(OCI_ATTR_SCALE);
        if call(c"OCIAttrGet: \"OCI_ATTR_SCALE\"", status) {
            break 'read false;
        }
        let scale = i8::from_ne_bytes([bytes[0]]);
        let mut nchar = false;
        if matches!(dtype, SQLT_CHR | SQLT_VCS | SQLT_AFC | SQLT_CLOB) {
            let (status, bytes) = get(OCI_ATTR_CHARSET_FORM);
            if call(c"OCIAttrGet: \"OCI_ATTR_CHARSET_FORM\"", status) {
                break 'read false;
            }
            nchar = bytes[0] == SQLCS_NCHAR;
        }
        if dtype == 0 {
            // No declared type: a NULL column.
            meta.insert_long(b"oci:decl_type", 0);
            meta.insert_bytes(b"native_type", Some(b"NULL"));
        } else if let Some(name) = type_name(dtype, precision, scale, nchar) {
            meta.insert_bytes(b"oci:decl_type", Some(name));
            meta.insert_bytes(b"native_type", Some(name));
        } else {
            meta.insert_long(b"oci:decl_type", dtype.into());
            meta.insert_bytes(b"native_type", Some(b"UNKNOWN"));
        }
        if matches!(dtype, SQLT_BLOB | SQLT_CLOB | SQLT_BFILE) {
            flags.push_bytes(b"blob");
        }
        // C set this here, BFILE as STR, then again below with BFILE as LOB;
        // the second write keeps the first one's place among the keys.
        let lob = matches!(dtype, SQLT_BLOB | SQLT_CLOB);
        meta.insert_long(b"pdo_type", if lob { PARAM_LOB } else { PARAM_STR }.into());
        let (status, bytes) = get(OCI_ATTR_IS_NULL);
        if call(c"OCIAttrGet: \"OCI_ATTR_IS_NULL\"", status) {
            break 'read false;
        }
        flags.push_bytes(if bytes[0] != 0 { b"nullable" } else { b"not_null" });
        let lob = lob || dtype == SQLT_BFILE;
        meta.insert_long(b"pdo_type", if lob { PARAM_LOB } else { PARAM_STR }.into());
        meta.insert_long(b"scale", scale.into());
        true
    };
    if done {
        meta.insert(b"flags", flags.into_owned());
    }
    drop(param);
    output.set(meta.into_owned());
}

/// `oci_stmt_fetch`: the next row PDO's orientation asks for (`OCIStmtFetch2`
/// through the coroutine hook): true for a row, false at the end, or with the
/// error recorded on the statement.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_fetch(stmt: &PdoStmt, dbh: &Dbh, orientation: u32, offset: zend_long) -> bool {
    // PDO_FETCH_ORI_NEXT, PRIOR, FIRST, LAST, ABS, REL; anything else is next.
    let orientation: u16 = match orientation {
        1 => 0x10,
        2 => 0x04,
        3 => 0x08,
        4 => 0x20,
        5 => 0x40,
        _ => 0x02,
    };
    let err = swr_oci_stmt_err(stmt);
    let status = swoole_oci_stmt_fetch2(
        swr_oci_stmt_handle(stmt),
        err,
        1,
        orientation,
        // `(sb4) offset`, as C truncates it.
        offset.c_cast(),
        0,
    );
    match status {
        OCI_NO_DATA => false,
        OCI_SUCCESS | OCI_SUCCESS_WITH_INFO => true,
        OCI_NEED_DATA => {
            stmt_call(dbh, stmt, err, c"OCI_NEED_DATA", status, (UPSTREAM_STATEMENT, 569));
            false
        }
        _ => {
            stmt_call(dbh, stmt, err, c"OCIStmtFetch", status, (UPSTREAM_STATEMENT, 577));
            false
        }
    }
}

/// `oci_stmt_execute`: the statement run (a scrollable cursor, a commit on
/// success outside a transaction, or neither), its first execution setting up
/// the columns, then the row count. A query with no LOB parameter runs no
/// iteration, leaving its rows to fetch. False when a call recorded an Oracle
/// error, as C's `return 0` was.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_execute(stmt: &PdoStmt, dbh: &Dbh) -> bool {
    let (handle, err) = (swr_oci_stmt_handle(stmt), swr_oci_stmt_err(stmt));
    // Upstream's location, which bug44301.phpt matches (see UPSTREAM_STATEMENT).
    let call = |what: &CStr, status, line| stmt_call(dbh, stmt, err, what, status, (UPSTREAM_STATEMENT, line)) != 0;
    let get = |attribute| {
        let mut value = 0u32;
        let status = OCIAttrGetStmt(handle, OCI_HTYPE_STMT, &mut value, None, attribute, err);
        (status, value)
    };
    if swr_oci_stmt_type(stmt) == 0 {
        let (status, kind) = get(OCI_ATTR_STMT_TYPE);
        // A ub2, in the low bytes of the u32 in native order.
        let bytes = kind.to_ne_bytes();
        swr_oci_set_stmt_type(stmt, u16::from_ne_bytes([bytes[0], bytes[1]]));
        if call(c"OCIAttrGet: \"OCI_ATTR_STMT_TYPE\"", status, 136) {
            return false;
        }
    }
    if stmt.executed() {
        // Cancel the cursor a previous fetch left open; best effort, as in C.
        let _status = swoole_oci_stmt_fetch(handle, err, 0, OCI_FETCH_NEXT, 0);
    }
    let mode = if swr_oci_stmt_scrollable(stmt) {
        OCI_STMT_SCROLLABLE_READONLY
    } else if dbh.auto_commit() && !dbh.in_txn() {
        OCI_COMMIT_ON_SUCCESS
    } else {
        0
    };
    // A query runs no iteration unless a LOB parameter is bound, as in C.
    let iterations = u32::from(swr_oci_stmt_type(stmt) != OCI_STMT_SELECT || swr_oci_stmt_have_blobs(stmt));
    let none = Live(ptr::null_mut(), PhantomData);
    let status = swoole_oci_stmt_execute(crate::handle::swr_oci_svc(dbh), handle, err, iterations, 0, none, none, mode);
    if call(c"OCIStmtExecute", status, 156) {
        return false;
    }
    if !stmt.executed() {
        let (status, count) = get(OCI_ATTR_PARAM_COUNT);
        if call(c"OCIAttrGet: \"ATTR_PARAM_COUNT\"", status, 165) {
            return false;
        }
        php_pdo_stmt_set_column_count(stmt, 0);
        stmt.set_result_counts(count.c_cast(), stmt.row_count());
        crate::columns::reset(stmt, count);
    }
    let (status, rows) = get(OCI_ATTR_ROW_COUNT);
    if call(c"OCIAttrGet: \"ATTR_ROW_COUNT\"", status, 190) {
        return false;
    }
    stmt.set_row_count(rows.into());
    true
}

/// Cancel a SELECT without draining the rows. DML statements have no cursor.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_close_cursor(stmt: &PdoStmt, dbh: &Dbh) -> bool {
    if swr_oci_stmt_type(stmt) != OCI_STMT_SELECT {
        return true;
    }
    let err = swr_oci_stmt_err(stmt);
    let status = swoole_oci_stmt_fetch(swr_oci_stmt_handle(stmt), err, 0, OCI_FETCH_NEXT, 0);
    stmt_call(dbh, stmt, err, c"OCIStmtFetch", status, (UPSTREAM_STATEMENT, 1116)) == 0
}

/// `oci_stmt_dtor`'s statement handles: the cursor cancelled if rows were left,
/// then the statement and its error handle freed.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_stmt_close(stmt: &PdoStmt) {
    let (handle, err) = (swr_oci_stmt_handle(stmt), swr_oci_stmt_err(stmt));
    if !handle.is_null() {
        // Cancelling and freeing are best effort, as in C.
        let _status = swoole_oci_stmt_fetch(handle, err, 0, OCI_FETCH_NEXT, 0);
        let _status = OCIHandleFreeStmt(handle.any(), OCI_HTYPE_STMT);
    }
    if !err.is_null() {
        // Freeing fails only for a null or foreign handle.
        let _status = OCIHandleFreeStmt(err.any(), OCI_HTYPE_ERROR);
    }
}
}
