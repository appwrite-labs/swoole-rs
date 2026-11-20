//! Literal quoting and SQLSTATE selection for the bundled PDO SQLite driver.
use std::ffi::c_int;
use swoole_rs_corebase::sys::Text;
use swoole_rs_zend::{string::ZBuf, sys::zend_string};

#[unsafe(no_mangle)]
extern "C" fn pdo_sqlite_sqlstate(code: c_int, state: &mut [u8; 6]) {
    *state = *match code {
        0 => b"00000\0",  // SQLITE_OK
        9 => b"01002\0",  // SQLITE_INTERRUPT
        12 => b"42S02\0", // SQLITE_NOTFOUND
        18 => b"22001\0", // SQLITE_TOOBIG
        19 => b"23000\0", // SQLITE_CONSTRAINT
        22 => b"HYC00\0", // SQLITE_NOLFS
        _ => b"HY000\0",
    };
}

/// The caller checks SQLite's length limit and handles PHP 8.5's NUL diagnostic.
/// Earlier PHP versions quote the bytes before the first NUL, as SQLite's %q does.
#[unsafe(no_mangle)]
extern "C" fn pdo_sqlite_quote_literal(value: Text<'_>) -> *mut zend_string {
    let bytes = value.get().to_bytes();
    let mut parts = bytes.split(|&byte| byte == b'\'');
    let first = parts.next().unwrap_or_default();
    let mut result = ZBuf::with_capacity(bytes.len() + parts.clone().count() + 2);
    result.extend_from_slice(b"'");
    result.extend_from_slice(first);
    for part in parts {
        result.extend_from_slice(b"''");
        result.extend_from_slice(part);
    }
    result.extend_from_slice(b"'");
    result.into_zstring().into_raw()
}
