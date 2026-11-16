//! The stream transports `enable_hook` swaps (upstream's `ori_factory`): the
//! factories PHP had registered at RINIT, which hooking replaces with C++'s
//! coroutine `socket_create` and unhooking puts back, and which that factory
//! falls back to outside a coroutine.
use std::{
    ffi::{CStr, c_int, c_void},
    io::IoSlice,
    ptr::NonNull,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_corebase::sys::Text;

/// A `php_stream_transport_factory`: one PHP's transport table held, or
/// zend-sys's coroutine one ([`Factory::coroutine`]). Only those make one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub(crate) struct Factory(NonNull<c_void>);

impl Factory {
    /// zend-sys's coroutine transport factory (a function's address, never null).
    pub(crate) fn coroutine() -> Option<Self> {
        NonNull::new(swoole_rs_zend::sys::swr_zend_stream_factory as *mut c_void).map(Self)
    }

    /// The factory, as the zend-sys shim that calls it takes it.
    pub(crate) fn as_ptr(self) -> *mut c_void {
        self.0.as_ptr()
    }
}

unsafe extern "C" {
    /// The stream transport factory registered as `name`, read from the lent
    /// bytes (an `IoSlice` is an iovec), or none: a lookup in PHP's table.
    safe fn swr_zend_runtime_transport(name: IoSlice<'_>) -> Option<Factory>;
    /// `php_stream_xport_register(name, factory)`, or with none
    /// `php_stream_xport_unregister(name)`: 0 for `SUCCESS`.
    safe fn swr_zend_runtime_set_transport(name: Text<'_>, factory: Option<Factory>) -> c_int;
}

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

/// MINIT: register the coroutine factory as `async.tcp` .. `async.tls`.
pub(crate) fn register_async() {
    for name in ASYNC {
        set(name, Factory::coroutine());
    }
}

/// Register `factory` as transport `name`, or with none unregister it: whether
/// PHP reported `SUCCESS`.
fn set(name: &CStr, factory: Option<Factory>) -> bool {
    swr_zend_runtime_set_transport(name.into(), factory) == 0
}

/// The factory a slot of [`ORIGINAL`] holds: only [`remember`] stores one.
fn load(slot: &AtomicPtr<c_void>) -> Option<Factory> {
    NonNull::new(slot.load(Ordering::Relaxed)).map(Factory)
}

/// RINIT, on the main thread: remember what PHP registered.
pub(crate) fn remember() {
    for (slot, name) in ORIGINAL.iter().zip(NAMES) {
        let factory = swr_zend_runtime_transport(IoSlice::new(name.to_bytes()));
        slot.store(
            factory.map_or(std::ptr::null_mut(), Factory::as_ptr),
            Ordering::Relaxed,
        );
    }
}

/// Register the coroutine factory (`hook`) or the original one for transport
/// `index`; one PHP never had is unregistered. Whether PHP took it.
pub(crate) fn register(index: usize, hook: bool) -> bool {
    let (Some(slot), Some(name)) = (ORIGINAL.get(index), NAMES.get(index)) else {
        return false;
    };
    set(
        name,
        if hook {
            Factory::coroutine()
        } else {
            load(slot)
        },
    )
}

/// The original, process-lived PHP factory for an exact borrowed protocol name,
/// or none: `socket_create` outside a coroutine.
pub(crate) fn original(proto: &[u8]) -> Option<Factory> {
    NAMES
        .iter()
        .zip(&ORIGINAL)
        .find(|(name, _)| name.to_bytes() == proto)
        .and_then(|(_, slot)| load(slot))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_matches_whole_names_only() {
        let mut marker = 0u8;
        ORIGINAL[4].store((&raw mut marker).cast(), Ordering::Relaxed);
        let find = original;
        assert_eq!(
            find(b"ssl").map(Factory::as_ptr),
            Some((&raw mut marker).cast())
        );
        assert!(find(b"ss").is_none());
        assert!(find(b"sslv3").is_none());
        assert!(find(b"tcp").is_none());
        ORIGINAL[4].store(std::ptr::null_mut(), Ordering::Relaxed);
    }
}
