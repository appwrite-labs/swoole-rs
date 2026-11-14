//! MINIT and MSHUTDOWN for the bundled `pdo_sqlite` driver (upstream's
//! `php_swoole_sqlite_minit`): PDO's `SQLITE_*` constants unless `pdo_sqlite`
//! declared them, and the driver.
use std::ffi::{CStr, c_int};
use swoole_rs_zend::sys::{swr_zend_pdo_declare_long, swr_zend_pdo_declared};

/// `pdo_driver_t`, opaque.
#[repr(C)]
pub struct PdoDriver {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    /// The bundled driver (`thirdparty/pdo_sqlite/sqlite_driver.c`).
    static swoole_pdo_sqlite_driver: PdoDriver;
    fn php_pdo_register_driver(driver: *const PdoDriver) -> c_int;
    fn php_pdo_unregister_driver(driver: *const PdoDriver);
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
/// # Safety
/// Called once at MINIT, with PDO loaded.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_sqlite_minit(_module: c_int) {
    // SAFETY: MINIT, PDO loaded.
    if !unsafe { swr_zend_pdo_declared(c"SQLITE_ATTR_OPEN_FLAGS".as_ptr()) } {
        for (name, value) in CONSTANTS {
            // SAFETY: MINIT, PDO loaded.
            unsafe { swr_zend_pdo_declare_long(name.as_ptr(), value) };
        }
    }
    let driver = &raw const swoole_pdo_sqlite_driver;
    // SAFETY: the driver is static; registering replaces pdo_sqlite's own.
    unsafe { php_pdo_unregister_driver(driver) };
    // SAFETY: as above.
    unsafe { php_pdo_register_driver(driver) };
}

/// `php_swoole_sqlite_mshutdown`.
/// # Safety
/// Called once at MSHUTDOWN.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_sqlite_mshutdown() {
    // SAFETY: the driver is static.
    unsafe { php_pdo_unregister_driver(&raw const swoole_pdo_sqlite_driver) };
}
