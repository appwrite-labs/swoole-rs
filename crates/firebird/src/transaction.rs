//! `pdo_firebird`'s transactions (`firebird_driver.c`'s
//! `php_firebird_begin_transaction`, `_commit_transaction`,
//! `_rollback_transaction` and the `firebird_handle_manually_*` methods PDO
//! calls), under their C names. The connection record stays C: its status
//! vector, database and transaction handles and transaction options are read
//! through the driver's `swr_firebird_*` accessors.
//!
//! Firebird has no autocommit, so in PDO's autocommit mode a transaction is
//! always open: a commit retains it, and a manual commit or rollback reopens it.
use std::ffi::c_ulong;
use swoole_rs_firebird_sys::{Bytes, Handle, Status};
use swoole_rs_zend::pdo::Dbh;

use crate::{
    swoole_isc_commit_retaining, swoole_isc_commit_transaction, swoole_isc_rollback_transaction,
    swoole_isc_start_transaction,
};

unsafe extern "C" {
    /// The connection's status vector, `&H->db` and `&H->tr`.
    safe fn swr_firebird_status(dbh: &Dbh) -> Status;
    safe fn swr_firebird_db(dbh: &Dbh) -> Handle<'_>;
    safe fn swr_firebird_tr(dbh: &Dbh) -> Handle<'_>;
    /// Whether the connection has a transaction open (`H->tr`).
    safe fn swr_firebird_in_transaction(dbh: &Dbh) -> bool;
    /// `H->is_writable_txn` and `H->txn_isolation_level`.
    safe fn swr_firebird_txn_options(dbh: &Dbh, writable: &mut bool, level: &mut c_ulong);
    /// `H->in_manually_txn = manual`.
    safe fn swr_firebird_set_manual(dbh: &Dbh, manual: bool);
    /// `php_firebird_error(dbh)`: the status vector's error recorded on the
    /// connection, thrown when the connection is not yet made.
    safe fn swr_firebird_error(dbh: &Dbh);
}

/// `PDO_FB_READ_COMMITTED` and `PDO_FB_SERIALIZABLE`
/// (`PDO_ATTR_DRIVER_SPECIFIC` + 4 and + 6); any other level is
/// `PDO_FB_REPEATABLE_READ`.
const READ_COMMITTED: c_ulong = 1004;
const SERIALIZABLE: c_ulong = 1006;

/// The transaction parameter blocks `php_firebird_begin_transaction` built:
/// `isc_tpb_version3`, `isc_tpb_write` or `isc_tpb_read`, then
/// `isc_tpb_read_committed` with `isc_tpb_rec_version` (autocommit, and the
/// read committed level), `isc_tpb_consistency` (serializable) or
/// `isc_tpb_concurrency` (repeatable read).
static WRITE_READ_COMMITTED: [u8; 4] = [3, 9, 15, 17];
static READ_READ_COMMITTED: [u8; 4] = [3, 8, 15, 17];
static WRITE_CONSISTENCY: [u8; 3] = [3, 9, 1];
static READ_CONSISTENCY: [u8; 3] = [3, 8, 1];
static WRITE_CONCURRENCY: [u8; 3] = [3, 9, 2];
static READ_CONCURRENCY: [u8; 3] = [3, 8, 2];

/// The parameter block for a transaction in autocommit mode or of `level`.
fn tpb(writable: bool, auto_commit: bool, level: c_ulong) -> &'static [u8] {
    match (
        auto_commit || level == READ_COMMITTED,
        level == SERIALIZABLE,
        writable,
    ) {
        (true, _, true) => &WRITE_READ_COMMITTED,
        (true, _, false) => &READ_READ_COMMITTED,
        (false, true, true) => &WRITE_CONSISTENCY,
        (false, true, false) => &READ_CONSISTENCY,
        (false, false, true) => &WRITE_CONCURRENCY,
        (false, false, false) => &READ_CONCURRENCY,
    }
}

/// `php_firebird_begin_transaction`: a transaction started on the connection,
/// read committed in autocommit mode, else at the connection's isolation
/// level; false with the error recorded.
#[unsafe(no_mangle)]
pub extern "C" fn php_firebird_begin_transaction(dbh: &Dbh, auto_commit: bool) -> bool {
    let (mut writable, mut level) = (false, 0);
    swr_firebird_txn_options(dbh, &mut writable, &mut level);
    let tpb = tpb(writable, auto_commit, level);
    let started = swoole_isc_start_transaction(
        swr_firebird_status(dbh),
        swr_firebird_tr(dbh),
        1,
        swr_firebird_db(dbh),
        tpb.len(),
        Bytes::from_static(tpb),
    );
    if started != 0 {
        swr_firebird_error(dbh);
        return false;
    }
    true
}

/// `php_firebird_commit_transaction`: the transaction committed, kept open
/// when `retain`; false with the error recorded.
#[unsafe(no_mangle)]
pub extern "C" fn php_firebird_commit_transaction(dbh: &Dbh, retain: bool) -> bool {
    let (status, tr) = (swr_firebird_status(dbh), swr_firebird_tr(dbh));
    let committed = if retain {
        swoole_isc_commit_retaining(status, tr)
    } else {
        swoole_isc_commit_transaction(status, tr)
    };
    if committed != 0 {
        swr_firebird_error(dbh);
        return false;
    }
    true
}

/// `php_firebird_rollback_transaction`: the transaction rolled back; false
/// with the error recorded.
#[unsafe(no_mangle)]
pub extern "C" fn php_firebird_rollback_transaction(dbh: &Dbh) -> bool {
    if swoole_isc_rollback_transaction(swr_firebird_status(dbh), swr_firebird_tr(dbh)) != 0 {
        swr_firebird_error(dbh);
        return false;
    }
    true
}

/// `firebird_handle_manually_begin`: `PDO::beginTransaction`. In autocommit
/// mode the open transaction is committed first.
#[unsafe(no_mangle)]
pub extern "C" fn firebird_handle_manually_begin(dbh: &Dbh) -> bool {
    if dbh.auto_commit()
        && swr_firebird_in_transaction(dbh)
        && !php_firebird_commit_transaction(dbh, false)
    {
        return false;
    }
    if !php_firebird_begin_transaction(dbh, false) {
        return false;
    }
    swr_firebird_set_manual(dbh, true);
    true
}

/// `firebird_handle_manually_commit`: `PDO::commit`, then in autocommit mode
/// a new transaction, as the isolation level may have changed.
#[unsafe(no_mangle)]
pub extern "C" fn firebird_handle_manually_commit(dbh: &Dbh) -> bool {
    if !php_firebird_commit_transaction(dbh, false) {
        return false;
    }
    if dbh.auto_commit() && !php_firebird_begin_transaction(dbh, true) {
        return false;
    }
    swr_firebird_set_manual(dbh, false);
    true
}

/// `firebird_handle_manually_rollback`: `PDO::rollBack`, then in autocommit
/// mode a new transaction.
#[unsafe(no_mangle)]
pub extern "C" fn firebird_handle_manually_rollback(dbh: &Dbh) -> bool {
    if !php_firebird_rollback_transaction(dbh) {
        return false;
    }
    if dbh.auto_commit() && !php_firebird_begin_transaction(dbh, true) {
        return false;
    }
    swr_firebird_set_manual(dbh, false);
    true
}
