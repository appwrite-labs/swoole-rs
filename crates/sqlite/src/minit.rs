//! MINIT and MSHUTDOWN for the bundled `pdo_sqlite` driver (upstream's
//! `php_swoole_sqlite_minit`): PDO's `SQLITE_*` constants unless `pdo_sqlite`
//! declared them, and the driver.
use std::ffi::CStr;
use swoole_rs_zend::{
    Minit, Mshutdown,
    pdo::{Driver, php_pdo_register_driver},
};

unsafe extern "C" {
    /// The bundled driver (`thirdparty/pdo_sqlite/sqlite_driver.c`).
    safe static swoole_pdo_sqlite_driver: Driver;
}

/// `PDO_ATTR_DRIVER_SPECIFIC`, where `php_pdo_sqlite_int.h`'s attributes start.
const PDO_ATTR_DRIVER_SPECIFIC: i64 = 1000;

/// The constants in upstream's order: `SQLITE_DETERMINISTIC` and the open
/// flags as sqlite3.h defines them, the attributes as `php_pdo_sqlite_int.h`.
const CONSTANTS: [(&CStr, i64); 7] = [
    (c"SQLITE_DETERMINISTIC", 0x800),
    (c"SQLITE_ATTR_OPEN_FLAGS", PDO_ATTR_DRIVER_SPECIFIC),
    (c"SQLITE_OPEN_READONLY", 0x1),
    (c"SQLITE_OPEN_READWRITE", 0x2),
    (c"SQLITE_OPEN_CREATE", 0x4),
    (
        c"SQLITE_ATTR_READONLY_STATEMENT",
        PDO_ATTR_DRIVER_SPECIFIC + 1,
    ),
    (
        c"SQLITE_ATTR_EXTENDED_RESULT_CODES",
        PDO_ATTR_DRIVER_SPECIFIC + 2,
    ),
];

/// `php_swoole_sqlite_minit`.
/// Nothing unless PDO is loaded.
pub fn minit(minit: &Minit) {
    let Some(pdo) = minit.pdo() else {
        return;
    };
    if !pdo.declared(c"SQLITE_ATTR_OPEN_FLAGS") {
        for (name, value) in CONSTANTS {
            pdo.declare_long(name, value);
        }
    }
    // Registering replaces pdo_sqlite's own.
    pdo.unregister(&swoole_pdo_sqlite_driver);
    // SAFETY: MINIT, with PDO loaded. PDO raises `E_ERROR` for a driver built
    // against another PDO API: no frame up to the step's `zend_try` owns a drop.
    unsafe { php_pdo_register_driver(&swoole_pdo_sqlite_driver) };
}

/// `php_swoole_sqlite_mshutdown`.
pub fn mshutdown(mshutdown: &Mshutdown) {
    mshutdown.pdo_unregister(&swoole_pdo_sqlite_driver);
}
