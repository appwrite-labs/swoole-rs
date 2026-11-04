//! The stream transports `enable_hook` swaps (upstream's `ori_factory`): the
//! factories PHP had registered at RINIT, which hooking replaces with C++'s
//! coroutine `socket_create` and unhooking puts back, and which that factory
//! falls back to outside a coroutine.
use std::{
    ffi::{CStr, c_char, c_void},
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_zend::sys::{swr_zend_runtime_set_transport, swr_zend_runtime_transport};

/// The transports, in `HOOK_TCP` .. `HOOK_TLS` order.
pub(crate) const NAMES: [&CStr; 6] = [c"tcp", c"udp", c"unix", c"udg", c"ssl", c"tls"];

static ORIGINAL: [AtomicPtr<c_void>; 6] = [const { AtomicPtr::new(std::ptr::null_mut()) }; 6];

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
    let original = ORIGINAL[index].load(Ordering::Relaxed);
    let (factory, remove) = if hook {
        (coroutine, false)
    } else {
        (original, index >= 4 && original.is_null())
    };
    // SAFETY: a transport name and a factory (or null) of PHP's or C++'s.
    unsafe { swr_zend_runtime_set_transport(NAMES[index].as_ptr(), factory, remove) }
}

/// The factory PHP had registered for `proto`, or null (C++ then warns of an
/// unknown protocol): `socket_create` outside a coroutine.
///
/// # Safety
/// `proto` points to `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpruntime_original_transport(
    proto: *const c_char,
    length: usize,
) -> *mut c_void {
    // SAFETY: the caller's bytes.
    let proto = unsafe { std::slice::from_raw_parts(proto.cast::<u8>(), length) };
    NAMES
        .iter()
        .position(|name| name.to_bytes() == proto)
        .map_or(std::ptr::null_mut(), |index| {
            ORIGINAL[index].load(Ordering::Relaxed)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_matches_whole_names_only() {
        let mut marker = 0u8;
        ORIGINAL[4].store((&raw mut marker).cast(), Ordering::Relaxed);
        let find = |proto: &[u8]| unsafe {
            swr_v1_phpruntime_original_transport(proto.as_ptr().cast(), proto.len())
        };
        assert_eq!(find(b"ssl"), (&raw mut marker).cast());
        assert!(find(b"ss").is_null());
        assert!(find(b"sslv3").is_null());
        assert!(find(b"tcp").is_null());
        ORIGINAL[4].store(std::ptr::null_mut(), Ordering::Relaxed);
    }
}
