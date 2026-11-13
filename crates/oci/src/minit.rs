//! MINIT and MSHUTDOWN for the bundled `pdo_oci` driver (upstream's
//! `php_swoole_oracle_minit`): PDO's `OCI_ATTR_*` constants unless `pdo_oci`
//! declared them, and the driver.
use crate::{Handle, swoole_pdo_oci_Env};
use std::{ffi::c_int, sync::atomic::Ordering};
use swoole_rs_zend::{
    Minit, Mshutdown,
    pdo::{Driver, php_pdo_register_driver},
};

unsafe extern "C" {
    /// The bundled driver (`thirdparty/pdo_oci/oci_driver.c`).
    safe static swoole_pdo_oci_driver: Driver;
    fn OCIHandleFree(handle: Handle, kind: u32) -> c_int;
}

/// `PDO_ATTR_DRIVER_SPECIFIC`, where `php_pdo_oci_int.h`'s attributes start.
const PDO_ATTR_DRIVER_SPECIFIC: i64 = 1000;
/// `OCI_HTYPE_ENV`.
const OCI_HTYPE_ENV: u32 = 1;

/// `php_swoole_oracle_minit`.
/// Nothing unless PDO is loaded.
pub fn minit(minit: &Minit) {
    let Some(pdo) = minit.pdo() else {
        return;
    };
    if !pdo.declared(c"OCI_ATTR_ACTION") {
        let constants = [
            c"OCI_ATTR_ACTION",
            c"OCI_ATTR_CLIENT_INFO",
            c"OCI_ATTR_CLIENT_IDENTIFIER",
            c"OCI_ATTR_MODULE",
            c"OCI_ATTR_CALL_TIMEOUT",
        ];
        for (value, name) in (PDO_ATTR_DRIVER_SPECIFIC..).zip(constants) {
            pdo.declare_long(name, value);
        }
    }
    // Registering replaces pdo_oci's own.
    pdo.unregister(&swoole_pdo_oci_driver);
    // SAFETY: MINIT, with PDO loaded. PDO raises `E_ERROR` for a driver built
    // against another PDO API: no frame up to the step's `zend_try` owns a drop.
    unsafe { php_pdo_register_driver(&swoole_pdo_oci_driver) };
}

/// `php_swoole_oracle_mshutdown`: the driver goes, and the environment free
/// runs as upstream's condition has it, only while none was created.
pub fn mshutdown(mshutdown: &Mshutdown) {
    mshutdown.pdo_unregister(&swoole_pdo_oci_driver);
    let env = swoole_pdo_oci_Env.load(Ordering::Relaxed);
    if env.is_null() {
        // SAFETY: OCI rejects a null handle without touching memory.
        unsafe { OCIHandleFree(env, OCI_HTYPE_ENV) };
    }
}
