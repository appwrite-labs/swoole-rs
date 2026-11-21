//! MINIT and MSHUTDOWN for the bundled `pdo_pgsql` driver (upstream's
//! `php_swoole_pgsql_minit`): PDO's `PGSQL_*` constants unless `pdo_pgsql`
//! declared them, and the driver.
use pq_sys::PGTransactionStatusType::{
    PQTRANS_ACTIVE, PQTRANS_IDLE, PQTRANS_INERROR, PQTRANS_INTRANS, PQTRANS_UNKNOWN,
};
use std::ffi::c_int;
use swoole_rs_zend::sys::{swr_zend_pdo_declare_long, swr_zend_pdo_declared};

/// `pdo_driver_t`, opaque.
#[repr(C)]
pub struct PdoDriver {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    /// The bundled driver (`thirdparty/php*/pdo_pgsql/pgsql_driver.c`).
    static swoole_pdo_pgsql_driver: PdoDriver;
    fn php_pdo_register_driver(driver: *const PdoDriver) -> c_int;
    fn php_pdo_unregister_driver(driver: *const PdoDriver);
}

/// `PDO_ATTR_DRIVER_SPECIFIC`, `PDO_PGSQL_ATTR_DISABLE_PREPARES`'s value.
const PDO_ATTR_DRIVER_SPECIFIC: i64 = 1000;

/// `php_swoole_pgsql_minit`.
/// # Safety
/// Called once at MINIT, with PDO loaded.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_pgsql_minit(_module: c_int) {
    // SAFETY: MINIT, PDO loaded.
    if !unsafe { swr_zend_pdo_declared(c"PGSQL_ATTR_DISABLE_PREPARES".as_ptr()) } {
        let constants = [
            (c"PGSQL_ATTR_DISABLE_PREPARES", PDO_ATTR_DRIVER_SPECIFIC),
            (c"PGSQL_TRANSACTION_IDLE", PQTRANS_IDLE as i64),
            (c"PGSQL_TRANSACTION_ACTIVE", PQTRANS_ACTIVE as i64),
            (c"PGSQL_TRANSACTION_INTRANS", PQTRANS_INTRANS as i64),
            (c"PGSQL_TRANSACTION_INERROR", PQTRANS_INERROR as i64),
            (c"PGSQL_TRANSACTION_UNKNOWN", PQTRANS_UNKNOWN as i64),
        ];
        for (name, value) in constants {
            // SAFETY: MINIT, PDO loaded.
            unsafe { swr_zend_pdo_declare_long(name.as_ptr(), value) };
        }
    }
    let driver = &raw const swoole_pdo_pgsql_driver;
    // SAFETY: the driver is static; registering replaces pdo_pgsql's own.
    unsafe { php_pdo_unregister_driver(driver) };
    // SAFETY: as above.
    unsafe { php_pdo_register_driver(driver) };
}

/// `php_swoole_pgsql_mshutdown`.
/// # Safety
/// Called once at MSHUTDOWN.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_pgsql_mshutdown() {
    // SAFETY: the driver is static.
    unsafe { php_pdo_unregister_driver(&raw const swoole_pdo_pgsql_driver) };
}
