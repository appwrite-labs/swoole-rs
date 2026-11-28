//! PHP packet-length callbacks shared by clients, coroutine sockets and server ports.
use std::{any::Any, rc::Rc};
use swoole_rs_clients::protocol::Protocol;
use swoole_rs_corebase::net_types::PacketLength;
use swoole_rs_zend::{bailout, host::callable::Callable};

/// How a crate runs a protocol's length callable.
pub trait PackageLength {
    /// The packet length `callable` returns for `bytes` so far (0: not yet
    /// known), or `None` when it failed or returned a negative length, with any
    /// bailout deferred.
    fn length(callable: &Callable, bytes: &[u8]) -> Option<usize>;
}

/// `php_swoole_length_func`: `L`'s length for the packet so far, by the callable of
/// `owner`, whose reference the caller hands over; `None` without one. A
/// bailout is re-raised as the last step.
pub fn length<L: PackageLength>(
    _protocol: &Protocol,
    _: Option<u8>,
    owner: Option<Rc<dyn Any>>,
    packet: &mut PacketLength,
) -> Option<usize> {
    let callable = owner?.downcast::<Callable>().ok()?;
    let length = L::length(&callable, packet.bytes());
    // A callback may replace its own last stored reference.
    drop(callable);
    length.filter(|_| !bailout::pending())
}
