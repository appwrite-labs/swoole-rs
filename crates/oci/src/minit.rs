//! MINIT and MSHUTDOWN for the bundled `pdo_oci` driver (upstream's
//! `php_swoole_oracle_minit`): PDO's `OCI_ATTR_*` constants unless `pdo_oci`
//! declared them, and the driver.
use crate::{Handle, swoole_pdo_oci_Env};
use std::{ffi::c_int, sync::atomic::Ordering};
use swoole_rs_zend::sys::{swr_zend_pdo_declare_long, swr_zend_pdo_declared};

/// `pdo_driver_t`, opaque.
#[repr(C)]
pub struct PdoDriver {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    /// The bundled driver (`thirdparty/pdo_oci/oci_driver.c`).
    static swoole_pdo_oci_driver: PdoDriver;
    fn php_pdo_register_driver(driver: *const PdoDriver) -> c_int;
    fn php_pdo_unregister_driver(driver: *const PdoDriver);
    fn OCIHandleFree(handle: Handle, kind: u32) -> c_int;
}

/// `PDO_ATTR_DRIVER_SPECIFIC`, where `php_pdo_oci_int.h`'s attributes start.
const PDO_ATTR_DRIVER_SPECIFIC: i64 = 1000;
/// `OCI_HTYPE_ENV`.
const OCI_HTYPE_ENV: u32 = 1;

/// `php_swoole_oracle_minit`.
/// # Safety
/// Called once at MINIT, with PDO loaded.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_oracle_minit(_module: c_int) {
    // SAFETY: MINIT, PDO loaded.
    if !unsafe { swr_zend_pdo_declared(c"OCI_ATTR_ACTION".as_ptr()) } {
        let constants = [
            c"OCI_ATTR_ACTION",
            c"OCI_ATTR_CLIENT_INFO",
            c"OCI_ATTR_CLIENT_IDENTIFIER",
            c"OCI_ATTR_MODULE",
            c"OCI_ATTR_CALL_TIMEOUT",
        ];
        for (value, name) in (PDO_ATTR_DRIVER_SPECIFIC..).zip(constants) {
            // SAFETY: MINIT, PDO loaded.
            unsafe { swr_zend_pdo_declare_long(name.as_ptr(), value) };
        }
    }
    let driver = &raw const swoole_pdo_oci_driver;
    // SAFETY: the driver is static; registering replaces pdo_oci's own.
    unsafe { php_pdo_unregister_driver(driver) };
    // SAFETY: as above.
    unsafe { php_pdo_register_driver(driver) };
}

/// `php_swoole_oracle_mshutdown`: the driver goes, and the environment free
/// runs as upstream's condition has it, only while none was created.
/// # Safety
/// Called once at MSHUTDOWN.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_oracle_mshutdown() {
    // SAFETY: the driver is static.
    unsafe { php_pdo_unregister_driver(&raw const swoole_pdo_oci_driver) };
    let env = swoole_pdo_oci_Env.load(Ordering::Relaxed);
    if env.is_null() {
        // SAFETY: OCI rejects a null handle without touching memory.
        unsafe { OCIHandleFree(env, OCI_HTYPE_ENV) };
    }
}
