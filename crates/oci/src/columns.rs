//! A statement's result columns (`php_pdo_oci_int.h`'s `pdo_oci_column`):
//! `describe` defines each on the statement, OCI writes each fetched value into
//! the column's buffer (or a LOB's locator) and its indicator, length and return
//! code, and `get_col` reads them back. The C statement keeps them by pointer.
use crate::{
    Env, Error, Live, Lob, Out, Stmt, Svc, Sword,
    error::{Nothing, here, stmt_call},
    handle::{swr_oci_env, swr_oci_err, swr_oci_max_char_width, swr_oci_svc},
    statement::{Descriptor, OCIAttrGetParam, OCIParamGet, Param},
};
use std::{cell::Cell, ffi::c_void, marker::PhantomData, ptr};
use swoole_rs_corebase::CCast;
use swoole_rs_zend::{
    E_WARNING, Owned, ZString,
    pdo::{Dbh, Slot, Stmt as PdoStmt},
    sys::{pdo_column_data, zend_string},
};

/// `OCI_HTYPE_STMT`, `OCI_DTYPE_PARAM` and `OCI_DTYPE_LOB`.
const OCI_HTYPE_STMT: u32 = 4;
const OCI_DTYPE_PARAM: u32 = 53;
const OCI_DTYPE_LOB: u32 = 50;
/// `OCI_ATTR_DATA_SIZE`, `OCI_ATTR_DATA_TYPE` and `OCI_ATTR_PRECISION`.
const OCI_ATTR_DATA_SIZE: u32 = 1;
const OCI_ATTR_DATA_TYPE: u32 = 2;
const OCI_ATTR_PRECISION: u32 = 5;
/// `OCI_DYNAMIC_FETCH`: OCI asks [`define_callback`] for a LOB's buffer.
const OCI_DYNAMIC_FETCH: u32 = 2;
/// `OCI_CONTINUE` and `OCI_ONE_PIECE`.
const OCI_CONTINUE: Sword = -24200;
const OCI_ONE_PIECE: u8 = 0;
/// The `SQLT_*` types `describe` sizes.
const SQLT_CHR: u16 = 1;
const SQLT_NUM: u16 = 2;
const SQLT_LNG: u16 = 8;
const SQLT_DAT: u16 = 12;
const SQLT_BIN: u16 = 23;
const SQLT_LBI: u16 = 24;
const SQLT_IBFLOAT: u16 = 100;
const SQLT_IBDOUBLE: u16 = 101;
const SQLT_RDD: u16 = 104;
const SQLT_CLOB: u16 = 112;
const SQLT_BLOB: u16 = 113;
const SQLT_TIMESTAMP: u16 = 187;
const SQLT_TIMESTAMP_TZ: u16 = 188;

/// One result column. OCI keeps pointers to the fields it writes (the define
/// handle, buffer, indicator, length and return code) from `describe` until the
/// statement is destroyed, so they are `Cell`s in a column that stays where the
/// boxed slice put it.
pub(crate) struct Column {
    def: Cell<crate::Handle>,
    fetched_len: Cell<u16>,
    retcode: Cell<u16>,
    indicator: Cell<i16>,
    datalen: Cell<u32>,
    dtype: Cell<u16>,
    /// The fetched value, one byte longer than OCI is told, as C allocated it.
    buffer: Cell<Option<Box<[Cell<u8>]>>>,
    /// A BLOB's or CLOB's locator, null for other columns.
    lob: Cell<Live<Lob>>,
}

impl Column {
    fn new() -> Self {
        Self {
            def: Cell::new(ptr::null_mut()),
            fetched_len: Cell::new(0),
            retcode: Cell::new(0),
            indicator: Cell::new(0),
            datalen: Cell::new(0),
            dtype: Cell::new(0),
            buffer: Cell::new(None),
            lob: Cell::new(Live(ptr::null_mut(), PhantomData)),
        }
    }

    fn is_lob(&self) -> bool {
        matches!(self.dtype.get(), SQLT_BLOB | SQLT_CLOB)
    }

    /// The first `length` bytes OCI fetched into the buffer.
    fn bytes(&self, length: usize) -> Vec<u8> {
        let buffer = self.buffer.take();
        let bytes = buffer.as_deref().map_or_else(Vec::new, |cells| {
            cells.iter().take(length).map(Cell::get).collect()
        });
        self.buffer.set(buffer);
        bytes
    }
}

/// A statement's columns, from its first execution.
pub(crate) struct Columns(Box<[Column]>);

/// Where the C statement keeps its columns: none before its first execution.
type Slot_ = Cell<Option<Box<Columns>>>;

/// `f` of the statement's columns (none before the first execution). The
/// columns stay where they are while `f` runs, so what OCI was given of them
/// stays valid.
fn with_columns<R>(stmt: &PdoStmt, f: impl FnOnce(Option<&Columns>) -> R) -> R {
    let slot = swr_oci_stmt_columns(stmt);
    let columns = slot.take();
    let result = f(columns.as_deref());
    slot.set(columns);
    result
}

/// The OCI define callback's signature: OCI asks for the buffer, length and
/// indicator to fetch a dynamically defined column into.
type DefineCallback = extern "C" fn(
    column: &Column,
    define: crate::Handle,
    iteration: u32,
    buffer: &mut *mut c_void,
    length: &mut *mut u32,
    piece: &mut u8,
    indicator: &mut *mut c_void,
    retcode: &mut *mut u16,
) -> Sword;

#[expect(
    improper_ctypes,
    reason = "C and OCI never read a column or the columns: they keep the pointer and pass it back"
)]
unsafe extern "C" {
    /// `&S->cols`, which lives as long as the statement does.
    safe fn swr_oci_stmt_columns(stmt: &PdoStmt) -> &Slot_;
    /// A LOB column's value: `oci_create_lob_stream`, `OCILobOpen` read-only and
    /// the stream in `result`.
    safe fn swr_oci_lob_value(stmt: &PdoStmt, lob: Live<Lob>, result: &Slot);
    /// `OCI_ATTR_NAME` of a column's descriptor as a new string, with OCI's
    /// status in `status`; null unless the call succeeded.
    safe fn swr_oci_param_name(
        param: Live<Param>,
        err: Live<Error>,
        status: &mut Sword,
    ) -> *mut zend_string;
    /// OCI's own declaration: a new LOB locator, written to `lob`.
    safe fn OCIDescriptorAlloc(
        env: Live<Env>,
        lob: &mut crate::Handle,
        kind: u32,
        extra: usize,
        user_memory: Option<&'static Nothing>,
    ) -> Sword;
    /// OCI's own declaration: define column `position` into `value`, keeping
    /// the indicator, length and return code to write at each fetch.
    safe fn OCIDefineByPos(
        stmt: Live<Stmt>,
        define: &Cell<crate::Handle>,
        err: Live<Error>,
        position: u32,
        value: Out,
        size: i32,
        kind: u16,
        indicator: &Cell<i16>,
        length: &Cell<u16>,
        retcode: &Cell<u16>,
        mode: u32,
    ) -> Sword;
    /// OCI's own declaration: fetch the define through `callback`, with
    /// `column` as its context.
    safe fn OCIDefineDynamic(
        define: crate::Handle,
        err: Live<Error>,
        column: &Column,
        callback: DefineCallback,
    ) -> Sword;
    /// OCI's own declaration: whether the locator is a temporary LOB.
    safe fn OCILobIsTemporary(
        env: Live<Env>,
        err: Live<Error>,
        lob: Live<Lob>,
        flag: &mut i32,
    ) -> Sword;
    /// OCI's own declaration: free a temporary LOB's storage.
    safe fn OCILobFreeTemporary(svc: Live<Svc>, err: Live<Error>, lob: Live<Lob>) -> Sword;
    /// OCI's own declaration for the locator this frees.
    #[link_name = "OCIDescriptorFree"]
    safe fn OCIDescriptorFreeLob(lob: Live<Lob>, kind: u32) -> Sword;
}

/// `oci_define_callback`: a LOB column's locator as its buffer, in one piece.
extern "C" fn define_callback(
    column: &Column,
    _define: crate::Handle,
    _iteration: u32,
    buffer: &mut *mut c_void,
    length: &mut *mut u32,
    piece: &mut u8,
    indicator: &mut *mut c_void,
    _retcode: &mut *mut u16,
) -> Sword {
    // Only LOB columns are defined dynamically.
    if column.is_lob() {
        *piece = OCI_ONE_PIECE;
        *buffer = column.lob.get().0;
        *length = column.datalen.as_ptr();
        *indicator = column.indicator.as_ptr().cast();
    }
    OCI_CONTINUE
}

swoole_rs_zend::exports! {
/// `oci_stmt_execute`'s first-time column setup: `count` empty columns. As in
/// C, a LOB locator left from an earlier execution is not freed, as a LOB stream
/// may still read it.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_columns_reset(stmt: &PdoStmt, count: u32) {
    let slot = swr_oci_stmt_columns(stmt);
    if let Some(old) = slot.take() {
        for column in &old.0 {
            column.lob.set(Live(ptr::null_mut(), PhantomData));
        }
    }
    slot.set(Some(Box::new(Columns((0..count).map(|_| Column::new()).collect()))));
}

/// `oci_stmt_dtor`'s column cleanup: each LOB locator freed, its temporary LOB
/// first while the connection's object is still usable.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_columns_free(stmt: &PdoStmt, dbh: &Dbh, usable: bool) {
    let Some(columns) = swr_oci_stmt_columns(stmt).take() else {
        return;
    };
    for column in &columns.0 {
        let lob = column.lob.get();
        if !column.is_lob() || lob.is_null() {
            continue;
        }
        let err = swr_oci_err(dbh);
        if usable {
            let mut temporary = 0;
            // A failed check leaves it 0: not temporary, as in C.
            let _status = OCILobIsTemporary(swr_oci_env(dbh), err, lob, &mut temporary);
            if temporary != 0 {
                // Freeing the temporary LOB is best effort, as in C.
                let _status = OCILobFreeTemporary(swr_oci_svc(dbh), err, lob);
            }
        }
        // Freeing fails only for a null or foreign locator.
        let _status = OCIDescriptorFreeLob(lob, OCI_DTYPE_LOB);
    }
}

/// `oci_stmt_describe`: column `colno`'s name, length and precision for PDO,
/// and its define on the statement (a buffer sized for its type, or a LOB
/// locator fetched through [`define_callback`]). False when a call recorded an
/// Oracle error, as C's `return 0` was.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_describe(
    stmt: &PdoStmt,
    dbh: &Dbh,
    colno: u32,
    desc: &mut pdo_column_data,
) -> bool {
    let (handle, err) = (crate::statement::swr_oci_stmt_handle(stmt), crate::statement::swr_oci_stmt_err(stmt));
    let call = |what: &std::ffi::CStr, status| stmt_call(dbh, stmt, err, what, status, here!()) != 0;
    let position = colno.wrapping_add(1);
    let mut raw = ptr::null_mut();
    let status = OCIParamGet(handle, OCI_HTYPE_STMT, err, &mut raw, position);
    let param = Descriptor(Live(raw, PhantomData));
    if call(c"OCIParamGet", status) {
        return false;
    }
    let get = |attribute| {
        let mut value = 0u32;
        let status = OCIAttrGetParam(param.0, OCI_DTYPE_PARAM, &mut value, None, attribute, err);
        let bytes = value.to_ne_bytes();
        (status, u16::from_ne_bytes([bytes[0], bytes[1]]))
    };
    let (status, dtype) = get(OCI_ATTR_DATA_TYPE);
    if call(c"OCIAttrGet: \"OCI_ATTR_DATA_TYPE\"", status) {
        return false;
    }
    let (status, size) = get(OCI_ATTR_DATA_SIZE);
    if call(c"OCIAttrGet: \"OCI_ATTR_DATA_SIZE\"", status) {
        return false;
    }
    let (status, precision) = get(OCI_ATTR_PRECISION);
    if call(c"OCIAttrGet: \"OCI_ATTR_PRECISION\"", status) {
        return false;
    }
    let mut status = 0;
    let name = swr_oci_param_name(param.0, err, &mut status);
    if call(c"OCIAttrGet: \"OCI_ATTR_NAME\"", status) {
        return false;
    }
    desc.precision = precision.into();
    desc.maxlen = size.into();
    desc.name = if name.is_null() { ZString::new(b"").into_raw() } else { name };
    let maxlen = u64::from(size);
    with_columns(stmt, |columns| {
        let Some(column) = columns.and_then(|columns| columns.0.get(colno.c_cast::<usize>())) else {
            return true;
        };
        column.dtype.set(dtype);
        let mut dynamic = false;
        let define_type = match dtype {
            SQLT_LBI | SQLT_LNG => {
                column.datalen.set(512);
                if dtype == SQLT_LBI { SQLT_BIN } else { SQLT_CHR }
            }
            SQLT_BLOB | SQLT_CLOB => {
                let mut lob = ptr::null_mut();
                let status = OCIDescriptorAlloc(swr_oci_env(dbh), &mut lob, OCI_DTYPE_LOB, 0, None);
                column.lob.set(Live(lob, PhantomData));
                if call(c"OCIDescriptorAlloc", status) {
                    return false;
                }
                column.datalen.set(size_of::<*mut c_void>().c_cast());
                dynamic = true;
                dtype
            }
            _ => {
                let length = match dtype {
                    SQLT_DAT | SQLT_NUM | SQLT_RDD | SQLT_TIMESTAMP | SQLT_TIMESTAMP_TZ => 512,
                    SQLT_IBFLOAT | SQLT_IBDOUBLE => 1024,
                    // Raw bytes come back as two hex digits each.
                    SQLT_BIN => maxlen * 2,
                    _ => maxlen * u64::from(swr_oci_max_char_width(dbh)),
                };
                // `(ub4)` of the product, as C truncates it.
                column.datalen.set(length.c_cast());
                SQLT_CHR
            }
        };
        let value = if dynamic {
            Out::handle(column.lob.get())
        } else {
            let length = column.datalen.get().c_cast::<usize>() + 1;
            column.buffer.set(Some((0..length).map(|_| Cell::new(0)).collect()));
            let buffer = column.buffer.take();
            let value = buffer.as_deref().map_or(Out::handle(column.lob.get()), Out::cells);
            column.buffer.set(buffer);
            value
        };
        let status = OCIDefineByPos(
            handle,
            &column.def,
            err,
            position,
            value,
            column.datalen.get().cast_signed(),
            define_type,
            &column.indicator,
            &column.fetched_len,
            &column.retcode,
            if dynamic { OCI_DYNAMIC_FETCH } else { 0 },
        );
        if call(c"OCIDefineByPos", status) {
            return false;
        }
        if dynamic {
            let status = OCIDefineDynamic(column.def.get(), err, column, define_callback);
            if call(c"OCIDefineDynamic", status) {
                return false;
            }
        }
        true
    })
}

/// `oci_stmt_get_col`: column `colno`'s fetched value: null, a LOB as a stream,
/// or the bytes fetched (with a warning when they were truncated). False for a
/// LOB column with no locator, as in C.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_column_value(stmt: &PdoStmt, colno: u32, result: &Slot) -> bool {
    with_columns(stmt, |columns| {
        let Some(column) = columns.and_then(|columns| columns.0.get(colno.c_cast::<usize>())) else {
            return false;
        };
        let length = usize::from(column.fetched_len.get());
        match column.indicator.get() {
            -1 => result.set(Owned::null()),
            0 if column.is_lob() => {
                if column.lob.get().is_null() {
                    return false;
                }
                swr_oci_lob_value(stmt, column.lob.get(), result);
            }
            0 => result.set(Owned::string(ZString::new(&column.bytes(length)))),
            _ => {
                let warning = format!(
                    "Column {colno} data was too large for buffer and was truncated to fit it"
                );
                if let Ok(warning) = std::ffi::CString::new(warning) {
                    swoole_rs_zend::bailout::defer(swoole_rs_zend::report(E_WARNING, &warning));
                }
                // A LOB column has no buffer (C read the locator's bytes here).
                result.set(Owned::string(ZString::new(&column.bytes(length))));
            }
        }
        true
    })
}
}
