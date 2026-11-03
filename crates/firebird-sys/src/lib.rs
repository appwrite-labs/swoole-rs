//! The Firebird client API calls Swoole's bundled `pdo_firebird` makes, as
//! `ibase.h` (Firebird 3 and later, which `config.m4` requires) declares them.
//! The extension links the system `libfbclient` itself (`--with-swoole-firebird`,
//! through `fb_config`); nothing here links it, so the workspace builds and
//! tests on machines without Firebird.
#![expect(
    non_camel_case_types,
    reason = "the C type names, as ibase.h spells them"
)]

use std::ffi::{c_char, c_int, c_short, c_uint, c_ushort, c_void};

/// `ISC_STATUS`: `intptr_t`.
pub type ISC_STATUS = isize;
/// `FB_API_HANDLE`, the type of every `isc_*_handle`.
pub type FB_API_HANDLE = c_uint;
pub type isc_db_handle = FB_API_HANDLE;
pub type isc_tr_handle = FB_API_HANDLE;
pub type isc_stmt_handle = FB_API_HANDLE;
pub type isc_blob_handle = FB_API_HANDLE;
/// `ISC_SCHAR`.
pub type ISC_SCHAR = c_char;
/// `XSQLDA`, only ever passed by pointer here.
#[repr(C)]
pub struct XSQLDA {
    _opaque: [u8; 0],
}
/// `ISC_QUAD`, only ever passed by pointer here.
#[repr(C)]
pub struct ISC_QUAD {
    _opaque: [u8; 0],
}
/// `ISC_VERSION_CALLBACK`.
pub type ISC_VERSION_CALLBACK = Option<unsafe extern "C" fn(*mut c_void, *const c_char)>;

unsafe extern "C" {
    pub fn isc_attach_database(
        status: *mut ISC_STATUS,
        name_length: c_short,
        name: *const ISC_SCHAR,
        db: *mut isc_db_handle,
        dpb_length: c_short,
        dpb: *const ISC_SCHAR,
    ) -> ISC_STATUS;
    pub fn isc_detach_database(status: *mut ISC_STATUS, db: *mut isc_db_handle) -> ISC_STATUS;
    pub fn isc_dsql_execute(
        status: *mut ISC_STATUS,
        tr: *mut isc_tr_handle,
        stmt: *mut isc_stmt_handle,
        dialect: c_ushort,
        sqlda: *const XSQLDA,
    ) -> ISC_STATUS;
    pub fn isc_dsql_execute2(
        status: *mut ISC_STATUS,
        tr: *mut isc_tr_handle,
        stmt: *mut isc_stmt_handle,
        dialect: c_ushort,
        in_sqlda: *const XSQLDA,
        out_sqlda: *const XSQLDA,
    ) -> ISC_STATUS;
    pub fn isc_dsql_sql_info(
        status: *mut ISC_STATUS,
        stmt: *mut isc_stmt_handle,
        items_length: c_short,
        items: *const ISC_SCHAR,
        buffer_length: c_short,
        buffer: *mut ISC_SCHAR,
    ) -> ISC_STATUS;
    pub fn isc_dsql_free_statement(
        status: *mut ISC_STATUS,
        stmt: *mut isc_stmt_handle,
        option: c_ushort,
    ) -> ISC_STATUS;
    /// Variadic: each database is followed by its TPB length and TPB.
    pub fn isc_start_transaction(
        status: *mut ISC_STATUS,
        tr: *mut isc_tr_handle,
        count: c_short,
        ...
    ) -> ISC_STATUS;
    pub fn isc_commit_retaining(status: *mut ISC_STATUS, tr: *mut isc_tr_handle) -> ISC_STATUS;
    pub fn isc_commit_transaction(status: *mut ISC_STATUS, tr: *mut isc_tr_handle) -> ISC_STATUS;
    pub fn isc_rollback_transaction(status: *mut ISC_STATUS, tr: *mut isc_tr_handle) -> ISC_STATUS;
    pub fn isc_dsql_allocate_statement(
        status: *mut ISC_STATUS,
        db: *mut isc_db_handle,
        stmt: *mut isc_stmt_handle,
    ) -> ISC_STATUS;
    pub fn isc_dsql_prepare(
        status: *mut ISC_STATUS,
        tr: *mut isc_tr_handle,
        stmt: *mut isc_stmt_handle,
        length: c_ushort,
        statement: *const ISC_SCHAR,
        dialect: c_ushort,
        sqlda: *mut XSQLDA,
    ) -> ISC_STATUS;
    pub fn isc_dsql_fetch(
        status: *mut ISC_STATUS,
        stmt: *mut isc_stmt_handle,
        dialect: c_ushort,
        sqlda: *const XSQLDA,
    ) -> ISC_STATUS;
    pub fn isc_open_blob(
        status: *mut ISC_STATUS,
        db: *mut isc_db_handle,
        tr: *mut isc_tr_handle,
        blob: *mut isc_blob_handle,
        id: *mut ISC_QUAD,
    ) -> ISC_STATUS;
    pub fn isc_blob_info(
        status: *mut ISC_STATUS,
        blob: *mut isc_blob_handle,
        items_length: c_short,
        items: *const ISC_SCHAR,
        buffer_length: c_short,
        buffer: *mut ISC_SCHAR,
    ) -> ISC_STATUS;
    pub fn isc_get_segment(
        status: *mut ISC_STATUS,
        blob: *mut isc_blob_handle,
        length: *mut c_ushort,
        buffer_length: c_ushort,
        buffer: *mut ISC_SCHAR,
    ) -> ISC_STATUS;
    pub fn isc_put_segment(
        status: *mut ISC_STATUS,
        blob: *mut isc_blob_handle,
        length: c_ushort,
        buffer: *const ISC_SCHAR,
    ) -> ISC_STATUS;
    pub fn isc_create_blob(
        status: *mut ISC_STATUS,
        db: *mut isc_db_handle,
        tr: *mut isc_tr_handle,
        blob: *mut isc_blob_handle,
        id: *mut ISC_QUAD,
    ) -> ISC_STATUS;
    pub fn isc_close_blob(status: *mut ISC_STATUS, blob: *mut isc_blob_handle) -> ISC_STATUS;
    pub fn isc_dsql_set_cursor_name(
        status: *mut ISC_STATUS,
        stmt: *mut isc_stmt_handle,
        name: *const ISC_SCHAR,
        kind: c_ushort,
    ) -> ISC_STATUS;
    pub fn fb_ping(status: *mut ISC_STATUS, db: *mut isc_db_handle) -> ISC_STATUS;
    pub fn isc_version(
        db: *mut isc_db_handle,
        callback: ISC_VERSION_CALLBACK,
        arg: *mut c_void,
    ) -> c_int;
}
