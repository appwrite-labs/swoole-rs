//! A protocol's `package_length_func`: the PHP callable of the [`Named`] owner a
//! client, a coroutine socket or this thread's port registry keeps, which a measure
//! hands the length callback.
use std::{any::Any, rc::Rc};
use swoole_rs_clients::protocol::{Protocol, Socket};
use swoole_rs_corebase::net_types::PacketLength;
use swoole_rs_zend::{bailout, host::callable::Callable};

/// A `package_length_func`'s owner: it holds the callable's reference, which goes
/// when the last reference to the owner drops.
pub struct Named(Rc<Callable>);

impl Named {
    #[must_use]
    pub fn new(callable: Rc<Callable>) -> Self {
        Self(callable)
    }
}

/// How a crate runs a protocol's length callable.
pub trait PackageLength {
    /// The packet length `callable` returns for `bytes` so far (0: not yet
    /// known), or `None` when it failed or returned a negative length, with any
    /// bailout deferred.
    fn length(callable: &Callable, bytes: &[u8]) -> Option<usize>;
}

/// `php_swoole_length_func`: `L`'s length for the packet so far, by the callable of
/// `owner`, a [`Named`] whose reference the caller hands over; -1 without one. A
/// bailout is re-raised as the last step.
pub fn length<L: PackageLength>(
    _protocol: &Protocol,
    _socket: Option<Socket<'_>>,
    owner: Option<Rc<dyn Any>>,
    packet: &mut PacketLength,
) -> isize {
    let Some(Ok(named)) = owner.map(<Rc<dyn Any>>::downcast::<Named>) else {
        return -1;
    };
    let length = L::length(&named.0, packet.bytes());
    // A callback may replace its own last stored reference.
    drop(named);
    if bailout::pending() {
        return -1;
    }
    length.map_or(-1, usize::cast_signed)
}
