//! `pdo_firebird`'s connection attributes (`firebird_driver.c`'s
//! `pdo_firebird_set_attribute` and `pdo_firebird_get_attribute`), under their
//! C names. The connection record stays C: its fields are read and written
//! through the driver's `swr_firebird_*` accessors, the format strings
//! allocated there.
use std::ffi::{CStr, c_int, c_ulong};
use swoole_rs_corebase::lent::{Count, In, Out, lend_in, lend_out};
use swoole_rs_corebase::sys::Text;
use swoole_rs_zend::pdo::{Attribute, Dbh, Slot, bool_param, long_param};
use swoole_rs_zend::sys::{
    pdo_attribute_type_PDO_ATTR_AUTOCOMMIT as AUTOCOMMIT,
    pdo_attribute_type_PDO_ATTR_CLIENT_VERSION as CLIENT_VERSION,
    pdo_attribute_type_PDO_ATTR_CONNECTION_STATUS as CONNECTION_STATUS,
    pdo_attribute_type_PDO_ATTR_DRIVER_SPECIFIC as DRIVER_SPECIFIC,
    pdo_attribute_type_PDO_ATTR_FETCH_TABLE_NAMES as FETCH_TABLE_NAMES,
    pdo_attribute_type_PDO_ATTR_SERVER_INFO as SERVER_INFO,
    pdo_attribute_type_PDO_ATTR_SERVER_VERSION as SERVER_VERSION, zend_long,
};
use swoole_rs_zend::{Owned, Value, ZString, value_error};

use crate::transaction::{php_firebird_begin_transaction, php_firebird_commit_transaction};

unsafe extern "C" {
    /// `H->in_manually_txn`, and whether a transaction is open (`H->tr`).
    safe fn swr_firebird_in_manual_txn(dbh: &Dbh) -> bool;
    safe fn swr_firebird_in_transaction(dbh: &Dbh) -> bool;
    /// `H->is_writable_txn` and `H->txn_isolation_level`, and setting each.
    safe fn swr_firebird_txn_options(dbh: &Dbh, writable: &mut bool, level: &mut c_ulong);
    safe fn swr_firebird_set_writable(dbh: &Dbh, writable: bool);
    safe fn swr_firebird_set_isolation(dbh: &Dbh, level: c_ulong);
    /// `H->fetch_table_names`, and setting it.
    safe fn swr_firebird_fetch_table_names(dbh: &Dbh) -> bool;
    safe fn swr_firebird_set_fetch_table_names(dbh: &Dbh, on: bool);
    /// The date (0), time (1) or timestamp (2) format, or its default, live
    /// until it is set again.
    safe fn swr_firebird_format<'a>(dbh: &'a Dbh, which: c_int) -> Text<'a>;
    /// The format replaced by a copy of the lent bytes (`pestrndup`), the old
    /// one freed.
    safe fn swr_firebird_set_format<'a>(
        dbh: &Dbh,
        which: c_int,
        format: In<'a, u8>,
        len: Count<'a>,
    );
    /// `!isc_version(&H->db, ...)`: whether the server answered, its version
    /// lines joined by spaces into the lent 512 bytes, NUL-terminated.
    safe fn swr_firebird_server_version<'a>(dbh: &Dbh, info: Out<'a, u8>, len: Count<'a>) -> bool;
    /// `!isc_version(&H->db, ...)` with no buffer: whether the server answered.
    safe fn swr_firebird_connected(dbh: &Dbh) -> bool;
    /// `isc_get_client_version` into the lent 512 bytes, NUL-terminated.
    safe fn swr_firebird_client_version<'a>(version: Out<'a, u8>, len: Count<'a>);
    /// PDO's own: the impl error `sqlstate` with `message`.
    safe fn pdo_raise_impl_error(
        dbh: &Dbh,
        stmt: Option<&!>,
        sqlstate: Text<'static>,
        message: Text<'static>,
    );
}

/// `php_pdo_firebird_int.h`'s driver attributes, from `PDO_ATTR_DRIVER_SPECIFIC`.
const DATE_FORMAT: u32 = DRIVER_SPECIFIC;
const TIME_FORMAT: u32 = DRIVER_SPECIFIC + 1;
const TIMESTAMP_FORMAT: u32 = DRIVER_SPECIFIC + 2;
const ISOLATION_LEVEL: u32 = DRIVER_SPECIFIC + 3;
const READ_COMMITTED: zend_long = DRIVER_SPECIFIC as zend_long + 4;
const REPEATABLE_READ: zend_long = DRIVER_SPECIFIC as zend_long + 5;
const SERIALIZABLE: zend_long = DRIVER_SPECIFIC as zend_long + 6;
const WRITABLE: u32 = DRIVER_SPECIFIC + 7;
/// `INFO_BUF_LEN`.
const INFO_LEN: usize = 512;

/// `pdo_raise_impl_error(dbh, NULL, "HY000", message)`, its bailout deferred.
fn refuse(dbh: &Dbh, message: &'static CStr) {
    swoole_rs_zend::bailout::defer(swoole_rs_zend::contain_with(|| {
        pdo_raise_impl_error(dbh, None, c"HY000".into(), message.into());
    }));
}

swoole_rs_zend::exports! {
/// `pdo_firebird_set_attribute`: `attr` set to `val`, or false.
#[unsafe(no_mangle)]
pub extern "C" fn pdo_firebird_set_attribute(dbh: &Dbh, attr: zend_long, val: &Value) -> bool {
    match u32::try_from(attr) {
        Ok(AUTOCOMMIT) => set_auto_commit(dbh, val),
        Ok(FETCH_TABLE_NAMES) => bool_param(val).is_some_and(|on| {
            swr_firebird_set_fetch_table_names(dbh, on);
            true
        }),
        Ok(which @ (DATE_FORMAT | TIME_FORMAT | TIMESTAMP_FORMAT)) => {
            let Some(format) = val.try_string() else {
                return false;
            };
            let which = c_int::try_from(which - DATE_FORMAT).unwrap_or_default();
            lend_in(format.bytes(), |bytes, len| {
                swr_firebird_set_format(dbh, which, bytes, len);
            });
            true
        }
        Ok(ISOLATION_LEVEL) => set_isolation(dbh, val),
        Ok(WRITABLE) => set_writable(dbh, val),
        _ => false,
    }
}

/// `pdo_firebird_get_attribute`: `attr`'s value in `val`.
#[unsafe(no_mangle)]
pub extern "C" fn pdo_firebird_get_attribute(dbh: &Dbh, attr: zend_long, val: &Slot) -> Attribute {
    let (mut writable, mut level) = (false, 0);
    let value = match u32::try_from(attr) {
        Ok(AUTOCOMMIT) => Owned::bool(dbh.auto_commit()),
        Ok(CONNECTION_STATUS) => Owned::bool(swr_firebird_connected(dbh)),
        Ok(CLIENT_VERSION) => {
            let mut version = [0u8; INFO_LEN];
            lend_out(&mut version, |out, len: Count<'_>| swr_firebird_client_version(out, len));
            string(&version)
        }
        Ok(SERVER_VERSION | SERVER_INFO) => {
            let mut info = [0u8; INFO_LEN];
            if !lend_out(&mut info, |out, len| swr_firebird_server_version(dbh, out, len)) {
                return Attribute::Failed;
            }
            string(&info)
        }
        Ok(FETCH_TABLE_NAMES) => Owned::bool(swr_firebird_fetch_table_names(dbh)),
        Ok(which @ (DATE_FORMAT | TIME_FORMAT | TIMESTAMP_FORMAT)) => {
            let which = c_int::try_from(which - DATE_FORMAT).unwrap_or_default();
            Owned::string(ZString::new(swr_firebird_format(dbh, which).get().to_bytes()))
        }
        Ok(ISOLATION_LEVEL) => {
            swr_firebird_txn_options(dbh, &mut writable, &mut level);
            // ZVAL_LONG of the zend_ulong, as C stored it.
            Owned::long(level.cast_signed())
        }
        Ok(WRITABLE) => {
            swr_firebird_txn_options(dbh, &mut writable, &mut level);
            Owned::bool(writable)
        }
        _ => return Attribute::Unknown,
    };
    val.set(value);
    Attribute::Set
}
}

/// A NUL-terminated buffer's string.
fn string(buffer: &[u8]) -> Owned {
    let text = CStr::from_bytes_until_nul(buffer).map_or(&b""[..], CStr::to_bytes);
    Owned::string(ZString::new(text))
}

/// `PDO_ATTR_AUTOCOMMIT`: refused while a manual transaction is open. In
/// autocommit mode a transaction is always open, so turning it on starts one
/// and turning it off commits it.
fn set_auto_commit(dbh: &Dbh, val: &Value) -> bool {
    let Some(on) = bool_param(val) else {
        return false;
    };
    if swr_firebird_in_manual_txn(dbh) {
        refuse(
            dbh,
            c"Cannot change autocommit mode while a transaction is already open",
        );
        return false;
    }
    if dbh.auto_commit() != on {
        let open = swr_firebird_in_transaction(dbh);
        if on && !open && !php_firebird_begin_transaction(dbh, true) {
            return false;
        }
        if !on && open && !php_firebird_commit_transaction(dbh, false) {
            return false;
        }
        dbh.set_auto_commit(on);
    }
    true
}

/// `Pdo\Firebird::TRANSACTION_ISOLATION_LEVEL`: used from the next manual
/// transaction, autocommit being read committed; refused while a manual one is
/// open, and a `ValueError` for a level that isn't one.
fn set_isolation(dbh: &Dbh, val: &Value) -> bool {
    let Some(level) = long_param(val) else {
        return false;
    };
    if swr_firebird_in_manual_txn(dbh) {
        refuse(
            dbh,
            c"Cannot change transaction isolation level while a transaction is already open",
        );
        return false;
    }
    let (mut writable, mut current) = (false, 0);
    swr_firebird_txn_options(dbh, &mut writable, &mut current);
    // C compared the zend_ulong with the zend_long as unsigned.
    if current != level.cast_unsigned() {
        if ![READ_COMMITTED, REPEATABLE_READ, SERIALIZABLE].contains(&level) {
            value_error(
                c"Pdo\\Firebird::TRANSACTION_ISOLATION_LEVEL must be a valid transaction isolation level (Pdo\\Firebird::READ_COMMITTED, Pdo\\Firebird::REPEATABLE_READ, or Pdo\\Firebird::SERIALIZABLE)",
            );
            return false;
        }
        swr_firebird_set_isolation(dbh, level.cast_unsigned());
    }
    true
}

/// `Pdo\Firebird::WRITABLE_TRANSACTION`: refused while a manual transaction is
/// open; in autocommit mode the open transaction is committed and a new one
/// started, the setting reverted if either fails.
fn set_writable(dbh: &Dbh, val: &Value) -> bool {
    let Some(on) = bool_param(val) else {
        return false;
    };
    if swr_firebird_in_manual_txn(dbh) {
        refuse(
            dbh,
            c"Cannot change access mode while a transaction is already open",
        );
        return false;
    }
    let (mut writable, mut level) = (false, 0);
    swr_firebird_txn_options(dbh, &mut writable, &mut level);
    if writable != on {
        swr_firebird_set_writable(dbh, on);
        if dbh.auto_commit() {
            let reopened = (!swr_firebird_in_transaction(dbh)
                || php_firebird_commit_transaction(dbh, false))
                && php_firebird_begin_transaction(dbh, true);
            if !reopened {
                swr_firebird_set_writable(dbh, !on);
                return false;
            }
        }
    }
    true
}
