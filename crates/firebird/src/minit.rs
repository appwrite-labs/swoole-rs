//! MINIT and MSHUTDOWN for the bundled `pdo_firebird` driver (upstream's
//! `php_swoole_firebird_minit`): PDO's `FB_*` constants unless `pdo_firebird`
//! declared them, and the driver.
use std::ffi::c_int;
use swoole_rs_zend::sys::{swr_zend_pdo_declare_long, swr_zend_pdo_declared};

/// `pdo_driver_t`, opaque.
#[repr(C)]
pub struct PdoDriver {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    /// The bundled driver (`thirdparty/php*/pdo_firebird/firebird_driver.c`).
    static swoole_pdo_firebird_driver: PdoDriver;
    fn php_pdo_register_driver(driver: *const PdoDriver) -> c_int;
    fn php_pdo_unregister_driver(driver: *const PdoDriver);
}

/// `PDO_ATTR_DRIVER_SPECIFIC`, where `php_pdo_firebird_int.h`'s attributes start.
const PDO_ATTR_DRIVER_SPECIFIC: i64 = 1000;

/// `php_swoole_firebird_minit`.
/// # Safety
/// Called once at MINIT, with PDO loaded.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_firebird_minit(_module: c_int) {
    // SAFETY: MINIT, PDO loaded.
    if !unsafe { swr_zend_pdo_declared(c"FB_ATTR_DATE_FORMAT".as_ptr()) } {
        let constants = [
            (c"FB_ATTR_DATE_FORMAT", PDO_ATTR_DRIVER_SPECIFIC),
            (c"FB_ATTR_TIME_FORMAT", PDO_ATTR_DRIVER_SPECIFIC + 1),
            (c"FB_ATTR_TIMESTAMP_FORMAT", PDO_ATTR_DRIVER_SPECIFIC + 2),
        ];
        for (name, value) in constants {
            // SAFETY: MINIT, PDO loaded.
            unsafe { swr_zend_pdo_declare_long(name.as_ptr(), value) };
        }
    }
    let driver = &raw const swoole_pdo_firebird_driver;
    // SAFETY: the driver is static; registering replaces pdo_firebird's own.
    unsafe { php_pdo_unregister_driver(driver) };
    // SAFETY: as above.
    unsafe { php_pdo_register_driver(driver) };
}

/// `php_swoole_firebird_mshutdown`.
/// # Safety
/// Called once at MSHUTDOWN.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_firebird_mshutdown() {
    // SAFETY: the driver is static.
    unsafe { php_pdo_unregister_driver(&raw const swoole_pdo_firebird_driver) };
}
