//! `php_swoole_serialize` and `php_swoole_unserialize`, under their upstream
//! symbols. A bailout in `__serialize`, `__sleep` or `__wakeup` is trapped in C
//! and raised again as each returns, as upstream let it through.
use std::{ffi::c_int, ptr};
use swoole_rs_zend::{
    bailout,
    string::bytes,
    sys::{
        swr_zend_facade_exception, swr_zend_serialize, swr_zend_string_persistent,
        swr_zend_string_release, swr_zend_unserialize, zend_string, zval,
    },
};

/// `zend_string *php_swoole_serialize(zval *zdata)`: `serialize($zdata)` as a
/// persistent string, or null when it threw.
/// # Safety
/// `value` is a live zval, on a thread running PHP.
#[unsafe(export_name = "_Z20php_swoole_serializeP12_zval_struct")]
pub unsafe extern "C" fn php_swoole_serialize(value: *mut zval) -> *mut zend_string {
    let mut bailed = false;
    // SAFETY: per the caller.
    let serialized = unsafe { swr_zend_serialize(value, &raw mut bailed) };
    let mut result = ptr::null_mut();
    if !bailed && !serialized.is_null() {
        // SAFETY: a pure read of EG(exception).
        if unsafe { swr_zend_facade_exception() }.is_null() {
            // SAFETY: the string serialize made.
            let text = unsafe { bytes(serialized) };
            // SAFETY: persistent memory, owned by the caller.
            result = unsafe { swr_zend_string_persistent(text.as_ptr().cast(), text.len()) };
        }
        // SAFETY: released once.
        unsafe { swr_zend_string_release(serialized) };
    }
    bailout::raise(bailout::trapped(c_int::from(bailed)));
    result
}

/// `bool php_swoole_unserialize(const zend_string *data, zval *zv)`: whether
/// `unserialize($data)` into `zv` succeeded; a failure warns with its offset.
/// # Safety
/// `data` is a live string and `out` a writable zval, on a thread running PHP.
#[unsafe(export_name = "_Z22php_swoole_unserializePK12_zend_stringP12_zval_struct")]
pub unsafe extern "C" fn php_swoole_unserialize(data: *const zend_string, out: *mut zval) -> bool {
    // SAFETY: per the caller.
    let text = unsafe { bytes(data) };
    let (mut offset, mut bailed) = (0, false);
    // SAFETY: per the caller, with the string's live bytes.
    let done = unsafe {
        swr_zend_unserialize(
            out,
            text.as_ptr().cast(),
            text.len(),
            &raw mut offset,
            &raw mut bailed,
        )
    };
    if !bailed && !done {
        swoole_rs_corebase::log::warning(
            c"php_swoole_unserialize",
            format_args!(
                "unserialize() failed, Error at offset {offset} of {} bytes",
                text.len()
            ),
        );
    }
    bailout::raise(bailout::trapped(c_int::from(bailed)));
    done
}
