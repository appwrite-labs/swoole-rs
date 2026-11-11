//! `php_swoole_serialize` and `php_swoole_unserialize`. A bailout in
//! `__serialize`, `__sleep` or `__wakeup` is trapped in C and handed back for
//! the caller to raise once its frames are gone.
use std::ffi::c_int;
use swoole_rs_zend::{
    Owned, Value, ZString,
    bailout::{self, Bailout},
    sys::{swr_zend_facade_exception, swr_zend_serialize, swr_zend_unserialize},
};

/// `serialize($value)`'s bytes (`None` when it threw), and whether it bailed out.
pub fn serialize(value: &Value) -> (Option<Box<[u8]>>, Result<(), Bailout>) {
    let mut bailed = false;
    // SAFETY: a live zval, on the thread running PHP that holds it.
    let serialized = unsafe { swr_zend_serialize(value.as_ptr(), &raw mut bailed) };
    let mut result = None;
    if !bailed && !serialized.is_null() {
        // SAFETY: the new string serialize made, released on drop.
        let serialized = unsafe { ZString::from_raw(serialized) };
        // SAFETY: a pure read of EG(exception).
        if unsafe { swr_zend_facade_exception() }.is_null() {
            result = Some(serialized.bytes().into());
        }
    }
    (result, bailout::trapped(c_int::from(bailed)))
}

/// `unserialize($text)`, as far as it parsed (a failure warns with its offset),
/// and whether it bailed out.
pub fn unserialize(text: &[u8]) -> (Owned, Result<(), Bailout>) {
    let out = Owned::default();
    let (mut offset, mut bailed) = (0, false);
    // SAFETY: a slot this owns and the string's live bytes, on the thread running PHP.
    let done = unsafe {
        swr_zend_unserialize(
            out.as_ptr(),
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
    (out, bailout::trapped(c_int::from(bailed)))
}
