//! Scoped coroutine socket state: each scope saves what it changes and restores it
//! when dropped. A scope borrows its socket and never owns or destroys it; the
//! socket outlives the scope.
use crate::co_socket::{Co, Protocol};
use core::ffi::c_int;

/// `Socket::ProtocolSwitch`: the socket's EOF and length checks and protocol,
/// restored when dropped.
pub struct ProtocolScope {
    socket: Co,
    eof: bool,
    length: bool,
    protocol: Protocol,
}

impl ProtocolScope {
    #[must_use]
    pub fn new(socket: Co) -> Self {
        socket.with(|record| Self {
            socket,
            eof: record.open_eof_check,
            length: record.open_length_check,
            protocol: record.protocol,
        })
    }
}

impl Drop for ProtocolScope {
    fn drop(&mut self) {
        self.socket.with(|record| {
            record.open_eof_check = self.eof;
            record.open_length_check = self.length;
            record.protocol = self.protocol;
        });
    }
}

/// `Socket::TimeoutSetter`: `timeout` for the timeouts `kind` selects, each restored
/// when dropped. A zero timeout changes nothing.
pub struct TimeoutScope {
    socket: Co,
    timeout: f64,
    kind: c_int,
    original: [f64; 4],
    /// `TimeoutController`'s clock, 0 until [`TimeoutScope::timed_out`] starts it.
    startup: f64,
}

/// The timeout bits, in `original`'s order.
const BITS: [c_int; 4] = [1, 2, 4, 8];

impl TimeoutScope {
    #[must_use]
    #[expect(
        clippy::float_cmp,
        reason = "the original compares saved timeouts exactly"
    )]
    pub fn new(socket: Co, timeout: f64, kind: c_int) -> Self {
        let mut original = [0.0; 4];
        if timeout != 0.0 {
            socket.net(|network| {
                for (original, bit) in original.iter_mut().zip(BITS) {
                    if kind & bit != 0 {
                        let value = network.timeouts.get(bit).unwrap_or_default();
                        *original = value;
                        if timeout != value {
                            network.timeouts.set(timeout, bit);
                        }
                    }
                }
            });
        }
        Self {
            socket,
            timeout,
            kind,
            original,
            startup: 0.0,
        }
    }

    /// `TimeoutController::has_timedout(kind)` at `now`: the first call starts the
    /// clock; later ones shrink the socket's timeout to what is left, or set
    /// `ETIMEDOUT` once none is.
    pub fn timed_out(&mut self, kind: c_int, now: f64) -> bool {
        crate::co_io::timed_out(self.socket, self.timeout, &mut self.startup, now, kind)
    }
}

impl Drop for TimeoutScope {
    #[expect(
        clippy::float_cmp,
        reason = "the original compares saved timeouts exactly"
    )]
    fn drop(&mut self) {
        if self.timeout == 0.0 {
            return;
        }
        self.socket.net(|network| {
            for (original, bit) in self.original.into_iter().zip(BITS) {
                if self.kind & bit != 0 && self.timeout != original {
                    network.timeouts.set(original, bit);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::co_socket::TestSocket;

    /// `rust_socket_scopes.protocol_snapshot_and_copy_restore`, formerly the hooks
    /// system fixture's C++ witness over `ProtocolSwitch`, less the C++ copy.
    #[test]
    fn protocol_snapshot_and_restore() {
        let mut socket = TestSocket::new();
        socket.0.open_eof_check = true;
        socket.0.open_length_check = false;
        socket.0.protocol.package_max_length = 117;
        socket.0.protocol.package_eof_len = 2;
        {
            let _scope = ProtocolScope::new(socket.co());
            socket.0.open_eof_check = false;
            socket.0.open_length_check = true;
            socket.0.protocol.package_max_length = 1;
            socket.0.protocol.package_eof_len = 1;
        }
        assert!(socket.0.open_eof_check);
        assert!(!socket.0.open_length_check);
        assert_eq!(socket.0.protocol.package_max_length, 117);
        assert_eq!(socket.0.protocol.package_eof_len, 2);
        {
            let _first = ProtocolScope::new(socket.co());
            socket.0.protocol.package_max_length = 222;
            {
                let _nested = ProtocolScope::new(socket.co());
                socket.0.protocol.package_max_length = 333;
            }
            assert_eq!(socket.0.protocol.package_max_length, 222);
        }
        assert_eq!(socket.0.protocol.package_max_length, 117);
    }
}
