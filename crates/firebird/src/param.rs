//! `pdo_firebird_stmt_param_hook` (`firebird_statement.c`): a bound
//! parameter's or column's index resolved, a parameter's storage allocated
//! and its value bound into its input variable before each execute, and a
//! bound column's value fetched. The statement record and the variables'
//! storage stay C. Once the index is resolved, Rust holds no reference into
//! the descriptor: binding can run PHP code (a stream's read, `__toString`, an
//! error handler) that re-enters the hook, so each read or write of the input
//! variable goes through a `swr_firebird_input_*` accessor, which finds it anew.
use std::cell::Cell;
use std::ffi::{c_char, c_int, c_short};

use swoole_rs_corebase::CCast;
use swoole_rs_corebase::lent::{Count, In, InOut, lend_in};
use swoole_rs_firebird_sys::ibase::{
    SQL_ARRAY, SQL_BLOB, SQL_BOOLEAN, SQL_DOUBLE, SQL_FLOAT, SQL_INT64, SQL_LONG, SQL_SHORT,
    SQL_TEXT, SQL_TIMESTAMP, SQL_TYPE_DATE, SQL_TYPE_TIME, XSQLVAR,
};
use swoole_rs_zend::pdo::{Dbh, Slot, Stmt};
use swoole_rs_zend::sys::{
    IS_DOUBLE, IS_FALSE, IS_LONG, IS_NULL, IS_RESOURCE, IS_STRING, IS_TRUE, pdo_param_event,
    pdo_param_event_PDO_PARAM_EVT_ALLOC as EVT_ALLOC,
    pdo_param_event_PDO_PARAM_EVT_EXEC_PRE as EVT_EXEC_PRE,
    pdo_param_event_PDO_PARAM_EVT_FETCH_POST as EVT_FETCH_POST,
    pdo_param_event_PDO_PARAM_EVT_FREE as EVT_FREE, zend_long,
};
use swoole_rs_zend::{Array, Owned, Value, ZStr};

use crate::error::stmt_state_message;
#[cfg(fb_api_40)]
use crate::statement::{SQL_DEC16, SQL_DEC34, SQL_INT128, SQL_TIME_TZ, SQL_TIMESTAMP_TZ};
use crate::statement::{base_type, swr_firebird_bind_blob, swr_firebird_stmt_dbh};

unsafe extern "C" {
    /// The statement's named parameters (`S->named_params`): each name the
    /// preprocessor replaced, with its index.
    safe fn swr_firebird_named_params(stmt: &Stmt) -> &Array;
    /// Input variable `index`'s type (`sqltype`, with its nullable bit), or 0
    /// past the input descriptor.
    safe fn swr_firebird_input_type(stmt: &Stmt, index: zend_long) -> c_short;
    /// Input variable `index`'s indicator and data storage (re)allocated
    /// (`emalloc` of its length and two shorts).
    safe fn swr_firebird_input_alloc(stmt: &Stmt, index: zend_long);
    /// Input variable `index`'s indicator set: -1 for NULL, else 0.
    safe fn swr_firebird_input_null(stmt: &Stmt, index: zend_long, null: bool);
    /// The lent bytes copied to input variable `index`'s data when they fit
    /// its length.
    safe fn swr_firebird_input_data<'a>(
        stmt: &Stmt,
        index: zend_long,
        data: In<'a, u8>,
        len: Count<'a>,
    );
    /// Input variable `index` retyped to `sqltype` (its nullable bit kept) and
    /// pointed at where `value` holds its long, double or string, with that
    /// length.
    safe fn swr_firebird_input_point(
        stmt: &Stmt,
        index: zend_long,
        sqltype: c_short,
        value: &Value,
    );
    /// A stream parameter read into a string, which replaces it in `value`
    /// (`php_stream_copy_to_mem`); false, with `HY105` raised, for a resource
    /// that is no stream.
    safe fn swr_firebird_param_stream(stmt: &Stmt, value: &Slot) -> bool;
    /// `pdo_firebird_stmt_get_col`: column `colno`'s value into `result`.
    safe fn pdo_firebird_stmt_get_col(
        stmt: &Stmt,
        colno: c_int,
        result: &Slot,
        kind: Option<&mut c_int>,
    ) -> c_int;
}

/// `strncasecmp(name, field, n) == 0` of a parameter's name and a variable's
/// 32-byte name field: equal, ASCII case aside, up to `n` bytes or a NUL.
fn name_matches(name: &[u8], field: &[c_char; 32], length: c_short) -> bool {
    if length == 0 {
        return false;
    }
    // C's `min(ZSTR_LEN(name), length)`.
    let n = name
        .len()
        .min(usize::try_from(length).unwrap_or(usize::MAX));
    for i in 0..n {
        let a = name.get(i).copied().unwrap_or(0);
        let b = field.get(i).map_or(0, |c| c.to_ne_bytes()[0]);
        if !a.eq_ignore_ascii_case(&b) {
            return false;
        }
        if a == 0 {
            break;
        }
    }
    true
}

/// A named parameter's index: the one the preprocessor recorded for `name`,
/// else the first input variable whose alias or name matches it.
fn named_index(stmt: &Stmt, vars: &[XSQLVAR], name: &ZStr) -> Option<zend_long> {
    if let Some(index) = swr_firebird_named_params(stmt).find(name.bytes()) {
        // C's `Z_LVAL_P`: the preprocessor recorded an int.
        return Some(index.as_long().unwrap_or_default());
    }
    vars.iter()
        .position(|var| {
            name_matches(name.bytes(), &var.aliasname, var.aliasname_length)
                || name_matches(name.bytes(), &var.sqlname, var.sqlname_length)
        })
        .map(CCast::c_cast)
}

/// The types an empty string binds as NULL to.
fn empty_is_null(sqltype: u32) -> bool {
    #[cfg(fb_api_40)]
    if matches!(
        sqltype,
        SQL_INT128 | SQL_DEC16 | SQL_DEC34 | SQL_TIMESTAMP_TZ | SQL_TIME_TZ
    ) {
        return true;
    }
    matches!(
        sqltype,
        SQL_SHORT
            | SQL_LONG
            | SQL_INT64
            | SQL_FLOAT
            | SQL_DOUBLE
            | SQL_TIMESTAMP
            | SQL_TYPE_DATE
            | SQL_TYPE_TIME
    )
}

/// NULL bound, or `HY105` recorded when the variable takes no NULL.
fn bind_null(dbh: &Dbh, stmt: &Stmt, index: zend_long, sqltype: c_short) -> bool {
    if sqltype & 1 == 0 {
        stmt_state_message(dbh, stmt, b"HY105", b"Parameter requires non-null value");
        return false;
    }
    swr_firebird_input_null(stmt, index, true);
    true
}

/// A `BOOLEAN` variable's value: a number or bool's truth; a string's numeric
/// truth, or `true`/`false` by its first bytes (an empty string is false).
/// None, with the error recorded, for anything else.
fn boolean(dbh: &Dbh, stmt: &Stmt, value: &Value) -> Option<bool> {
    let text = match value.kind() {
        IS_LONG | IS_DOUBLE | IS_TRUE | IS_FALSE => return Some(value.truthy()),
        IS_STRING => value.str().map_or(&[][..], ZStr::bytes),
        _ => {
            stmt_state_message(
                dbh,
                stmt,
                b"HY105",
                b"Binding arrays/objects is not supported",
            );
            return None;
        }
    };
    if text.is_empty() {
        return Some(false);
    }
    // `zend_binary_strncasecmp(text, len, word, n, n) == 0`: the first `n`
    // bytes are the word.
    let starts = |word: &[u8]| {
        text.get(..word.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(word))
    };
    match swoole_rs_zend::numeric(text) {
        (kind, long, _) if u32::from(kind) == IS_LONG => Some(long != 0),
        (kind, _, double) if u32::from(kind) == IS_DOUBLE => Some(double != 0.0),
        _ if starts(b"true") => Some(true),
        _ if starts(b"false") => Some(false),
        _ => {
            stmt_state_message(dbh, stmt, b"HY105", b"Cannot convert string to boolean");
            None
        }
    }
}

/// `PDO_PARAM_EVT_EXEC_PRE` for input variable `index`: `value` bound to it.
/// Its type is read after each step that can run PHP code, as C read it.
fn bind(dbh: &Dbh, stmt: &Stmt, index: zend_long, value: &Slot) -> bool {
    swr_firebird_input_null(stmt, index, false);
    if value.value().kind() == IS_RESOURCE {
        let mut read = false;
        if let Err(bailout) =
            swoole_rs_zend::contain_with(|| read = swr_firebird_param_stream(stmt, value))
        {
            bailout.defer();
            return false;
        }
        if !read {
            return false;
        }
    }
    let value = value.value();
    let sqltype = swr_firebird_input_type(stmt, index);
    match base_type(sqltype) {
        SQL_ARRAY => {
            stmt_state_message(dbh, stmt, b"HY000", b"Cannot bind to array field");
            return false;
        }
        SQL_BLOB if value.kind() == IS_NULL => return bind_null(dbh, stmt, index, sqltype),
        SQL_BLOB => {
            let mut id = [0; 2];
            if !swr_firebird_bind_blob(stmt, &mut id, value) {
                return false;
            }
            let bytes = [id[0].to_ne_bytes(), id[1].to_ne_bytes()].concat();
            lend_in(&bytes, |data, len| {
                swr_firebird_input_data(stmt, index, data, len);
            });
            return true;
        }
        SQL_BOOLEAN if value.kind() == IS_NULL => {
            swr_firebird_input_null(stmt, index, true);
            return true;
        }
        SQL_BOOLEAN => {
            let Some(truth) = boolean(dbh, stmt, value) else {
                return false;
            };
            // FB_TRUE or FB_FALSE.
            lend_in(&[u8::from(truth)], |data, len| {
                swr_firebird_input_data(stmt, index, data, len);
            });
            return true;
        }
        _ => {}
    }
    // The variable reads the value where the zval holds it.
    let retyped = match value.kind() {
        // C's `sizeof(zend_long) == 8 ? SQL_INT64 : SQL_LONG`.
        IS_LONG if cfg!(target_pointer_width = "64") => SQL_INT64,
        IS_LONG => SQL_LONG,
        IS_DOUBLE => SQL_DOUBLE,
        IS_STRING => {
            let len = value.str().map_or(0, |text| text.bytes().len());
            if c_short::try_from(len).is_err() {
                stmt_state_message(dbh, stmt, b"22001", b"String data right truncation");
                return false;
            }
            if len == 0 && empty_is_null(base_type(sqltype)) {
                return bind_null(dbh, stmt, index, sqltype);
            }
            SQL_TEXT
        }
        IS_NULL => return bind_null(dbh, stmt, index, sqltype),
        _ => {
            stmt_state_message(
                dbh,
                stmt,
                b"HY105",
                b"Binding arrays/objects is not supported",
            );
            return false;
        }
    };
    swr_firebird_input_point(stmt, index, retyped.c_cast(), value);
    true
}

swoole_rs_zend::exports! {
/// `pdo_firebird_stmt_param_hook`, given the variables of the statement's
/// input descriptor (for a parameter) or output one (a column): a `count` of -1
/// when it has none. A named parameter's index (-1 in `paramno`) is resolved
/// first. Its storage is allocated, and its `value` (dereferenced) bound
/// before each execute; a bound column's `value` is fetched after each row.
/// False with the error recorded on the statement.
#[expect(clippy::too_many_arguments, reason = "PDO's bound-parameter record, field by field")]
#[unsafe(no_mangle)]
pub extern "C" fn swr_firebird_param_hook<'v>(stmt: &Stmt, vars: InOut<'v, XSQLVAR>,
    count: Count<'v, c_short>, event: pdo_param_event, is_param: bool, name: Option<&ZStr>,
    paramno: &Cell<zend_long>, value: &Slot) -> bool {
    if event == EVT_FREE {
        return true;
    }
    let dbh = swr_firebird_stmt_dbh(stmt);
    let described = count.get() >= 0;
    // Read only here, before anything can run PHP code.
    let Some(vars) = vars.items(count) else {
        return false;
    };
    let vars: &[XSQLVAR] = vars;
    if !described || usize::try_from(paramno.get()).is_ok_and(|index| index >= vars.len()) {
        stmt_state_message(dbh, stmt, b"HY093", b"Invalid parameter index");
        return false;
    }
    if is_param && paramno.get() == -1 {
        let Some(index) = name.and_then(|name| named_index(stmt, vars, name)) else {
            stmt_state_message(dbh, stmt, b"HY093", b"Invalid parameter name");
            return false;
        };
        paramno.set(index);
    }
    let index = paramno.get();
    match event {
        EVT_ALLOC if is_param => swr_firebird_input_alloc(stmt, index),
        EVT_EXEC_PRE if is_param => return bind(dbh, stmt, index, value),
        EVT_FETCH_POST if index == -1 => return false,
        EVT_FETCH_POST if !is_param => {
            value.set(Owned::null());
            return pdo_firebird_stmt_get_col(stmt, index.c_cast(), value, None) != 0;
        }
        _ => {}
    }
    true
}
}
