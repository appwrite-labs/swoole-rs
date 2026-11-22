//! `pdo_firebird`'s connection methods (`firebird_driver.c`), under their C
//! names: so far `pdo_firebird_check_liveness`, `pdo_firebird_fetch_error_func`
//! and `pdo_firebird_in_manually_transaction`. The connection record stays C,
//! read through the driver's `swr_firebird_*` accessors.
use std::ffi::c_int;
use swoole_rs_corebase::lent::{Count, Out, lend_out};
use swoole_rs_firebird_sys::{Handle, Status};
use swoole_rs_zend::pdo::{Dbh, Slot};
use swoole_rs_zend::sys::zend_result;

use crate::swoole_fb_ping;

unsafe extern "C" {
    /// The connection's status vector and `&H->db`.
    safe fn swr_firebird_status(dbh: &Dbh) -> Status;
    safe fn swr_firebird_db(dbh: &Dbh) -> Handle;
    /// `H->in_manually_txn`.
    safe fn swr_firebird_in_manual_txn(dbh: &Dbh) -> bool;
    /// The connection's last error (`H->einfo`): its SQL code in `sqlcode`,
    /// and up to the lent bytes of its message, whose length it returns.
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
