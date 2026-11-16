//! `php_swoole_serialize` and `php_swoole_unserialize`. A bailout in
//! `__serialize`, `__sleep` or `__wakeup` is trapped in C and handed back for
//! the caller to raise once its frames are gone.
use std::{
    ffi::{CStr, c_int},
    io::IoSlice,
};
use swoole_rs_zend::{
    Owned, Value,
    bailout::{self, Bailout},
};

unsafe extern "C" {
    /// `php_var_unserialize` of the lent bytes (an `IoSlice` is an iovec) into
    /// `out`, which it writes without reading, inside `zend_try` (`__wakeup` and
    /// `__unserialize` run PHP): whether it succeeded, `offset` where it stopped,
    /// and `bailed` set for a bailout.
    safe fn swr_zend_unserialize(
        out: &Owned,
        text: IoSlice<'_>,
        offset: &mut usize,
        bailed: &mut bool,
    ) -> bool;
}

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
    let done = swr_zend_unserialize(&out, IoSlice::new(text), &mut offset, &mut bailed);
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
