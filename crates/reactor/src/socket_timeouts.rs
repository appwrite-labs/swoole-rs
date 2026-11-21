//! `network::Socket`'s timeout group and the operations over it.
use std::ffi::c_int;

const SW_TIMEOUT_DNS: c_int = 1 << 0;
const SW_TIMEOUT_CONNECT: c_int = 1 << 1;
const SW_TIMEOUT_READ: c_int = 1 << 2;
const SW_TIMEOUT_WRITE: c_int = 1 << 3;

/// `dns_timeout`, `connect_timeout`, `read_timeout` and `write_timeout`, in the
/// order and types `network::Socket` embeds them (asserted natively).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SocketTimeouts {
    pub dns: f64,
    pub connect: f64,
    pub read: f64,
    pub write: f64,
}

impl SocketTimeouts {
    /// `Socket::set_timeout`: `timeout` into every timeout `kind`'s bits name. A zero
    /// timeout changes nothing.
    pub fn set(&mut self, timeout: f64, kind: c_int) {
        if timeout == 0.0 {
            return;
        }
        for (bit, slot) in [
            (SW_TIMEOUT_DNS, &mut self.dns),
            (SW_TIMEOUT_CONNECT, &mut self.connect),
            (SW_TIMEOUT_READ, &mut self.read),
            (SW_TIMEOUT_WRITE, &mut self.write),
        ] {
            if kind & bit != 0 {
                *slot = timeout;
            }
        }
    }

    /// `Socket::get_timeout`: the timeout `kind` names, unless it names other than one.
    #[must_use]
    pub fn get(&self, kind: c_int) -> Option<f64> {
        match kind {
            SW_TIMEOUT_DNS => Some(self.dns),
            SW_TIMEOUT_CONNECT => Some(self.connect),
            SW_TIMEOUT_READ => Some(self.read),
            SW_TIMEOUT_WRITE => Some(self.write),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_and_gets_like_upstream() {
        let mut timeouts = SocketTimeouts {
            dns: 1.0,
            connect: 2.0,
            read: 3.0,
            write: 4.0,
        };
        timeouts.set(0.0, 15);
        assert_eq!(
            timeouts,
            SocketTimeouts {
                dns: 1.0,
                connect: 2.0,
                read: 3.0,
                write: 4.0
            }
        );
        timeouts.set(-1.0, SW_TIMEOUT_READ | SW_TIMEOUT_WRITE);
        assert_eq!(
            timeouts,
            SocketTimeouts {
                dns: 1.0,
                connect: 2.0,
                read: -1.0,
                write: -1.0
            }
        );
        timeouts.set(0.5, 15);
        assert_eq!(
            timeouts,
            SocketTimeouts {
                dns: 0.5,
                connect: 0.5,
                read: 0.5,
                write: 0.5
            }
        );
        timeouts.set(7.0, SW_TIMEOUT_CONNECT);
        assert_eq!(timeouts.get(SW_TIMEOUT_DNS), Some(0.5));
        assert_eq!(timeouts.get(SW_TIMEOUT_CONNECT), Some(7.0));
        assert_eq!(timeouts.get(SW_TIMEOUT_WRITE), Some(0.5));
        assert_eq!(timeouts.get(SW_TIMEOUT_READ | SW_TIMEOUT_WRITE), None);
        assert_eq!(timeouts.get(0), None);
    }
}
