//! A protocol's `package_length_func`: the PHP callable its `private_data_1`
//! names, a reference `Rc::into_raw` gave the slot, or a [`Named`] owner beside
//! it keeps.
use std::{ffi::c_void, mem::ManuallyDrop, rc::Rc};
use swoole_rs_corebase::net_types::{PacketLength, Protocol};
use swoole_rs_zend::{bailout, host::callable::Callable};

/// A callable a protocol's `private_data_1` names: the reference `Rc::into_raw`
/// gave, which the owner gives back when it drops. The protocol only names it.
pub struct Named(*const Callable);

impl Named {
    #[must_use]
    pub fn new(callable: Rc<Callable>) -> Self {
        Self(Rc::into_raw(callable))
    }

    /// The pointer `private_data_1` keeps.
    #[must_use]
    pub fn as_ptr(&self) -> *mut c_void {
        self.0.cast_mut().cast()
    }
}

impl Drop for Named {
    fn drop(&mut self) {
        // SAFETY: the reference `new` took from `Rc::into_raw`, given back once.
        drop(unsafe { Rc::from_raw(self.0) });
    }
}

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
/// C++ calls it with a live protocol whose `private_data_1` is a reference
/// `Rc::into_raw` gave that the slot or its [`Named`] owner keeps, and a live
/// packet.
pub unsafe extern "C" fn length<L: PackageLength>(
    protocol: *const Protocol,
    _socket: *mut c_void,
    packet: &mut PacketLength,
) -> isize {
    // SAFETY: per the caller.
    let raw = unsafe { (*protocol).private_data_1 };
    // SAFETY: an `Rc::into_raw` reference its holder keeps, borrowed.
    let slot = ManuallyDrop::new(unsafe { Rc::from_raw(raw.cast_const().cast::<Callable>()) });
    let held = L::owns().then(|| Rc::clone(&slot));
    let length = L::length(&slot, packet.bytes());
    // Released first: `finish` may jump over this frame.
    drop(held);
    bailout::finish();
    length
}
