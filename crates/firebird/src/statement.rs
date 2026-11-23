//! `pdo_firebird`'s statement methods (`firebird_statement.c`): so far the
//! column description, metadata and output type coercion, which read a
//! column's `XSQLVAR` in the statement's output descriptor, and the values
//! other than blobs and the time-zone types. The methods PDO calls stay C, as
//! thin callers that find the descriptor in their statement record.
use std::ffi::{c_char, c_int, c_uint};

use swoole_rs_corebase::lent::{Count, In};
use swoole_rs_corebase::sys::Text;
use swoole_rs_firebird_sys::ibase::{
    ISC_TIMESTAMP, SQL_BOOLEAN, SQL_DOUBLE, SQL_FLOAT, SQL_INT64, SQL_LONG, SQL_SHORT, SQL_TEXT,
    SQL_TIMESTAMP, SQL_TYPE_DATE, SQL_TYPE_TIME, SQL_VARYING, XSQLVAR,
};
use swoole_rs_zend::pdo::{Dbh, Slot};
use swoole_rs_zend::sys::{
    pdo_column_data, pdo_param_type_PDO_PARAM_BOOL as PARAM_BOOL,
    pdo_param_type_PDO_PARAM_INT as PARAM_INT, pdo_param_type_PDO_PARAM_STR as PARAM_STR,
};
use swoole_rs_zend::{Owned, OwnedArray, ZString};

unsafe extern "C" {
    /// The connection's date (0), time (1) or timestamp (2) format, or its
    /// default, live until it is set again.
    safe fn swr_firebird_format<'a>(dbh: &'a Dbh, which: c_int) -> Text<'a>;
    /// fbclient's decoders: a `DATE`, `TIME` or `TIMESTAMP` into the `tm`.
    safe fn isc_decode_sql_date(date: &i32, tm: &mut libc::tm);
    safe fn isc_decode_sql_time(time: &u32, tm: &mut libc::tm);
    safe fn isc_decode_timestamp(timestamp: &ISC_TIMESTAMP, tm: &mut libc::tm);
}

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

/// A `DATE`, `TIME` or `TIMESTAMP` as `strftime` prints it in the
/// connection's format for its type, at most 79 bytes as C's buffer held.
fn datetime(dbh: &Dbh, var: &XSQLVAR, data: &[u8]) -> Option<Owned> {
    let mut tm = swoole_rs_corebase::sys::zeroed::<libc::tm>();
    let which = match base_type(var.sqltype) {
        SQL_TYPE_DATE => {
            isc_decode_sql_date(&i32::from_ne_bytes(integer::<4>(data)?), &mut tm);
            0
        }
        SQL_TYPE_TIME => {
            isc_decode_sql_time(&u32::from_ne_bytes(integer::<4>(data)?), &mut tm);
            1
        }
        _ => {
            let date = i32::from_ne_bytes(integer::<4>(data)?);
            let time = u32::from_ne_bytes(integer::<4>(data.get(4..)?)?);
            let timestamp = ISC_TIMESTAMP {
                timestamp_date: date,
                timestamp_time: time,
            };
            isc_decode_timestamp(&timestamp, &mut tm);
            2
        }
    };
    let format = swr_firebird_format(dbh, which);
    let mut text = [0u8; 80];
    let len = swoole_rs_corebase::sys::strftime(&mut text, format.get(), &tm);
    Some(Owned::string(ZString::new(
        text.get(..len).unwrap_or_default(),
    )))
}

/// `pdo_firebird_stmt_get_col` for a null value, a scaled `SMALLINT`,
/// `INTEGER`, `BIGINT` or `DOUBLE`, a `VARCHAR` or `CHAR`, an unscaled integer,
/// a `BOOLEAN`, `FLOAT` and `DOUBLE`, and a `DATE`, `TIME` or `TIMESTAMP` in
/// the connection's format: the value in `result`, read from the lent `sqldata` bytes. False,
/// setting nothing, for the types C still reads.
#[unsafe(no_mangle)]
pub extern "C" fn swr_firebird_value<'a>(
    dbh: &Dbh,
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
            SQL_TYPE_DATE | SQL_TYPE_TIME | SQL_TIMESTAMP => datetime(dbh, var, data),
            _ => None,
        }
    };
    let Some(value) = value else {
        return false;
    };
    result.set(value);
    true
}

/// Firebird 4's type codes (`SQL_INT128`, `SQL_TIMESTAMP_TZ`, `SQL_TIME_TZ`,
/// `SQL_DEC16`, `SQL_DEC34`), which a Firebird 3 ibase.h lacks: the driver
/// coerces them only when built against a Firebird 4 one.
const SQL_INT128: u32 = 32752;
const SQL_TIMESTAMP_TZ: u32 = 32754;
const SQL_TIME_TZ: u32 = 32756;
const SQL_DEC16: u32 = 32760;
const SQL_DEC34: u32 = 32762;

/// `set_coercing_output_data_types` for one output variable: Firebird 4's
/// `INT128` (with its scale dropped), `DECFLOAT(16)` and `DECFLOAT(34)`, and
/// the time-zone types below a Firebird 4 client, fetched as `VARCHAR`s of
/// their longest text, the nullable bit kept.
#[unsafe(no_mangle)]
pub extern "C" fn swr_firebird_coerce(var: &mut XSQLVAR, client_major: c_uint) {
    let length = match base_type(var.sqltype) {
        SQL_INT128 => {
            var.sqlscale = 0;
            46
        }
        SQL_DEC16 => 24,
        SQL_DEC34 => 43,
        SQL_TIMESTAMP_TZ if client_major < 4 => 58,
        SQL_TIME_TZ if client_major < 4 => 46,
        _ => return,
    };
    var.sqltype = i16::try_from(SQL_VARYING).unwrap_or_default() | (var.sqltype & 1);
    var.sqllen = length;
}
