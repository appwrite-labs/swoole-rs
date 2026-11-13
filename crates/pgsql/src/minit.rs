//! MINIT and MSHUTDOWN for the bundled `pdo_pgsql` driver (upstream's
//! `php_swoole_pgsql_minit`): PDO's `PGSQL_*` constants unless `pdo_pgsql`
//! declared them, and the driver.
use pq_sys::PGTransactionStatusType::{
    PQTRANS_ACTIVE, PQTRANS_IDLE, PQTRANS_INERROR, PQTRANS_INTRANS, PQTRANS_UNKNOWN,
};
use swoole_rs_zend::{
    Minit, Mshutdown,
    pdo::{Driver, php_pdo_register_driver},
};

unsafe extern "C" {
    /// The bundled driver (`thirdparty/php*/pdo_pgsql/pgsql_driver.c`).
    safe static swoole_pdo_pgsql_driver: Driver;
}

/// `PDO_ATTR_DRIVER_SPECIFIC`, `PDO_PGSQL_ATTR_DISABLE_PREPARES`'s value.
const PDO_ATTR_DRIVER_SPECIFIC: i64 = 1000;

/// `php_swoole_pgsql_minit`.
/// Nothing unless PDO is loaded.
pub fn minit(minit: &Minit) {
    let Some(pdo) = minit.pdo() else {
        return;
    };
    if !pdo.declared(c"PGSQL_ATTR_DISABLE_PREPARES") {
        let constants = [
            (c"PGSQL_ATTR_DISABLE_PREPARES", PDO_ATTR_DRIVER_SPECIFIC),
            (c"PGSQL_TRANSACTION_IDLE", PQTRANS_IDLE as i64),
            (c"PGSQL_TRANSACTION_ACTIVE", PQTRANS_ACTIVE as i64),
            (c"PGSQL_TRANSACTION_INTRANS", PQTRANS_INTRANS as i64),
            (c"PGSQL_TRANSACTION_INERROR", PQTRANS_INERROR as i64),
            (c"PGSQL_TRANSACTION_UNKNOWN", PQTRANS_UNKNOWN as i64),
        ];
        for (name, value) in constants {
            pdo.declare_long(name, value);
        }
    }
    // Registering replaces pdo_pgsql's own.
    pdo.unregister(&swoole_pdo_pgsql_driver);
    // SAFETY: MINIT, with PDO loaded. PDO raises `E_ERROR` for a driver built
    // against another PDO API: no frame up to the step's `zend_try` owns a drop.
    unsafe { php_pdo_register_driver(&swoole_pdo_pgsql_driver) };
}

/// `php_swoole_pgsql_mshutdown`.
pub fn mshutdown(mshutdown: &Mshutdown) {
    mshutdown.pdo_unregister(&swoole_pdo_pgsql_driver);
}
