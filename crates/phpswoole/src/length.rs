//! A protocol's `package_length_func`: the PHP callable its `private_data_1`
//! names, an `Rc` pointer whose reference the slot (`Rc::into_raw`) or an
//! owner beside it (`Rc::as_ptr`) keeps.
use std::{ffi::c_void, mem::ManuallyDrop, rc::Rc};
use swoole_rs_corebase::net_types::{PacketLength, Protocol};
use swoole_rs_zend::{bailout, host::callable::Callable};

/// How a crate runs a protocol's length callable.
pub trait PackageLength {
    /// Whether this thread owns the slot's count, so the call takes its own
    /// reference and a `set()` the callable runs drops only the slot's.
    #[must_use]
    fn owns() -> bool {
        true
    }

    /// The packet length `callable` returns for `bytes` so far, or -1, with
    /// any bailout deferred.
    fn length(callable: &Callable, bytes: &[u8]) -> isize;
}

/// `php_swoole_length_func`: `L`'s length for the packet so far. A bailout is
/// re-raised as the last step.
/// # Safety
/// C++ calls it with a live protocol whose `private_data_1` names a callable
/// that a live reference keeps (the slot's or its owner's), and a live packet.
pub unsafe extern "C" fn length<L: PackageLength>(
    protocol: *const Protocol,
    _socket: *mut c_void,
    packet: &mut PacketLength,
) -> isize {
    // SAFETY: per the caller.
    let raw = unsafe { (*protocol).private_data_1 };
    // SAFETY: a callable a live reference keeps, borrowed: its holder keeps it.
    let slot = ManuallyDrop::new(unsafe { Rc::from_raw(raw.cast_const().cast::<Callable>()) });
    let held = L::owns().then(|| Rc::clone(&slot));
    let length = L::length(&slot, packet.bytes());
    // Released first: `finish` may jump over this frame.
    drop(held);
    bailout::finish();
    length
}
