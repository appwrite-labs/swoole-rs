//! `network::Socket`'s timeout group and the operations over it.
use std::ffi::c_int;

/// Which of a socket's timeouts a call names: upstream's `SW_TIMEOUT_*` bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeoutKind(c_int);

impl TimeoutKind {
    pub const NONE: Self = Self(0);
    pub const DNS: Self = Self(1 << 0);
    pub const CONNECT: Self = Self(1 << 1);
    pub const READ: Self = Self(1 << 2);
    pub const WRITE: Self = Self(1 << 3);
    pub const RDWR: Self = Self(Self::READ.0 | Self::WRITE.0);
    pub const ALL: Self = Self(Self::DNS.0 | Self::CONNECT.0 | Self::RDWR.0);
    /// Each timeout on its own, in `network::Socket`'s order.
    pub const EACH: [Self; 4] = [Self::DNS, Self::CONNECT, Self::READ, Self::WRITE];

    /// The bits of upstream's `int`.
    #[must_use]
    pub const fn from_bits(bits: c_int) -> Self {
        Self(bits)
    }

    /// Whether it names any timeout `other` does.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl core::ops::BitOr for TimeoutKind {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

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
    pub fn set(&mut self, timeout: f64, kind: TimeoutKind) {
        if timeout == 0.0 {
            return;
        }
        for (bit, slot) in [
            (TimeoutKind::DNS, &mut self.dns),
            (TimeoutKind::CONNECT, &mut self.connect),
            (TimeoutKind::READ, &mut self.read),
            (TimeoutKind::WRITE, &mut self.write),
        ] {
            if kind.intersects(bit) {
                *slot = timeout;
            }
        }
    }

    /// `Socket::get_timeout`: the timeout `kind` names, unless it names other than one.
    #[must_use]
    pub fn get(&self, kind: TimeoutKind) -> Option<f64> {
        match kind {
            TimeoutKind::DNS => Some(self.dns),
            TimeoutKind::CONNECT => Some(self.connect),
            TimeoutKind::READ => Some(self.read),
            TimeoutKind::WRITE => Some(self.write),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SW_TIMEOUT_DNS: TimeoutKind = TimeoutKind::DNS;
    const SW_TIMEOUT_CONNECT: TimeoutKind = TimeoutKind::CONNECT;
    const SW_TIMEOUT_READ: TimeoutKind = TimeoutKind::READ;
    const SW_TIMEOUT_WRITE: TimeoutKind = TimeoutKind::WRITE;

    #[test]
    fn sets_and_gets_like_upstream() {
        let mut timeouts = SocketTimeouts {
            dns: 1.0,
            connect: 2.0,
            read: 3.0,
            write: 4.0,
        };
        timeouts.set(0.0, TimeoutKind::from_bits(15));
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
        timeouts.set(0.5, TimeoutKind::from_bits(15));
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
        assert_eq!(timeouts.get(TimeoutKind::from_bits(0)), None);
    }
}
