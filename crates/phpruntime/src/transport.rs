//! The stream transports `enable_hook` swaps (upstream's `ori_factory`): the
//! factories PHP had registered at RINIT, which hooking replaces with C++'s
//! coroutine `socket_create` and unhooking puts back, and which that factory
//! falls back to outside a coroutine.
use std::{
    ffi::{CStr, c_void},
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_zend::sys::{swr_zend_runtime_set_transport, swr_zend_runtime_transport};

/// The transports, in `HOOK_TCP` .. `HOOK_TLS` order.
pub(crate) const NAMES: [&CStr; 6] = [c"tcp", c"udp", c"unix", c"udg", c"ssl", c"tls"];

/// The names MINIT gives the coroutine factory for good.
const ASYNC: [&CStr; 6] = [
    c"async.tcp",
    c"async.udp",
    c"async.unix",
    c"async.udg",
    c"async.ssl",
    c"async.tls",
];

static ORIGINAL: [AtomicPtr<c_void>; 6] = [const { AtomicPtr::new(std::ptr::null_mut()) }; 6];

/// MINIT: register `coroutine` as `async.tcp` .. `async.tls`.
pub(crate) fn register_async(coroutine: *mut c_void) {
    for name in ASYNC {
        set(name, coroutine, false);
    }
}

/// Register `factory` as transport `name`, or with `remove` unregister it.
/// `SUCCESS` or `FAILURE`.
fn set(name: &CStr, factory: *mut c_void, remove: bool) -> i32 {
    // SAFETY: a NUL-terminated name and a factory (or null) of PHP's or the
    // coroutine one.
    unsafe { swr_zend_runtime_set_transport(name.as_ptr(), factory, remove) }
}

/// RINIT, on the main thread: remember what PHP registered.
pub(crate) fn remember() {
    for (slot, name) in ORIGINAL.iter().zip(NAMES) {
        // SAFETY: a name and its length.
        let factory = unsafe { swr_zend_runtime_transport(name.as_ptr(), name.count_bytes()) };
        slot.store(factory, Ordering::Relaxed);
    }
}

/// Register `coroutine` (`hook`) or the original factory for transport `index`;
/// an ssl or tls transport PHP never had is unregistered. `SUCCESS` or `FAILURE`.
pub(crate) fn register(index: usize, hook: bool, coroutine: *mut c_void) -> i32 {
    let (Some(slot), Some(name)) = (ORIGINAL.get(index), NAMES.get(index)) else {
        return -1;
    };
    let original = slot.load(Ordering::Relaxed);
    let (factory, remove) = if hook {
        (coroutine, false)
    } else {
        (original, index >= 4 && original.is_null())
    };
    set(name, factory, remove)
}

/// The original, process-lived PHP factory for an exact borrowed protocol name,
/// or null: `socket_create` outside a coroutine.
pub(crate) fn original(proto: &[u8]) -> *mut c_void {
    NAMES
        .iter()
        .zip(&ORIGINAL)
        .find(|(name, _)| name.to_bytes() == proto)
        .map_or(std::ptr::null_mut(), |(_, slot)| {
            slot.load(Ordering::Relaxed)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_matches_whole_names_only() {
        let mut marker = 0u8;
        ORIGINAL[4].store((&raw mut marker).cast(), Ordering::Relaxed);
        let find = original;
        assert_eq!(find(b"ssl"), (&raw mut marker).cast());
        assert!(find(b"ss").is_null());
        assert!(find(b"sslv3").is_null());
        assert!(find(b"tcp").is_null());
        ORIGINAL[4].store(std::ptr::null_mut(), Ordering::Relaxed);
    }
}
