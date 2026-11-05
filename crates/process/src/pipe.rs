//! Canonical pipe-pair state and control over native socket loans. Reads and
//! blocking waits remain native entrypoints until composed callback guards are
//! qualified; close/configuration effects cannot cross a Rust frame by unwinding.
use core::ffi::{c_int, c_void};

#[repr(C)]
pub struct SocketPairRecord {
    pub blocking: bool,
    pub timeout: f64,
    pub socks: [c_int; 2],
    pub master_socket: *mut c_void,
    pub worker_socket: *mut c_void,
}

/// # Safety
/// `pair` is fresh writable storage for one canonical record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_socket_pair_init(
    pair: *mut SocketPairRecord,
    blocking: bool,
    timeout: f64,
) {
    let state = SocketPairRecord {
        blocking,
        timeout,
        socks: [0; 2],
        master_socket: core::ptr::null_mut(),
        worker_socket: core::ptr::null_mut(),
    };
    // SAFETY: the constructor exclusively owns this fresh canonical record.
    unsafe { pair.write(state) };
}

/// Create the descriptors in their original read/write order. `unix` selects
/// `AF_UNIX` socketpair with the exact protocol, including invalid negative values.
/// # Safety
/// `pair` is live and its descriptor slots are exclusively loaned for creation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_socket_pair_open(
    pair: *mut SocketPairRecord,
    unix: bool,
    protocol: c_int,
) -> c_int {
    // SAFETY: the caller lends the writable two-descriptor array.
    let sockets = unsafe { &raw mut (*pair).socks }.cast::<c_int>();
    if unix {
        // SAFETY: AF_UNIX socketpair writes exactly the same two descriptors.
        unsafe { libc::socketpair(libc::AF_UNIX, protocol, 0, sockets) }
    } else {
        // SAFETY: the output has room for the two descriptors.
        unsafe { libc::pipe(sockets) }
    }
}

/// One fixed nonlogging native effect: 2 set buffer size, 3 set timeout.
/// Returns 1 success or 0 a kernel failure. It cannot invoke user code or unwind.
pub type SocketPairEffect = unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u64) -> c_int;

unsafe fn socket(pair: *mut SocketPairRecord, master: bool) -> *mut c_void {
    if master {
        // SAFETY: the pair remains live; this leaf borrow ends before effects.
        unsafe { (*pair).master_socket }
    } else {
        // SAFETY: the same lifetime and leaf-borrow rule for the worker endpoint.
        unsafe { (*pair).worker_socket }
    }
}

/// Configure buffer size or timeout through fixed, nonlogging socket methods.
/// Blocking changes, diagnostics and resource destruction retain native entrypoints.
/// # Safety
/// `pair` is live and synchronized as its original native methods require.
/// `effect` is the fixed nonlogging socket configuration leaf: no user code,
/// exceptions, diagnostics, cancellation point or whole-thread exit. No Rust
/// borrow is held over a native effect.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_socket_pair_control(
    pair: *mut SocketPairRecord,
    operation: u32,
    argument: u64,
    context: *mut c_void,
    effect: SocketPairEffect,
) -> c_int {
    if !(2..=3).contains(&operation) {
        return -1;
    }
    if operation == 3 {
        // SAFETY: native timeout assignment precedes both endpoint updates.
        unsafe { (*pair).timeout = f64::from_bits(argument) };
    }
    for master in [true, false] {
        // SAFETY: each endpoint loan ends before the native method can reenter.
        let pointer = unsafe { socket(pair, master) };
        // SAFETY: native methods receive the original endpoint, including original
        // null-pointer preconditions; no callback can unwind through this frame.
        let result = unsafe { effect(context, pointer, operation, argument) };
        if result < 0 || (operation == 2 && result == 0) {
            return result;
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_descriptors_preserve_pipe_direction_and_socket_pair_protocol() {
        let mut pair = SocketPairRecord {
            blocking: false,
            timeout: 0.0,
            socks: [0; 2],
            master_socket: core::ptr::null_mut(),
            worker_socket: core::ptr::null_mut(),
        };
        unsafe {
            swr_v1_process_socket_pair_init(&raw mut pair, true, 3.25);
        }
        assert!(pair.blocking);
        assert_eq!(pair.timeout, 3.25);
        assert!(pair.master_socket.is_null() && pair.worker_socket.is_null());
        for (unix, protocol) in [
            (false, 0),
            (true, libc::SOCK_STREAM),
            (true, libc::SOCK_DGRAM),
        ] {
            assert_eq!(
                unsafe { swr_v1_process_socket_pair_open(&raw mut pair, unix, protocol) },
                0
            );
            let sent = *b"wire";
            let mut received = [0_u8; 4];
            assert_eq!(
                unsafe { libc::write(pair.socks[1], sent.as_ptr().cast(), sent.len()) },
                4
            );
            assert_eq!(
                unsafe { libc::read(pair.socks[0], received.as_mut_ptr().cast(), received.len()) },
                4
            );
            assert_eq!(received, sent);
            for fd in pair.socks {
                assert_eq!(unsafe { libc::close(fd) }, 0);
            }
        }
        assert!(
            unsafe { swr_v1_process_socket_pair_open(&raw mut pair, true, -1) } < 0,
            "an invalid UnixSocket protocol must not accidentally select pipe(2)"
        );
    }
}
