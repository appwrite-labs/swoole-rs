//! `pdo_firebird`'s connection methods (`firebird_driver.c`), under their C
//! names: `firebird_handle_closer`, `firebird_handle_doer`, `pdo_firebird_check_liveness`,
//! `pdo_firebird_fetch_error_func`, `pdo_firebird_in_manually_transaction`,
//! and the handle factory's connecting (`swr_firebird_connect`). The
//! connection record stays C, read through the driver's `swr_firebird_*`
//! accessors.
use std::ffi::{CString, c_char, c_int, c_short, c_ushort};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::lent::{Count, Out, lend_in, lend_out};
use swoole_rs_corebase::sys::Text;
use swoole_rs_firebird_sys::ibase::{
    DSQL_drop, isc_dpb_lc_ctype, isc_dpb_password, isc_dpb_sql_role_name, isc_dpb_user_name,
    isc_dpb_version1, isc_info_sql_records,
};
use swoole_rs_firebird_sys::{FB_API_HANDLE, Handle, NameLength, Sqlda, Status};
use swoole_rs_zend::pdo::{Dbh, Slot, attr_long};
use swoole_rs_zend::sys::{zend_long, zend_result, zend_ulong};
use swoole_rs_zend::{Value, ZStr};

use crate::attribute::{ISOLATION_LEVEL, READ_COMMITTED, REPEATABLE_READ, SERIALIZABLE, WRITABLE};
use crate::error::status_error;
use crate::prepare::php_firebird_alloc_prepare_stmt;
use crate::statement::{SQLDA_VERSION, records};
use crate::transaction::php_firebird_begin_transaction;
use crate::{
    swoole_isc_attach_database, swoole_isc_dsql_execute, swoole_isc_dsql_free_statement,
    swoole_isc_dsql_sql_info,
};

use crate::transaction::{php_firebird_commit_transaction, php_firebird_rollback_transaction};
use crate::{swoole_fb_ping, swoole_isc_detach_database};

unsafe extern "C" {
    /// The connection's status vector and `&H->db`.
    safe fn swr_firebird_status(dbh: &Dbh) -> Status;
    safe fn swr_firebird_db(dbh: &Dbh) -> Handle<'_>;
    safe fn swr_firebird_tr(dbh: &Dbh) -> Handle<'_>;
    /// `H->in_manually_txn`, and setting it.
    safe fn swr_firebird_in_manual_txn(dbh: &Dbh) -> bool;
    safe fn swr_firebird_set_manual(dbh: &Dbh, manual: bool);
    /// Whether a transaction is open (`H->tr`), and whether the database is
    /// attached (`H->db`).
    safe fn swr_firebird_in_transaction(dbh: &Dbh) -> bool;
    safe fn swr_firebird_attached(dbh: &Dbh) -> bool;
    /// `php_firebird_error(dbh)`: the status vector's error recorded on the
    /// connection.
    safe fn swr_firebird_error(dbh: &Dbh);
    /// The connection record freed with its format strings and error message;
    /// the accessors may not be called again.
    safe fn swr_firebird_free(dbh: &Dbh);
    /// The connection's last error (`H->einfo`): its SQL code in `sqlcode`,
    /// and up to the lent bytes of its message, whose length it returns.
    safe fn swr_firebird_set_writable(dbh: &Dbh, writable: bool);
    safe fn swr_firebird_set_isolation(dbh: &Dbh, level: std::ffi::c_ulong);
    /// `H->sql_dialect = dialect`, a value its two bits hold.
    safe fn swr_firebird_set_dialect(dbh: &Dbh, dialect: c_ushort);
    /// The attached connection's PDO methods, upper-case column names, and
    /// columns the driver allocates.
    safe fn swr_firebird_set_methods(dbh: &Dbh);
    safe fn swr_firebird_last_error<'a>(
        dbh: &Dbh,
        sqlcode: &mut c_int,
        message: Out<'a, u8>,
        len: Count<'a>,
    ) -> usize;
}

/// `IS_NULL`, which C compared the SQL code with: a code of 1 is not reported.
const IS_NULL: c_int = 1;
/// `SUCCESS` and `FAILURE`.
const SUCCESS: zend_result = 0;
const FAILURE: zend_result = -1;
/// The longest message the error record holds: `php_firebird_set_error`'s
/// 512-byte buffer.
const MESSAGE_MAX: usize = 512;

/// `firebird_handle_closer`: an open transaction committed in autocommit mode,
/// else rolled back, the database detached, and the connection record freed.
/// A failure is recorded on the connection and the closing goes on.
#[unsafe(no_mangle)]
pub extern "C" fn firebird_handle_closer(dbh: &Dbh) {
    if swr_firebird_in_transaction(dbh) {
        // Each records its own failure; C went on regardless.
        let _ended = if dbh.auto_commit() {
            php_firebird_commit_transaction(dbh, false)
        } else {
            php_firebird_rollback_transaction(dbh)
        };
    }
    swr_firebird_set_manual(dbh, false);
    if swr_firebird_attached(dbh)
        && swoole_isc_detach_database(swr_firebird_status(dbh), swr_firebird_db(dbh)) != 0
    {
        swr_firebird_error(dbh);
    }
    swr_firebird_free(dbh);
}

/// `firebird_handle_doer` (`PDO::exec`): `sql` prepared and executed in the
/// connection's transaction, which autocommit mode then commits, and the
/// statement dropped. It keeps no result, so it takes no descriptor area.
/// The rows changed, or -1 with the error recorded on the connection.
#[unsafe(no_mangle)]
pub extern "C" fn firebird_handle_doer(dbh: &Dbh, sql: &ZStr) -> zend_long {
    let mut handle: FB_API_HANDLE = 0;
    if !php_firebird_alloc_prepare_stmt(dbh, sql, Sqlda::NONE, Handle::new(&mut handle), None) {
        return -1;
    }
    let rows = exec(dbh, &mut handle);
    let drop = u16::try_from(DSQL_drop).unwrap_or_default();
    if swoole_isc_dsql_free_statement(swr_firebird_status(dbh), Handle::new(&mut handle), drop) != 0
    {
        status_error(dbh, None);
    }
    rows.unwrap_or(-1)
}

/// [`firebird_handle_doer`] between the preparing and the dropping: the rows
/// the prepared statement changed, an `int`'s worth as C summed them.
fn exec(dbh: &Dbh, handle: &mut FB_API_HANDLE) -> Option<zend_long> {
    let status = swr_firebird_status(dbh);
    let tr = swr_firebird_tr(dbh);
    if swoole_isc_dsql_execute(status, tr, Handle::new(handle), SQLDA_VERSION, Sqlda::NONE) != 0 {
        status_error(dbh, None);
        return None;
    }
    let mut reply = [0u8; 64];
    // The one item asked for: `isc_info_sql_records`.
    let item = c_char::from_ne_bytes([u8::try_from(isc_info_sql_records).unwrap_or_default()]);
    let failed = lend_in(&[item], |items, items_length| {
        lend_out(&mut reply, |out, len| {
            swoole_isc_dsql_sql_info(
                status,
                Handle::new(handle),
                items_length,
                items,
                len,
                out.chars(),
            )
        })
    }) != 0;
    if failed {
        status_error(dbh, None);
        return None;
    }
    let mut rows: zend_ulong = 0;
    if reply
        .first()
        .is_some_and(|&item| u32::from(item) == isc_info_sql_records)
        && !records(&reply, &mut rows)
    {
        return None;
    }
    if dbh.auto_commit()
        && !swr_firebird_in_manual_txn(dbh)
        && !php_firebird_commit_transaction(dbh, true)
    {
        return None;
    }
    Some(rows.c_cast::<c_int>().into())
}

/// `pdo_firebird_check_liveness`: whether `fb_ping` reaches the server.
#[unsafe(no_mangle)]
pub extern "C" fn pdo_firebird_check_liveness(dbh: &Dbh) -> zend_result {
    if swoole_fb_ping(swr_firebird_status(dbh), swr_firebird_db(dbh)) != 0 {
        FAILURE
    } else {
        SUCCESS
    }
}

/// `pdo_firebird_fetch_error_func`: the connection's last SQL code and message
/// added to PDO's error info, each when it has one.
#[unsafe(no_mangle)]
pub extern "C" fn pdo_firebird_fetch_error_func(dbh: &Dbh, _stmt: Option<&!>, info: &Slot) {
    let mut sqlcode = IS_NULL;
    let mut message = [0u8; MESSAGE_MAX];
    let length = lend_out(&mut message, |out, len| {
        swr_firebird_last_error(dbh, &mut sqlcode, out, len)
    });
    info.edit_array(|info| {
        if sqlcode != IS_NULL {
            info.push_long(sqlcode.into());
        }
        if let Some(message) = message.get(..length).filter(|message| !message.is_empty()) {
            info.push_bytes(message);
        }
    });
}

/// `pdo_firebird_in_manually_transaction`: whether `beginTransaction` began
/// the open transaction. In autocommit mode one is always open.
#[unsafe(no_mangle)]
pub extern "C" fn pdo_firebird_in_manually_transaction(dbh: &Dbh) -> bool {
    swr_firebird_in_manual_txn(dbh)
}

/// The DSN's `dialect` as C stored it: `atoi` (`(int) strtol(text, NULL,
/// 10)`: leading whitespace and a sign taken, saturated at the long's range,
/// truncated to an `int`), kept to the record's two-bit field.
fn dialect(text: &[u8]) -> c_ushort {
    let start = text
        .iter()
        .position(|b| !b" \t\n\x0b\x0c\r".contains(b))
        .unwrap_or(text.len());
    let text = text.get(start..).unwrap_or_default();
    let (negative, digits) = match text.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, text),
    };
    let value = digits
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .fold(0_i64, |value, &digit| {
            let digit = i64::from(digit - b'0');
            if negative {
                value.saturating_mul(10).saturating_sub(digit)
            } else {
                value.saturating_mul(10).saturating_add(digit)
            }
        });
    (value.c_cast::<c_int>() & 3).c_cast()
}

/// `pdo_firebird_handle_factory`'s DPB: `isc_dpb_version1`, then each given
/// value as its tag, its length's low byte and its bytes, as `slprintf` wrote
/// them into the 256 bytes C counted from just past the version.
fn dpb(values: [(u32, Option<Text<'_>>); 4]) -> Vec<u8> {
    let mut dpb = vec![isc_dpb_version1.c_cast::<u8>()];
    let mut room: c_short = 256;
    for (tag, value) in values {
        let Some(value) = value.filter(|_| room > 0) else {
            continue;
        };
        let value = value.get().to_bytes();
        // `slprintf` keeps what fits before its NUL, and returns that length:
        // only those bytes of the tag, the length byte and the value are copied.
        let fits = usize::try_from(room).unwrap_or_default() - 1;
        let head = [tag.c_cast::<u8>(), value.len().c_cast::<u8>()];
        let head = head.get(..fits.min(2)).unwrap_or_default();
        let value = value
            .get(..(fits - head.len()).min(value.len()))
            .unwrap_or_default();
        dpb.extend_from_slice(head);
        dpb.extend_from_slice(value);
        let written = head.len() + value.len();
        room -= c_short::try_from(written).unwrap_or(room);
    }
    dpb
}

/// `pdo_firebird_handle_factory` once C has parsed the DSN: the transaction
/// options taken from `options` (a `ValueError` for an unknown isolation
/// level, connecting anyway as C did), the DPB built from the user, password,
/// charset and role, the dialect set, and the database attached. Attached,
/// the connection gets its PDO methods and, in autocommit mode, its
/// transaction; else a `PDOException` of Firebird's first error. False,
/// the connection closed, when either failed.
#[unsafe(no_mangle)]
pub extern "C" fn swr_firebird_connect(
    dbh: &Dbh,
    options: Option<&Value>,
    dbname: Option<Text<'_>>,
    charset: Option<Text<'_>>,
    role: Option<Text<'_>>,
    dialect: Option<Text<'_>>,
    user: Option<Text<'_>>,
    password: Option<Text<'_>>,
) -> bool {
    swr_firebird_set_manual(dbh, false);
    swr_firebird_set_writable(dbh, attr_long(options, WRITABLE, 1) != 0);
    let level = attr_long(options, ISOLATION_LEVEL, REPEATABLE_READ);
    if [READ_COMMITTED, REPEATABLE_READ, SERIALIZABLE].contains(&level) {
        swr_firebird_set_isolation(dbh, level.cast_unsigned());
    } else {
        swoole_rs_zend::value_error(
            c"Pdo\\Firebird::TRANSACTION_ISOLATION_LEVEL must be a valid transaction isolation level (Pdo\\Firebird::READ_COMMITTED, Pdo\\Firebird::REPEATABLE_READ, or Pdo\\Firebird::SERIALIZABLE)",
        );
    }
    let dpb = dpb([
        (isc_dpb_user_name, user),
        (isc_dpb_password, password),
        (isc_dpb_lc_ctype, charset),
        (isc_dpb_sql_role_name, role),
    ]);
    // `PDO_FB_DIALECT` (3), C's default for the DSN's `dialect`.
    swr_firebird_set_dialect(
        dbh,
        dialect.map_or(3, |text| self::dialect(text.get().to_bytes())),
    );
    let attached = lend_in(&dpb, |bytes, len| {
        swoole_isc_attach_database(
            swr_firebird_status(dbh),
            NameLength::NulTerminated,
            dbname,
            swr_firebird_db(dbh),
            len,
            bytes.chars(),
        )
    }) == 0;
    if attached {
        swr_firebird_set_methods(dbh);
    } else {
        let (text, code) = crate::error::first_error(dbh);
        let mut message = format!("SQLSTATE[HY000] [{code}] ").into_bytes();
        message.extend_from_slice(&text);
        dbh.throw_exception(code.c_cast(), &CString::new(message).unwrap_or_default());
    }
    let connected = attached
        && (!dbh.auto_commit()
            || swr_firebird_in_transaction(dbh)
            || php_firebird_begin_transaction(dbh, true));
    if !connected {
        firebird_handle_closer(dbh);
    }
    connected
}
