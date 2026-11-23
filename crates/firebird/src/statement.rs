//! `pdo_firebird`'s statement methods (`firebird_statement.c`): so far the
//! column description and metadata, which read a column's `XSQLVAR` in the
//! statement's output descriptor. The methods PDO calls stay C, as thin
//! callers that find the descriptor in their statement record.
use std::ffi::c_char;
use swoole_rs_firebird_sys::ibase::{
    SQL_BOOLEAN, SQL_INT64, SQL_LONG, SQL_SHORT, SQL_TEXT, SQL_VARYING, XSQLVAR,
};
use swoole_rs_zend::pdo::Slot;
use swoole_rs_zend::sys::{
    pdo_column_data, pdo_param_type_PDO_PARAM_BOOL as PARAM_BOOL,
    pdo_param_type_PDO_PARAM_INT as PARAM_INT, pdo_param_type_PDO_PARAM_STR as PARAM_STR,
};
use swoole_rs_zend::{OwnedArray, ZString};

/// An SQL type code without its nullable bit.
fn base_type(sqltype: i16) -> u32 {
    u32::from(sqltype.cast_unsigned() & !1)
}

/// A name of `length` bytes from its 32-byte field, as far as the field holds.
fn name(field: &[c_char; 32], length: i16) -> Vec<u8> {
    let bytes = field.map(|c| c.to_ne_bytes()[0]);
    let length = usize::try_from(length).unwrap_or(0).min(bytes.len());
    bytes.get(..length).unwrap_or_default().to_vec()
}

/// `pdo_firebird_stmt_describe`: PDO's description of the column whose
/// variable is `var`, named `relation.alias` when `fetch_table_names` and it
/// has a relation, else by its alias. A `CHAR` column is described, and fetched,
/// as a `VARCHAR`, as C rewrote it.
#[unsafe(no_mangle)]
pub extern "C" fn swr_firebird_describe(
    var: &mut XSQLVAR,
    fetch_table_names: bool,
    column: &mut pdo_column_data,
) {
    if base_type(var.sqltype) == SQL_TEXT {
        var.sqltype = i16::try_from(SQL_VARYING).unwrap_or_default() | (var.sqltype & 1);
    }
    let alias = name(&var.aliasname, var.aliasname_length);
    let mut full = Vec::with_capacity(alias.len() + 33);
    if fetch_table_names && var.relname_length != 0 {
        full.extend_from_slice(&name(&var.relname, var.relname_length));
        full.push(b'.');
    }
    full.extend_from_slice(&alias);
    // C stored the negated scale in the unsigned field, and the length.
    column.precision = i64::from(-var.sqlscale).cast_unsigned();
    column.maxlen = usize::try_from(var.sqllen).unwrap_or_default();
    column.name = ZString::new(&full).into_raw();
}

/// `pdo_firebird_stmt_get_column_meta`: the column's `pdo_type`, an integer
/// for an unscaled `SMALLINT`, `INTEGER` or `BIGINT`, a bool for a `BOOLEAN`,
/// else a string.
#[unsafe(no_mangle)]
pub extern "C" fn swr_firebird_column_meta(var: &XSQLVAR, meta: &Slot) {
    let param_type = if var.sqlscale < 0 {
        PARAM_STR
    } else {
        match base_type(var.sqltype) {
            SQL_SHORT | SQL_LONG => PARAM_INT,
            // C's SIZEOF_ZEND_LONG >= 8.
            SQL_INT64 if cfg!(target_pointer_width = "64") => PARAM_INT,
            SQL_BOOLEAN => PARAM_BOOL,
            _ => PARAM_STR,
        }
    };
    let mut array = OwnedArray::default();
    array.insert_long(b"pdo_type", param_type.into());
    meta.set(array.into_owned());
}
