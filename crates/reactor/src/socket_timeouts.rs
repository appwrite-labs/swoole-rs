//! `network::Socket`'s timeout group and the operations over it.
use std::ffi::c_int;
use std::ptr;

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

/// `Socket::set_timeout`: `timeout` into every timeout `kind`'s bits name. A zero
/// timeout changes nothing.
///
/// # Safety
/// `timeouts` is a live, writable timeout group.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_set_timeout(
    timeouts: *mut SocketTimeouts,
    timeout: f64,
    kind: c_int,
) {
    if timeout == 0.0 {
        return;
    }
    // SAFETY: as the caller promises.
    let timeouts = unsafe { &mut *timeouts };
    for (bit, slot) in [
        (SW_TIMEOUT_DNS, &mut timeouts.dns),
        (SW_TIMEOUT_CONNECT, &mut timeouts.connect),
        (SW_TIMEOUT_READ, &mut timeouts.read),
        (SW_TIMEOUT_WRITE, &mut timeouts.write),
    ] {
        if kind & bit != 0 {
            *slot = timeout;
        }
    }
}

/// `Socket::get_timeout`: the timeout `kind` names, or null unless it names exactly one.
///
/// # Safety
/// `timeouts` is a live timeout group; the result points into it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_timeout(
    timeouts: *const SocketTimeouts,
    kind: c_int,
) -> *const f64 {
    // SAFETY: as the caller promises.
    let timeouts = unsafe { &*timeouts };
    match kind {
        SW_TIMEOUT_DNS => &raw const timeouts.dns,
        SW_TIMEOUT_CONNECT => &raw const timeouts.connect,
        SW_TIMEOUT_READ => &raw const timeouts.read,
        SW_TIMEOUT_WRITE => &raw const timeouts.write,
        _ => ptr::null(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(timeouts: &SocketTimeouts, kind: c_int) -> Option<f64> {
        unsafe { swr_v1_socket_timeout(timeouts, kind).as_ref().copied() }
    }

    #[test]
    fn sets_and_gets_like_upstream() {
        let mut timeouts = SocketTimeouts {
            dns: 1.0,
            connect: 2.0,
            read: 3.0,
            write: 4.0,
        };
        unsafe { swr_v1_socket_set_timeout(&raw mut timeouts, 0.0, 15) };
        assert_eq!(
            timeouts,
            SocketTimeouts {
                dns: 1.0,
                connect: 2.0,
                read: 3.0,
                write: 4.0
            }
        );
        unsafe {
            swr_v1_socket_set_timeout(&raw mut timeouts, -1.0, SW_TIMEOUT_READ | SW_TIMEOUT_WRITE);
        };
        assert_eq!(
            timeouts,
            SocketTimeouts {
                dns: 1.0,
                connect: 2.0,
                read: -1.0,
                write: -1.0
            }
        );
        unsafe { swr_v1_socket_set_timeout(&raw mut timeouts, 0.5, 15) };
        assert_eq!(
            timeouts,
            SocketTimeouts {
                dns: 0.5,
                connect: 0.5,
                read: 0.5,
                write: 0.5
            }
        );
        unsafe { swr_v1_socket_set_timeout(&raw mut timeouts, 7.0, SW_TIMEOUT_CONNECT) };
        assert_eq!(get(&timeouts, SW_TIMEOUT_DNS), Some(0.5));
        assert_eq!(get(&timeouts, SW_TIMEOUT_CONNECT), Some(7.0));
        assert_eq!(get(&timeouts, SW_TIMEOUT_WRITE), Some(0.5));
        assert_eq!(get(&timeouts, SW_TIMEOUT_READ | SW_TIMEOUT_WRITE), None);
        assert_eq!(get(&timeouts, 0), None);
    }
}
