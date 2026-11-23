//! `pdo_firebird`'s preparer (`firebird_driver.c`'s `firebird_handle_preparer`
//! and `php_firebird_alloc_prepare_stmt`): the statement allocated and
//! prepared with its named parameters replaced, its type read, and its output
//! and input descriptors described. The statement record and the descriptors'
//! storage stay C, which allocates them between these calls.
use std::ffi::{CStr, c_char, c_short, c_ushort};

use swoole_rs_corebase::lent::{Count, InOut, lend_in, lend_out};
use swoole_rs_firebird_sys::ibase::{XSQLVAR, isc_info_sql_stmt_type};
use swoole_rs_firebird_sys::{
    Bytes, Handle, Sqlda, Status, isc_dsql_describe, isc_dsql_describe_bind,
};
use swoole_rs_zend::pdo::Dbh;
use swoole_rs_zend::{Array, ZStr};

use crate::error::{php_firebird_set_error, status_error};
use crate::statement::SQLDA_VERSION;
use crate::{
    swoole_isc_dsql_allocate_statement, swoole_isc_dsql_prepare, swoole_isc_dsql_sql_info,
};

unsafe extern "C" {
    /// The connection's status vector, `&H->db` and `&H->tr`.
    safe fn swr_firebird_status(dbh: &Dbh) -> Status;
    safe fn swr_firebird_db(dbh: &Dbh) -> Handle<'_>;
    safe fn swr_firebird_tr(dbh: &Dbh) -> Handle<'_>;
    /// The connection's SQL dialect (`H->sql_dialect`).
    safe fn swr_firebird_dialect(dbh: &Dbh) -> c_ushort;
}

/// `php_firebird_alloc_prepare_stmt`: a statement allocated into `stmt` and
/// `sql` prepared in it, in the connection's transaction and dialect, its
/// columns described into `out_sqlda`. Each `:name` becomes `?`, its index
/// recorded in `named_params` when given. False with the error recorded on the
/// connection: `07000` when the SQL can't be preprocessed.
#[unsafe(no_mangle)]
pub extern "C" fn php_firebird_alloc_prepare_stmt(
    dbh: &Dbh,
    sql: &ZStr,
    out_sqlda: Sqlda,
    stmt: Handle<'_>,
    named_params: Option<&Array>,
) -> bool {
    let status = swr_firebird_status(dbh);
    if swoole_isc_dsql_allocate_statement(status, swr_firebird_db(dbh), stmt) != 0 {
        status_error(dbh, None);
        return false;
    }
    let Some(mut prepared) = crate::preprocess::named(sql, named_params) else {
        lend_in(b"07000", |state, state_len| {
            lend_in(&[], |message, message_len| {
                php_firebird_set_error(dbh, None, state, state_len, message, message_len);
            });
        });
        return false;
    };
    prepared.push(0);
    // To its first NUL, as Firebird reads the C string when given no length.
    let text = CStr::from_bytes_until_nul(&prepared).unwrap_or_default();
    let failed = swoole_isc_dsql_prepare(
        status,
        swr_firebird_tr(dbh),
        stmt,
        0,
        Bytes::from_c_str(text),
        swr_firebird_dialect(dbh),
        out_sqlda,
    ) != 0;
    if failed {
        status_error(dbh, None);
    }
    !failed
}

/// The size of the preparer's statement-type reply (`result`).
const TYPE_REPLY: usize = 8;

/// `firebird_handle_preparer` once the statement is prepared: its type (the
/// reply's fourth byte, after the item and its length) into `kind`, its columns
/// described into `out_sqlda` (the record's), and its parameters counted into
/// `num_sqlda`. False when a call fails, the describing's error recorded on the
/// connection, as C recorded it there and again on the way out.
#[unsafe(no_mangle)]
pub extern "C" fn swr_firebird_describe_stmt(
    dbh: &Dbh,
    stmt: Handle<'_>,
    kind: &mut c_char,
    out_sqlda: Sqlda,
    num_sqlda: Sqlda,
) -> bool {
    let status = swr_firebird_status(dbh);
    let item = c_char::from_ne_bytes([u8::try_from(isc_info_sql_stmt_type).unwrap_or_default()]);
    let mut reply = [0u8; TYPE_REPLY];
    let failed = lend_in(&[item], |items, items_length| {
        lend_out(&mut reply, |out, len| {
            swoole_isc_dsql_sql_info(status, stmt, items_length, items, len, out.chars())
        })
    }) != 0;
    if failed {
        return false;
    }
    let [_, _, _, statement_type, ..] = reply;
    *kind = c_char::from_ne_bytes([statement_type]);
    if isc_dsql_describe(status, stmt, SQLDA_VERSION, out_sqlda) != 0 {
        status_error(dbh, None);
        return false;
    }
    isc_dsql_describe_bind(status, stmt, SQLDA_VERSION, num_sqlda) == 0
}

/// `firebird_handle_preparer`'s input descriptor: the statement's parameters
/// described into `in_sqlda`, allocated for as many as it counted, whose
/// variables are `vars`; then each made nullable (the low bit of its type),
/// as the description fills every one. False when the describing fails.
#[unsafe(no_mangle)]
pub extern "C" fn swr_firebird_describe_input<'v>(
    dbh: &Dbh,
    stmt: Handle<'_>,
    in_sqlda: Sqlda,
    vars: InOut<'v, XSQLVAR>,
    count: Count<'v, c_short>,
) -> bool {
    if isc_dsql_describe_bind(swr_firebird_status(dbh), stmt, SQLDA_VERSION, in_sqlda) != 0 {
        return false;
    }
    // Lent only now: the describing wrote them through `in_sqlda`.
    for var in vars.items(count).unwrap_or_default() {
        var.sqltype |= 1;
    }
    true
}
