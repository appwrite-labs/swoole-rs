//! MINIT and MSHUTDOWN for the bundled `pdo_firebird` driver (upstream's
//! `php_swoole_firebird_minit`): PDO's `FB_*` constants unless `pdo_firebird`
//! declared them, and the driver.
use swoole_rs_zend::{
    Minit, Mshutdown,
    pdo::{Driver, php_pdo_register_driver},
};

unsafe extern "C" {
    /// The bundled driver (`thirdparty/php*/pdo_firebird/firebird_driver.c`).
    safe static swoole_pdo_firebird_driver: Driver;
}

/// `PDO_ATTR_DRIVER_SPECIFIC`, where `php_pdo_firebird_int.h`'s attributes start.
const PDO_ATTR_DRIVER_SPECIFIC: i64 = 1000;

/// `php_swoole_firebird_minit`.
/// Nothing unless PDO is loaded.
pub fn minit(minit: &Minit) {
    let Some(pdo) = minit.pdo() else {
        return;
    };
    if !pdo.declared(c"FB_ATTR_DATE_FORMAT") {
        let constants = [
            (c"FB_ATTR_DATE_FORMAT", PDO_ATTR_DRIVER_SPECIFIC),
            (c"FB_ATTR_TIME_FORMAT", PDO_ATTR_DRIVER_SPECIFIC + 1),
            (c"FB_ATTR_TIMESTAMP_FORMAT", PDO_ATTR_DRIVER_SPECIFIC + 2),
        ];
        for (name, value) in constants {
            pdo.declare_long(name, value);
        }
    }
    // Registering replaces pdo_firebird's own.
    pdo.unregister(&swoole_pdo_firebird_driver);
    // SAFETY: MINIT, with PDO loaded. PDO raises `E_ERROR` for a driver built
    // against another PDO API: no frame up to the step's `zend_try` owns a drop.
    unsafe { php_pdo_register_driver(&swoole_pdo_firebird_driver) };
}

/// `php_swoole_firebird_mshutdown`.
pub fn mshutdown(mshutdown: &Mshutdown) {
    mshutdown.pdo_unregister(&swoole_pdo_firebird_driver);
}
