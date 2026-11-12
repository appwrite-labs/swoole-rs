//! `php_swoole_serialize` and `php_swoole_unserialize`. A bailout in
//! `__serialize`, `__sleep` or `__wakeup` is trapped in C and handed back for
//! the caller to raise once its frames are gone.
use std::ffi::{CStr, c_int};
use swoole_rs_zend::{
    Owned, Value,
    bailout::{self, Bailout},
    sys::swr_zend_unserialize,
};

/// `serialize($value)`'s bytes (`None` when it threw), and whether it bailed out.
pub fn serialize(value: &Value) -> (Option<Box<[u8]>>, Result<(), Bailout>) {
    let serialized = match value.serialize() {
        Ok(serialized) => serialized,
        Err(bailout) => return (None, Err(bailout)),
    };
    let threw = !swoole_rs_zend::host::exception().is_null();
    let bytes = serialized.filter(|_| !threw);
    (bytes.map(|bytes| bytes.bytes().into()), Ok(()))
}

/// `unserialize($text)`, as far as it parsed (a failure warns with its offset),
/// and whether it bailed out.
pub fn unserialize(text: &[u8]) -> (Owned, Result<(), Bailout>) {
    let (out, _, bailed) = unserialize_from(c"php_swoole_unserialize", text);
    (out, bailed)
}

/// [`unserialize`], whose failure warns as `origin`, and whether it parsed.
pub fn unserialize_from(origin: &CStr, text: &[u8]) -> (Owned, bool, Result<(), Bailout>) {
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
            origin,
            format_args!(
                "unserialize() failed, Error at offset {offset} of {} bytes",
                text.len()
            ),
        );
    }
    (out, done, bailout::trapped(c_int::from(bailed)))
}
