//! MINIT for the bundled `pdo_odbc` driver (upstream's
//! `php_swoole_odbc_minit`): connection pooling from `pdo_odbc.connection_pooling`,
//! PDO's `ODBC_*` constants unless `pdo_odbc` declared them, and the driver.
use std::{
    ffi::{CStr, c_char, c_int},
    sync::atomic::{AtomicUsize, Ordering},
};
use swoole_rs_odbc_sys::{SQLHENV, SQLINTEGER, SQLPOINTER, SQLSetEnvAttr};
use swoole_rs_zend::sys::{E_CORE_ERROR, swr_zend_pdo_declare_long, swr_zend_pdo_declared};

/// `pdo_driver_t`, opaque.
#[repr(C)]
pub struct PdoDriver {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    /// The bundled driver (`thirdparty/php*/pdo_odbc/odbc_driver.c`).
    static swoole_pdo_odbc_driver: PdoDriver;
    fn php_pdo_register_driver(driver: *const PdoDriver) -> c_int;
    fn php_pdo_unregister_driver(driver: *const PdoDriver);
    fn cfg_get_string(name: *const c_char, result: *mut *mut c_char) -> c_int;
}

/// `sqlext.h`'s pooling values.
const SQL_ATTR_CONNECTION_POOLING: SQLINTEGER = 201;
const SQL_CP_OFF: usize = 0;
const SQL_CP_ONE_PER_HENV: usize = 2;
const SQL_CP_STRICT_MATCH: usize = 0;
const SQL_CP_RELAXED_MATCH: usize = 1;
/// `PDO_ATTR_DRIVER_SPECIFIC`, where `php_pdo_odbc_int.h`'s attributes start.
const PDO_ATTR_DRIVER_SPECIFIC: i64 = 1000;

/// The `pdo_odbc_pool_on` and `pdo_odbc_pool_mode` the driver reads
/// (`zend_ulong`, as `php_pdo_odbc_int.h` declares them).
#[unsafe(no_mangle)]
pub static pdo_odbc_pool_on: AtomicUsize = AtomicUsize::new(SQL_CP_OFF);
#[unsafe(no_mangle)]
pub static pdo_odbc_pool_mode: AtomicUsize = AtomicUsize::new(SQL_CP_ONE_PER_HENV);

/// `pdo_odbc.connection_pooling`'s (on, mode), or none for an invalid value.
fn pooling(value: &[u8]) -> Option<(usize, usize)> {
    if value.eq_ignore_ascii_case(b"strict") || value == b"1" {
        Some((SQL_CP_ONE_PER_HENV, SQL_CP_STRICT_MATCH))
    } else if value.eq_ignore_ascii_case(b"relaxed") {
        Some((SQL_CP_ONE_PER_HENV, SQL_CP_RELAXED_MATCH))
    } else if value.is_empty() || value.eq_ignore_ascii_case(b"off") {
        Some((SQL_CP_OFF, pdo_odbc_pool_mode.load(Ordering::Relaxed)))
    } else {
        None
    }
}

/// `php_swoole_odbc_minit`: `SUCCESS` (0), or `FAILURE` (-1) for an invalid
/// `pdo_odbc.connection_pooling`.
/// # Safety
/// Called once at MINIT, with PDO loaded.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_odbc_minit(_module: c_int) -> c_int {
    // SAFETY: MINIT, PDO loaded.
    if !unsafe { swr_zend_pdo_declared(c"ODBC_ATTR_USE_CURSOR_LIBRARY".as_ptr()) } {
        // The ini is process wide, so it is read this once, as upstream does.
        let mut value: *mut c_char = std::ptr::null_mut();
        // SAFETY: a NUL-terminated name and an out pointer.
        let found =
            unsafe { cfg_get_string(c"pdo_odbc.connection_pooling".as_ptr(), &raw mut value) } == 0;
        let value = if found && !value.is_null() {
            // SAFETY: the ini entry's NUL-terminated value.
            unsafe { CStr::from_ptr(value) }.to_bytes()
        } else {
            b"strict"
        };
        let Some((on, mode)) = pooling(value) else {
            let message = c"Error in pdo_odbc.connection_pooling configuration. Value must be one of \"strict\", \"relaxed\", or \"off\"";
            // SAFETY: MINIT; E_CORE_ERROR bails out, raised from this outermost Rust frame.
            swoole_rs_zend::bailout::raise(unsafe {
                swoole_rs_zend::report(E_CORE_ERROR.cast_signed(), message)
            });
            return -1;
        };
        pdo_odbc_pool_on.store(on, Ordering::Relaxed);
        pdo_odbc_pool_mode.store(mode, Ordering::Relaxed);
        if on != SQL_CP_OFF {
            // SAFETY: the process-wide attribute takes its value in the pointer.
            unsafe {
                SQLSetEnvAttr(
                    std::ptr::null_mut::<std::ffi::c_void>() as SQLHENV,
                    SQL_ATTR_CONNECTION_POOLING,
                    on as SQLPOINTER,
                    0,
                )
            };
        }
        let constants = [
            (c"ODBC_ATTR_USE_CURSOR_LIBRARY", PDO_ATTR_DRIVER_SPECIFIC),
            (c"ODBC_ATTR_ASSUME_UTF8", PDO_ATTR_DRIVER_SPECIFIC + 1),
            // SQL_CUR_USE_IF_NEEDED, SQL_CUR_USE_DRIVER and SQL_CUR_USE_ODBC.
            (c"ODBC_SQL_USE_IF_NEEDED", 0),
            (c"ODBC_SQL_USE_DRIVER", 2),
            (c"ODBC_SQL_USE_ODBC", 1),
        ];
        for (name, value) in constants {
            // SAFETY: MINIT, PDO loaded.
            unsafe { swr_zend_pdo_declare_long(name.as_ptr(), value) };
        }
    }
    let driver = &raw const swoole_pdo_odbc_driver;
    // SAFETY: the driver is static; registering replaces pdo_odbc's own.
    unsafe { php_pdo_unregister_driver(driver) };
    // SAFETY: as above.
    unsafe { php_pdo_register_driver(driver) };
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pooling_values_match_upstream() {
        assert_eq!(
            pooling(b"STRICT"),
            Some((SQL_CP_ONE_PER_HENV, SQL_CP_STRICT_MATCH))
        );
        assert_eq!(
            pooling(b"1"),
            Some((SQL_CP_ONE_PER_HENV, SQL_CP_STRICT_MATCH))
        );
        assert_eq!(
            pooling(b"Relaxed"),
            Some((SQL_CP_ONE_PER_HENV, SQL_CP_RELAXED_MATCH))
        );
        assert_eq!(pooling(b"").map(|(on, _)| on), Some(SQL_CP_OFF));
        assert_eq!(pooling(b"oFF").map(|(on, _)| on), Some(SQL_CP_OFF));
        assert_eq!(pooling(b"yes"), None);
    }
}
