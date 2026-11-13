//! MINIT for the bundled `pdo_odbc` driver (upstream's
//! `php_swoole_odbc_minit`): connection pooling from `pdo_odbc.connection_pooling`,
//! PDO's `ODBC_*` constants unless `pdo_odbc` declared them, and the driver.
use std::{
    ffi::{CStr, c_char, c_int},
    sync::atomic::{AtomicUsize, Ordering},
};
use swoole_rs_odbc_sys::{SQLHENV, SQLINTEGER, SQLPOINTER, SQLSetEnvAttr};
use swoole_rs_zend::{
    Minit,
    pdo::{Driver, php_pdo_register_driver},
    sys::E_CORE_ERROR,
};

unsafe extern "C" {
    /// The bundled driver (`thirdparty/php*/pdo_odbc/odbc_driver.c`).
    safe static swoole_pdo_odbc_driver: Driver;
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

/// `php_swoole_odbc_minit`: an invalid `pdo_odbc.connection_pooling` raises
/// `E_CORE_ERROR` and registers nothing (upstream's `FAILURE`, which its caller ignores).
/// Nothing unless PDO is loaded.
pub fn minit(minit: &Minit) {
    let Some(pdo) = minit.pdo() else {
        return;
    };
    if !pdo.declared(c"ODBC_ATTR_USE_CURSOR_LIBRARY") {
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
            // MINIT; E_CORE_ERROR bails out, raised from this outermost Rust frame.
            swoole_rs_zend::bailout::raise(swoole_rs_zend::report(
                E_CORE_ERROR.cast_signed(),
                message,
            ));
            return;
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
            pdo.declare_long(name, value);
        }
    }
    // Registering replaces pdo_odbc's own.
    pdo.unregister(&swoole_pdo_odbc_driver);
    // SAFETY: MINIT, with PDO loaded. PDO raises `E_ERROR` for a driver built
    // against another PDO API: no frame up to the step's `zend_try` owns a drop.
    unsafe { php_pdo_register_driver(&swoole_pdo_odbc_driver) };
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
