//! `php_swoole_serialize` and `php_swoole_unserialize`. A bailout in
//! `__serialize`, `__sleep` or `__wakeup` is trapped in C and handed back for
//! the caller to raise once its frames are gone.
use std::{ffi::c_int, ptr};
use swoole_rs_zend::{
    Value, ZString,
    bailout::{self, Bailout},
    sys::{
        swr_zend_facade_exception, swr_zend_serialize, swr_zend_string_persistent,
        swr_zend_unserialize, zend_string, zval,
    },
};

/// `serialize($value)` as a persistent string (null when it threw), and
/// whether it bailed out.
pub fn serialize(value: &Value) -> (*mut zend_string, Result<(), Bailout>) {
    let mut bailed = false;
    // SAFETY: a live zval, on the thread running PHP that holds it.
    let serialized = unsafe { swr_zend_serialize(value.as_ptr(), &raw mut bailed) };
    let mut result = ptr::null_mut();
    if !bailed && !serialized.is_null() {
        // SAFETY: the new string serialize made, released on drop.
        let serialized = unsafe { ZString::from_raw(serialized) };
        // SAFETY: a pure read of EG(exception).
        if unsafe { swr_zend_facade_exception() }.is_null() {
            let text = serialized.bytes();
            // SAFETY: persistent memory, owned by the caller.
            result = unsafe { swr_zend_string_persistent(text.as_ptr().cast(), text.len()) };
        }
    }
    (result, bailout::trapped(c_int::from(bailed)))
}

/// `unserialize($text)` into `out`: whether it succeeded (a failure warns with
/// its offset), and whether it bailed out.
/// # Safety
/// `out` is a writable zval, on a thread running PHP.
pub unsafe fn unserialize(text: &[u8], out: *mut zval) -> (bool, Result<(), Bailout>) {
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
    (done, bailout::trapped(c_int::from(bailed)))
}
