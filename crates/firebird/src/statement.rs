//! `pdo_firebird`'s statement methods (`firebird_statement.c`): so far the
//! column description, metadata and output type coercion, which read a
//! column's `XSQLVAR` in the statement's output descriptor, and the values
//! other than blobs and the time-zone types. The methods PDO calls stay C, as
//! thin callers that find the descriptor in their statement record.
use std::ffi::{c_char, c_int, c_uint};

use swoole_rs_corebase::lent::{Count, In, lend_in, lend_out};
use swoole_rs_corebase::sys::Text;
use swoole_rs_firebird_sys::ibase::{
    ISC_TIMESTAMP, SQL_BOOLEAN, SQL_DOUBLE, SQL_FLOAT, SQL_INT64, SQL_LONG, SQL_SHORT, SQL_TEXT,
    SQL_TIMESTAMP, SQL_TYPE_DATE, SQL_TYPE_TIME, SQL_VARYING, XSQLVAR,
};
use swoole_rs_firebird_sys::{
    Bytes, Handle, ISC_STATUS, Quad, Sqlda, Status,
    ibase::{
        DSQL_close, DSQL_drop, isc_info_blob_total_length, isc_info_end, isc_info_error,
        isc_info_req_select_count, isc_info_sql_records, isc_info_sql_stmt_delete,
        isc_info_sql_stmt_exec_procedure, isc_info_sql_stmt_insert, isc_info_sql_stmt_update,
        isc_info_truncated, isc_segment, isc_segstr_eof,
    },
};
use swoole_rs_zend::pdo::{Attribute, Dbh, Slot, Stmt};
use swoole_rs_zend::sys::{
    PHP_VERSION_ID, pdo_attribute_type_PDO_ATTR_CURSOR_NAME as CURSOR_NAME, zend_long, zend_ulong,
};

use crate::transaction::php_firebird_commit_transaction;
use crate::{
    swoole_isc_blob_info, swoole_isc_close_blob, swoole_isc_dsql_execute, swoole_isc_dsql_execute2,
    swoole_isc_dsql_fetch, swoole_isc_dsql_free_statement, swoole_isc_dsql_set_cursor_name,
    swoole_isc_dsql_sql_info, swoole_isc_get_segment, swoole_isc_open_blob,
};
use swoole_rs_zend::sys::{
    pdo_column_data, pdo_param_type_PDO_PARAM_BOOL as PARAM_BOOL,
    pdo_param_type_PDO_PARAM_INT as PARAM_INT, pdo_param_type_PDO_PARAM_STR as PARAM_STR,
};
use swoole_rs_zend::{Owned, OwnedArray, Value, ZString, value_error};

unsafe extern "C" {
    /// The connection's date (0), time (1) or timestamp (2) format, or its
    /// default, live until it is set again.
    safe fn swr_firebird_format<'a>(dbh: &'a Dbh, which: c_int) -> Text<'a>;
    /// The connection's status vector.
    safe fn swr_firebird_status(dbh: &Dbh) -> Status;
    /// The statement's connection (`stmt->dbh`) and `&S->stmt`.
    safe fn swr_firebird_stmt_dbh(stmt: &Stmt) -> &Dbh;
    safe fn swr_firebird_stmt_handle(stmt: &Stmt) -> Handle<'_>;
    /// The cursor's name (`S->name`), empty when it has none, live until the
    /// cursor is renamed or closed; whether its cursor is open
    /// (`S->cursor_open`); and both cleared.
    safe fn swr_firebird_cursor_name(stmt: &Stmt) -> Text<'_>;
    safe fn swr_firebird_cursor_open(stmt: &Stmt) -> bool;
    safe fn swr_firebird_cursor_closed(stmt: &Stmt);
    /// Whether the statement's rows are exhausted (`S->exhausted`), and
    /// marking them so; whether it is an `EXECUTE PROCEDURE`; and its output
    /// descriptor (`&S->out_sqlda`).
    safe fn swr_firebird_exhausted(stmt: &Stmt) -> bool;
    safe fn swr_firebird_set_exhausted(stmt: &Stmt);
    safe fn swr_firebird_is_procedure(stmt: &Stmt) -> bool;
    safe fn swr_firebird_out_sqlda(stmt: &Stmt) -> Sqlda;
    /// The statement's input descriptor (`S->in_sqlda`, null without
    /// parameters) and type (`S->statement_type`); the connection's
    /// transaction (`&H->tr`) and manual-transaction flag.
    safe fn swr_firebird_in_sqlda(stmt: &Stmt) -> Sqlda;
    safe fn swr_firebird_statement_type(stmt: &Stmt) -> c_char;
    safe fn swr_firebird_tr(dbh: &Dbh) -> Handle<'_>;
    /// The connection's database (`&H->db`).
    safe fn swr_firebird_db(dbh: &Dbh) -> Handle<'_>;
    safe fn swr_firebird_in_manual_txn(dbh: &Dbh) -> bool;
    /// `S->cursor_open = 0`.
    safe fn swr_firebird_cursor_reset(stmt: &Stmt);
    /// Each output variable's indicator and data storage (re)allocated
    /// (`ecalloc` of its length and two shorts).
    safe fn swr_firebird_alloc_output(stmt: &Stmt);
    /// After an execute: the name cleared, the cursor open when the statement
    /// returns rows but is no procedure, exhausted when it returns none.
    safe fn swr_firebird_executed(stmt: &Stmt);
    /// Whether the statement's connection object is still usable (the server
    /// side of the statement is gone with it otherwise).
    safe fn swr_firebird_db_usable(stmt: &Stmt) -> bool;
    /// The statement record freed: its named parameters, descriptors' storage
    /// and itself. The statement's accessors may not be called again.
    safe fn swr_firebird_stmt_free(stmt: &Stmt);
    /// The cursor's name set to `name`'s first 31 bytes (`strlcpy`).
    safe fn swr_firebird_cursor_named(stmt: &Stmt, name: Text<'_>);
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

/// `pdo_firebird_stmt_get_attribute`: the cursor's name, null when it has none.
#[unsafe(no_mangle)]
pub extern "C" fn pdo_firebird_stmt_get_attribute(
    stmt: &Stmt,
    attr: zend_long,
    val: &Slot,
) -> Attribute {
    if u32::try_from(attr) != Ok(CURSOR_NAME) {
        return Attribute::Unknown;
    }
    let name = swr_firebird_cursor_name(stmt);
    let name = name.get().to_bytes();
    val.set(if name.is_empty() {
        Owned::null()
    } else {
        Owned::string(ZString::new(name))
    });
    Attribute::Set
}

/// `pdo_firebird_stmt_cursor_closer`: a named or open cursor closed
/// (`DSQL_close`), its name and open flag cleared; false with the error
/// recorded on the statement.
#[unsafe(no_mangle)]
pub extern "C" fn swr_firebird_cursor_close(stmt: &Stmt) -> bool {
    let dbh = swr_firebird_stmt_dbh(stmt);
    let open = !swr_firebird_cursor_name(stmt).get().is_empty() || swr_firebird_cursor_open(stmt);
    let close = u16::try_from(DSQL_close).unwrap_or_default();
    if open
        && swoole_isc_dsql_free_statement(
            swr_firebird_status(dbh),
            swr_firebird_stmt_handle(stmt),
            close,
        ) != 0
    {
        crate::error::status_error(dbh, Some(stmt));
        return false;
    }
    swr_firebird_cursor_closed(stmt);
    true
}

/// The longest cursor name the statement keeps (`sizeof(S->name) - 1`).
const CURSOR_NAME_MAX: usize = 31;

/// `pdo_firebird_stmt_set_attribute`: `PDO_ATTR_CURSOR_NAME` names the
/// statement's cursor, a `null` value as the empty name. From PHP 8.5 a name of
/// 32 bytes or more is a `ValueError`; before, it is kept to its first 31. False for any other
/// attribute, or with the error recorded on the statement.
#[unsafe(no_mangle)]
pub extern "C" fn swr_firebird_stmt_set_attribute(
    stmt: &Stmt,
    attr: zend_long,
    val: Option<&Value>,
) -> bool {
    if u32::try_from(attr) != Ok(CURSOR_NAME) {
        return false;
    }
    // PDO passes no value for `null`, whose string is the empty name.
    let name = match val {
        Some(val) => match val.try_string() {
            Some(name) => name,
            None => return false,
        },
        None => ZString::empty(),
    };
    if PHP_VERSION_ID >= 80500 && name.bytes().len() > CURSOR_NAME_MAX {
        value_error(c"Cursor name must not be longer than 31 bytes");
        return false;
    }
    let dbh = swr_firebird_stmt_dbh(stmt);
    let text = name.c_str();
    if swoole_isc_dsql_set_cursor_name(
        swr_firebird_status(dbh),
        swr_firebird_stmt_handle(stmt),
        Bytes::from_c_str(text),
        0,
    ) != 0
    {
        crate::error::status_error(dbh, Some(stmt));
        return false;
    }
    swr_firebird_cursor_named(stmt, text.into());
    true
}

/// `isc_vax_integer`: a little-endian integer of 1 to 4 bytes, its last byte
/// signed, sign-extended to the unsigned 64 bits C's unsigned uses of it read.
fn vax(bytes: &[u8]) -> u64 {
    let Some((&last, rest)) = bytes.split_last() else {
        return 0;
    };
    let value = rest
        .iter()
        .rev()
        .fold(i64::from(last.cast_signed()), |value, &byte| {
            (value << 8) | i64::from(byte)
        });
    value.cast_unsigned()
}

/// `pdo_firebird_stmt_execute`'s count of the rows an `INSERT`, `UPDATE`,
/// `DELETE` or procedure changed, from the `isc_info_sql_records` reply:
/// each count but the selected rows' added to `rows`, at the `zend_ulong`'s
/// width. False for a reply whose size, an item's length or an item's bytes run
/// past it or are malformed, as C's `goto error`.
fn records(reply: &[u8], rows: &mut zend_ulong) -> bool {
    // C's `unsigned result_size = isc_vax_integer(&result[1], 2)`.
    let size = reply.get(1..3).map_or(0, vax);
    if usize::try_from(size).map_or(true, |size| size > reply.len()) {
        return false;
    }
    let end = u8::try_from(isc_info_end).unwrap_or_default();
    let selected = u8::try_from(isc_info_req_select_count).unwrap_or_default();
    let mut at = 3;
    while let Some(&item) = reply.get(at)
        && item != end
        && u64::try_from(at).is_ok_and(|at| at < size)
    {
        let Some(length) = reply.get(at + 1..at + 3).map(vax) else {
            return false;
        };
        let Ok(length @ (1 | 2 | 4)) = usize::try_from(length) else {
            return false;
        };
        let Some(count) = reply.get(at + 3..at + 3 + length) else {
            return false;
        };
        if item != selected {
            // C's zend_ulong `+=` of the ISC_LONG: its low bytes at the
            // zend_ulong's width.
            let wide = vax(count).to_le_bytes();
            let count = wide
                .get(..size_of::<zend_ulong>())
                .and_then(|low| low.try_into().ok())
                .map_or(0, zend_ulong::from_le_bytes);
            *rows = rows.wrapping_add(count);
        }
        at += length + 3;
    }
    true
}

/// The size of `php_firebird_fetch_blob`'s info reply (`bl_info`).
const BLOB_INFO: usize = 20;

/// The length the blob info `reply` gives as its total: None, C's "Couldn't
/// determine BLOB size", when an end, truncation or error item comes first or
/// the reply runs out; zero when the search runs past the reply's items, as
/// C's loop fell through.
fn blob_length(reply: &[u8; BLOB_INFO]) -> Option<u64> {
    let ends = [isc_info_end, isc_info_truncated, isc_info_error];
    // An `unsigned short`, as C's index was.
    let mut i: u16 = 0;
    while usize::from(i) < BLOB_INFO {
        let item = u32::from(reply.get(usize::from(i)).copied().unwrap_or_default());
        i += 1;
        let at = usize::from(i);
        if ends.contains(&item) || at >= BLOB_INFO {
            return None;
        }
        let item_len =
            vax(reply.get(at..(at + 2).min(BLOB_INFO)).unwrap_or_default()).to_ne_bytes();
        let item_len = u16::from_ne_bytes([item_len[0], item_len[1]]);
        if item == isc_info_blob_total_length {
            // isc_vax_integer reads 1 to 4 bytes, and is 0 for any other length.
            if !(1..=4).contains(&item_len) {
                return Some(0);
            }
            let start = (at + 2).min(BLOB_INFO);
            let end = (start + usize::from(item_len)).min(BLOB_INFO);
            return Some(vax(reply.get(start..end).unwrap_or_default()));
        }
        i = i.wrapping_add(item_len).wrapping_add(2);
    }
    Some(0)
}

/// `isc_segment` and `isc_segstr_eof`, the statuses a segment read ends a
/// blob's reading with.
fn segment_status(code: u32) -> ISC_STATUS {
    ISC_STATUS::try_from(code).unwrap_or_default()
}

/// The open blob's data read into `result`: its total length from the blob's
/// info, then its segments. As in C, a failed segment read still sets the
/// value, then records the error.
fn read_blob(dbh: &Dbh, stmt: &Stmt, blob: Handle<'_>, result: &Slot) -> bool {
    let status = swr_firebird_status(dbh);
    let item =
        c_char::from_ne_bytes([u8::try_from(isc_info_blob_total_length).unwrap_or_default()]);
    let mut reply = [0u8; BLOB_INFO];
    let failed = lend_in(&[item], |items, items_length| {
        lend_out(&mut reply, |out, len| {
            swoole_isc_blob_info(status, blob, items_length, items, len, out.chars())
        })
    }) != 0;
    if failed {
        crate::error::status_error(dbh, Some(stmt));
        return false;
    }
    let Some(len) = blob_length(&reply) else {
        crate::error::stmt_message(dbh, stmt, b"Couldn't determine BLOB size");
        return false;
    };
    if len == 0 {
        return true;
    }
    // Past what a string can hold (C's ZSTR_MAX_LEN) or memory: false, nothing recorded.
    let Ok(len) = usize::try_from(len) else {
        return false;
    };
    let mut data = Vec::new();
    if data.try_reserve_exact(len).is_err() {
        return false;
    }
    data.resize(len, 0u8);
    let (segment, eof) = (segment_status(isc_segment), segment_status(isc_segstr_eof));
    let (mut read, mut stat) = (0usize, 0);
    while (stat == 0 || stat == segment) && read < len {
        let chunk = (len - read).min(usize::from(u16::MAX));
        let mut segment_len = 0;
        stat = lend_out(
            data.get_mut(read..read + chunk).unwrap_or_default(),
            |out, count| swoole_isc_get_segment(status, blob, &mut segment_len, count, out.chars()),
        );
        read += usize::from(segment_len);
    }
    result.set(Owned::string(ZString::new(&data)));
    let vector = crate::error::swr_firebird_status_vector(dbh);
    if vector.first() == Some(&1) && stat != 0 && stat != eof && stat != segment {
        crate::error::stmt_message(dbh, stmt, b"Error reading from BLOB");
        return false;
    }
    true
}

/// `php_firebird_fetch_blob`: the `BLOB` whose id is the lent `id` bytes (an
/// `ISC_QUAD`) opened, read into `result` and closed, each failure recorded on
/// the statement.
#[unsafe(no_mangle)]
pub extern "C" fn swr_firebird_fetch_blob<'a>(
    stmt: &Stmt,
    id: In<'a, u8>,
    id_len: Count<'a>,
    result: &Slot,
) -> bool {
    let dbh = swr_firebird_stmt_dbh(stmt);
    let status = swr_firebird_status(dbh);
    let id = id.drain(id_len).bytes();
    let word = |at: usize| {
        id.get(at..at + 4)
            .and_then(|bytes| bytes.try_into().ok())
            .map_or(0, u32::from_ne_bytes)
    };
    let mut quad = [word(0), word(4)];
    let mut handle = 0;
    let blob = Handle::new(&mut handle);
    let opened = swoole_isc_open_blob(
        status,
        swr_firebird_db(dbh),
        swr_firebird_tr(dbh),
        blob,
        Quad::new(&mut quad),
    );
    if opened != 0 {
        crate::error::status_error(dbh, Some(stmt));
        return false;
    }
    let read = read_blob(dbh, stmt, blob, result);
    if swoole_isc_close_blob(status, blob) != 0 {
        crate::error::status_error(dbh, Some(stmt));
        return false;
    }
    read
}

/// `PDO_FB_SQLDA_VERSION`: `SQLDA_VERSION1`, as ibase.h defines no `SQLDA_VERSION`.
const SQLDA_VERSION: u16 = 1;

/// `pdo_firebird_stmt_fetch`: the next row into the output descriptor,
/// counted. Before execution, the `HY000` closed-cursor error; an `EXECUTE
/// PROCEDURE`'s one row, which `execute` already fetched; false once
/// exhausted, the fetch's error recorded when it left one.
#[unsafe(no_mangle)]
pub extern "C" fn swr_firebird_fetch(stmt: &Stmt) -> bool {
    let dbh = swr_firebird_stmt_dbh(stmt);
    if !stmt.executed() {
        swoole_rs_corebase::lent::lend_in(b"HY000", |state, state_len| {
            swoole_rs_corebase::lent::lend_in(
                b"Cannot fetch from a closed cursor",
                |message, message_len| {
                    crate::error::php_firebird_set_error(
                        dbh,
                        Some(stmt),
                        state,
                        state_len,
                        message,
                        message_len,
                    );
                },
            );
        });
        return false;
    }
    if swr_firebird_exhausted(stmt) {
        return false;
    }
    if swr_firebird_is_procedure(stmt) {
        stmt.set_row_count(1);
        swr_firebird_set_exhausted(stmt);
        return true;
    }
    let fetched = swoole_isc_dsql_fetch(
        swr_firebird_status(dbh),
        swr_firebird_stmt_handle(stmt),
        SQLDA_VERSION,
        swr_firebird_out_sqlda(stmt),
    );
    if fetched != 0 {
        if crate::error::status_failed(dbh) {
            crate::error::status_error(dbh, Some(stmt));
        }
        swr_firebird_set_exhausted(stmt);
        return false;
    }
    stmt.set_row_count(stmt.row_count().wrapping_add(1));
    true
}

/// `pdo_firebird_stmt_execute`: a named or open cursor closed, the output
/// storage allocated, and the statement executed in the connection's
/// transaction (an `EXECUTE PROCEDURE`'s outputs into the output descriptor),
/// the rows an `INSERT`, `UPDATE`, `DELETE` or procedure changed counted, and
/// in autocommit mode the transaction committed and kept. False with the
/// error recorded on the statement.
#[unsafe(no_mangle)]
pub extern "C" fn swr_firebird_execute(stmt: &Stmt) -> bool {
    let dbh = swr_firebird_stmt_dbh(stmt);
    let executed = execute(dbh, stmt);
    if !executed {
        crate::error::status_error(dbh, Some(stmt));
    }
    executed
}

/// [`swr_firebird_execute`] but for recording its failure.
fn execute(dbh: &Dbh, stmt: &Stmt) -> bool {
    let (status, handle) = (swr_firebird_status(dbh), swr_firebird_stmt_handle(stmt));
    let named = !swr_firebird_cursor_name(stmt).get().is_empty();
    let close = u16::try_from(DSQL_close).unwrap_or_default();
    if (named || swr_firebird_cursor_open(stmt))
        && swoole_isc_dsql_free_statement(status, handle, close) != 0
    {
        return false;
    }
    swr_firebird_cursor_reset(stmt);
    swr_firebird_alloc_output(stmt);
    let kind = u32::from(swr_firebird_statement_type(stmt).cast_unsigned());
    let tr = swr_firebird_tr(dbh);
    let failed = if kind == isc_info_sql_stmt_exec_procedure {
        swoole_isc_dsql_execute2(
            status,
            tr,
            handle,
            SQLDA_VERSION,
            swr_firebird_in_sqlda(stmt),
            swr_firebird_out_sqlda(stmt),
        )
    } else {
        swoole_isc_dsql_execute(
            status,
            tr,
            handle,
            SQLDA_VERSION,
            swr_firebird_in_sqlda(stmt),
        )
    };
    if failed != 0 {
        return false;
    }
    stmt.set_row_count(0);
    let changes = [
        isc_info_sql_stmt_insert,
        isc_info_sql_stmt_update,
        isc_info_sql_stmt_delete,
        isc_info_sql_stmt_exec_procedure,
    ];
    if changes.contains(&kind) {
        let mut reply = [0u8; 64];
        // A failed info call leaves the count at 0, as C's `break` from its switch.
        // The one item asked for: `isc_info_sql_records`.
        let item = c_char::from_ne_bytes([u8::try_from(isc_info_sql_records).unwrap_or_default()]);
        let answered = lend_in(&[item], |items, items_length| {
            lend_out(&mut reply, |out, len| {
                swoole_isc_dsql_sql_info(
                    swr_firebird_status(dbh),
                    swr_firebird_stmt_handle(stmt),
                    items_length,
                    items,
                    len,
                    out.chars(),
                ) == 0
            })
        });
        if answered
            && reply
                .first()
                .is_some_and(|&item| u32::from(item) == isc_info_sql_records)
        {
            let mut rows: zend_ulong = 0;
            if !records(&reply, &mut rows) {
                return false;
            }
            stmt.set_row_count(rows.cast_signed());
        }
    }
    if dbh.auto_commit()
        && !swr_firebird_in_manual_txn(dbh)
        && !php_firebird_commit_transaction(dbh, true)
    {
        return false;
    }
    swr_firebird_executed(stmt);
    true
}

/// `pdo_firebird_stmt_dtor`: the statement dropped on the server
/// (`DSQL_drop`) while its connection object is usable, false with the error
/// recorded when that fails, and the record freed either way.
#[unsafe(no_mangle)]
pub extern "C" fn swr_firebird_stmt_dtor(stmt: &Stmt) -> bool {
    let dbh = swr_firebird_stmt_dbh(stmt);
    let drop = u16::try_from(DSQL_drop).unwrap_or_default();
    let failed = swr_firebird_db_usable(stmt)
        && swoole_isc_dsql_free_statement(
            swr_firebird_status(dbh),
            swr_firebird_stmt_handle(stmt),
            drop,
        ) != 0;
    if failed {
        crate::error::status_error(dbh, Some(stmt));
    }
    swr_firebird_stmt_free(stmt);
    !failed
}
