//! `pdo_firebird`'s statement methods (`firebird_statement.c`): so far the
//! column description and metadata, which read a column's `XSQLVAR` in the
//! statement's output descriptor, and the integer, string and bool values. The methods PDO calls stay C, as thin
//! callers that find the descriptor in their statement record.
use std::ffi::c_char;
use swoole_rs_corebase::lent::{Count, In};
use swoole_rs_firebird_sys::ibase::{
    SQL_BOOLEAN, SQL_DOUBLE, SQL_FLOAT, SQL_INT64, SQL_LONG, SQL_SHORT, SQL_TEXT, SQL_VARYING,
    XSQLVAR,
};
use swoole_rs_zend::pdo::Slot;
use swoole_rs_zend::sys::{
    pdo_column_data, pdo_param_type_PDO_PARAM_BOOL as PARAM_BOOL,
    pdo_param_type_PDO_PARAM_INT as PARAM_INT, pdo_param_type_PDO_PARAM_STR as PARAM_STR,
};
use swoole_rs_zend::{Owned, OwnedArray, ZString};

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

/// The column's integer of `N` bytes, as C copied it out of `sqldata`.
fn integer<const N: usize>(data: &[u8]) -> Option<[u8; N]> {
    data.get(..N)?.try_into().ok()
}

/// A scaled integer's digits, as `pdo_firebird_stmt_get_col` printed them:
/// `n / 10^scale`, a point, then the remainder padded to `scale` digits, with
/// the sign before a zero integer part kept.
fn scaled(n: i64, scale: u32) -> Option<String> {
    let f = 10_i64.checked_pow(scale)?;
    let width = usize::try_from(scale).ok()?;
    Some(if n >= 0 {
        format!("{}.{:0width$}", n / f, n % f)
    } else if n <= -f {
        // C's `-n % f` of the negative value.
        format!("{}.{:0width$}", n / f, n.wrapping_neg() % f)
    } else {
        format!("-0.{:0width$}", n.wrapping_neg() % f)
    })
}

/// `pdo_firebird_stmt_get_col` for a null value, a scaled `SMALLINT`,
/// `INTEGER`, `BIGINT` or `DOUBLE`, a `VARCHAR` or `CHAR`, an unscaled integer,
/// a `BOOLEAN`, `FLOAT` and `DOUBLE`: the value in `result`, read from the lent `sqldata` bytes. False,
/// setting nothing, for the types C still reads.
#[unsafe(no_mangle)]
pub extern "C" fn swr_firebird_value<'a>(
    var: &XSQLVAR,
    null: bool,
    data: In<'a, u8>,
    len: Count<'a>,
    result: &Slot,
) -> bool {
    let data = data.drain(len).bytes();
    let value = if null {
        Some(Owned::null())
    } else if var.sqlscale < 0 {
        let n = match base_type(var.sqltype) {
            SQL_SHORT => integer::<2>(data).map(|n| i16::from_ne_bytes(n).into()),
            SQL_LONG => integer::<4>(data).map(|n| i32::from_ne_bytes(n).into()),
            SQL_INT64 => integer::<8>(data).map(i64::from_ne_bytes),
            _ => None,
        };
        let scale = var.sqlscale.unsigned_abs();
        if base_type(var.sqltype) == SQL_DOUBLE {
            // A dialect 1 NUMERIC or DECIMAL, held as a double.
            integer::<8>(data)
                .map(|n| Owned::string(ZString::double_fixed(f64::from_ne_bytes(n), scale.into())))
        } else {
            let digits = n.and_then(|n| scaled(n, u32::from(scale)));
            digits.map(|digits| Owned::string(ZString::new(digits.as_bytes())))
        }
    } else {
        match base_type(var.sqltype) {
            SQL_VARYING => integer::<2>(data).map(|length| {
                let length = usize::try_from(i16::from_ne_bytes(length)).unwrap_or(0);
                let text = data.get(2..).unwrap_or_default();
                Owned::string(ZString::new(text.get(..length).unwrap_or(text)))
            }),
            SQL_TEXT => Some(Owned::string(ZString::new(data))),
            SQL_SHORT => integer::<2>(data).map(|n| Owned::long(i16::from_ne_bytes(n).into())),
            SQL_LONG => integer::<4>(data).map(|n| Owned::long(i32::from_ne_bytes(n).into())),
            // C's SIZEOF_ZEND_LONG >= 8, else the digits.
            SQL_INT64 if cfg!(target_pointer_width = "64") => {
                integer::<8>(data).map(|n| Owned::long(i64::from_ne_bytes(n)))
            }
            SQL_INT64 => integer::<8>(data)
                .map(|n| Owned::string(ZString::new(i64::from_ne_bytes(n).to_string().as_bytes()))),
            SQL_BOOLEAN => data.first().map(|&b| Owned::bool(b != 0)),
            // Strings, as C printed them: a `FLOAT` to 8 digits, a `DOUBLE` to 16.
            SQL_FLOAT => integer::<4>(data)
                .map(|n| Owned::string(ZString::double_digits(f32::from_ne_bytes(n).into(), 8))),
            SQL_DOUBLE => integer::<8>(data)
                .map(|n| Owned::string(ZString::double_digits(f64::from_ne_bytes(n), 16))),
            _ => None,
        }
    };
    let Some(value) = value else {
        return false;
    };
    result.set(value);
    true
}
