//! The statement's methods (`oci_statement.c`): so far `getColumnMeta`. The
//! statement is still C's `pdo_oci_stmt`, read through accessors.
use crate::{
    Any, Error, Live, Stmt, Sword,
    error::{Nothing, here, stmt_call},
};
use std::{ffi::CStr, marker::PhantomData, ptr};
use swoole_rs_zend::{
    OwnedArray,
    pdo::{Dbh, Slot, Stmt as PdoStmt},
    sys::{pdo_param_type_PDO_PARAM_LOB as PARAM_LOB, pdo_param_type_PDO_PARAM_STR as PARAM_STR},
};

/// A column's parameter descriptor (`OCIParam`).
pub(crate) enum Param {}

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
    /// The statement's OCI handle (`S->stmt`), live while the statement is.
    safe fn swr_oci_stmt_handle(stmt: &PdoStmt) -> Live<Stmt>;
    /// The statement's own error handle (`S->err`), live while the statement is.
    safe fn swr_oci_stmt_err(stmt: &PdoStmt) -> Live<Error>;
    /// OCI's own declaration: column `position`'s parameter descriptor, written
    /// to `param` (null when it fails).
    safe fn OCIParamGet(
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
    safe fn OCIAttrGetParam(
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
struct Descriptor(Live<Param>);

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
}
