// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/os/pipe.cc and src/os/unix_socket.cc
// (Swoole contributors).
//! `swoole::SocketPair`, `Pipe` and `UnixSocket`: two descriptors, each wrapped in a
//! `network::Socket`, the master end writing and the worker end reading. With
//! `upstream-symbols` each member is defined under its upstream (Itanium) symbol, and
//! the process patch deletes `pipe.cc` and `unix_socket.cc`.
use crate::upstream::{socket_free, socket_read_sync, socket_write_sync};
use std::ffi::{CStr, c_int, c_void};
use std::ptr;
use swoole_rs_corebase::log;
use swoole_rs_reactor::socket_upstream::{make_socket_fd, socket_set_fd_option};
use swoole_rs_reactor::{Socket, SocketRecord, swr_v1_socket_buffer_size, swr_v1_socket_defaults};

/// `SW_FD_PIPE`, `SW_EVENT_READ`, `SW_TIMEOUT_ALL` and `SW_OK`.
const FD_PIPE: c_int = 3;
const EVENT_READ: c_int = 1 << 9;
const TIMEOUT_ALL: c_int = 15;
const OK: c_int = 0;
/// `SW_PIPE_CLOSE_MASTER` and `SW_PIPE_CLOSE_WORKER`.
const CLOSE_MASTER: c_int = 1;
const CLOSE_WORKER: c_int = 2;

/// `swoole::SocketPair` (`SWR_SOCKET_PAIR_RECORD_FIELDS`); `Pipe` and `UnixSocket`
/// add nothing to it.
#[repr(C)]
pub struct SocketPair {
    pub blocking: bool,
    pub timeout: f64,
    pub socks: [c_int; 2],
    pub master_socket: *mut SocketRecord,
    pub worker_socket: *mut SocketRecord,
}

const _: () = assert!(
    size_of::<SocketPair>() == 40,
    "SWR_SOCKET_PAIR_RECORD_FIELDS on LP64"
);

/// The constructors: the base's fields, then `pipe(2)` or an `AF_UNIX` `socketpair(2)`
/// of `protocol`, then a socket for each end (`socks[1]` the master, which writes).
/// False, after a warning from `origin`, when the descriptors can't be created.
///
/// # Safety
/// `this` is writable storage for a `SocketPair`.
unsafe fn open(this: *mut SocketPair, blocking: bool, unix: Option<c_int>, origin: &CStr) -> bool {
    // SAFETY: the defaults are only assigned while no pair is being built.
    let timeout = unsafe { swr_v1_socket_defaults.timeouts.read };
    let pair = SocketPair {
        blocking,
        timeout,
        socks: [0; 2],
        master_socket: ptr::null_mut(),
        worker_socket: ptr::null_mut(),
    };
    // SAFETY: as the caller promises.
    unsafe { this.write(pair) };
    // SAFETY: as the caller promises; the record was just written.
    let pair = unsafe { &mut *this };
    let socks = pair.socks.as_mut_ptr();
    let result = match unix {
        // SAFETY: `socks` has room for both descriptors.
        Some(protocol) => unsafe { libc::socketpair(libc::AF_UNIX, protocol, 0, socks) },
        // SAFETY: as above.
        None => unsafe { libc::pipe(socks) },
    };
    if result < 0 {
        let call = if unix.is_some() { "socketpair" } else { "pipe" };
        log::sys_warning(origin, format_args!("{call}() failed"));
        return false;
    }
    pair.master_socket = make_socket_fd(pair.socks[1], FD_PIPE);
    pair.worker_socket = make_socket_fd(pair.socks[0], FD_PIPE);
    // SAFETY: both ends are live sockets.
    unsafe { set_blocking(this, blocking) };
    true
}

/// `Pipe(bool blocking)`.
///
/// # Safety
/// `this` is writable storage for a `Pipe`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole4PipeC1Eb")
)]
pub unsafe extern "C" fn pipe_new(this: *mut SocketPair, blocking: bool) -> *mut SocketPair {
    // SAFETY: as the caller promises.
    unsafe { open(this, blocking, None, c"Pipe") };
    this
}

/// The base-object constructor of `Pipe(bool)`.
///
/// # Safety
/// As `pipe_new`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole4PipeC2Eb")
)]
pub unsafe extern "C" fn pipe_new_base(this: *mut SocketPair, blocking: bool) -> *mut SocketPair {
    // SAFETY: as the caller promises.
    unsafe { pipe_new(this, blocking) }
}

/// `UnixSocket(bool blocking, int protocol)`: also sets both ends' kernel buffers to
/// `Socket::default_buffer_size`.
///
/// # Safety
/// `this` is writable storage for a `UnixSocket`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10UnixSocketC1Ebi")
)]
pub unsafe extern "C" fn unix_socket_new(
    this: *mut SocketPair,
    blocking: bool,
    protocol: c_int,
) -> *mut SocketPair {
    // SAFETY: as the caller promises.
    if unsafe { open(this, blocking, Some(protocol), c"UnixSocket") } {
        // SAFETY: the defaults are only assigned while no pair is being built.
        let size = unsafe { swr_v1_socket_defaults.buffer_size };
        // SAFETY: both ends are live sockets.
        let _ = unsafe { unix_socket_set_buffer_size(this, size as usize) };
    }
    this
}

/// The base-object constructor of `UnixSocket(bool, int)`.
///
/// # Safety
/// As `unix_socket_new`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10UnixSocketC2Ebi")
)]
pub unsafe extern "C" fn unix_socket_new_base(
    this: *mut SocketPair,
    blocking: bool,
    protocol: c_int,
) -> *mut SocketPair {
    // SAFETY: as the caller promises.
    unsafe { unix_socket_new(this, blocking, protocol) }
}

/// The two ends, master first (None once closed).
///
/// # Safety
/// `this` is a live pair whose open ends are live sockets while the handles are used.
unsafe fn ends<'a>(this: *const SocketPair) -> [Option<Socket<'a>>; 2] {
    // SAFETY: as the caller promises.
    let pair = unsafe { &*this };
    // SAFETY: as the caller promises.
    [pair.master_socket, pair.worker_socket].map(|end| unsafe { Socket::from_ptr(end) })
}

/// `bool UnixSocket::set_buffer_size(size_t) const`: the master's kernel buffers, then
/// the worker's; false at the first that fails.
///
/// # Safety
/// `this` is a live pair whose ends are live sockets.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole10UnixSocket15set_buffer_sizeEm")
)]
#[must_use]
pub unsafe extern "C" fn unix_socket_set_buffer_size(this: *const SocketPair, size: usize) -> bool {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Socket::set_buffer_size takes uint32_t"
    )]
    let size = size as u32;
    // SAFETY: as the caller promises.
    unsafe { ends(this) }.into_iter().all(|socket| {
        socket.is_some_and(|socket| swr_v1_socket_buffer_size(socket.fd(), true, true, size))
    })
}

/// `void SocketPair::set_timeout(double)`: the pair's, then each end's every timeout.
///
/// # Safety
/// `this` is a live pair whose ends are live sockets.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10SocketPair11set_timeoutEd")
)]
pub unsafe extern "C" fn set_timeout(this: *mut SocketPair, timeout: f64) {
    // SAFETY: as the caller promises.
    unsafe { (*this).timeout = timeout };
    // SAFETY: as the caller promises.
    for socket in unsafe { ends(this) }.into_iter().flatten() {
        socket.edit(|record| record.timeouts.set(timeout, TIMEOUT_ALL));
    }
}

/// `void SocketPair::set_blocking(bool)`: the worker end, then the master end.
///
/// # Safety
/// `this` is a live pair whose ends are live sockets.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10SocketPair12set_blockingEb")
)]
pub unsafe extern "C" fn set_blocking(this: *mut SocketPair, blocking: bool) {
    // SAFETY: as the caller promises.
    let [master, worker] = unsafe { ends(this) };
    for socket in [worker, master].into_iter().flatten() {
        socket.edit(|record| socket_set_fd_option(record, c_int::from(!blocking), -1));
    }
    // SAFETY: as the caller promises.
    unsafe { (*this).blocking = blocking };
}

/// `ssize_t SocketPair::read(void *, size_t) const`: from the worker end, waiting for
/// data when the pair is blocking.
///
/// # Safety
/// `this` is a live pair and `buffer` denotes `length` writable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole10SocketPair4readEPvm")
)]
#[must_use]
pub unsafe extern "C" fn read(
    this: *const SocketPair,
    buffer: *mut c_void,
    length: usize,
) -> isize {
    // SAFETY: as the caller promises.
    let pair = unsafe { &*this };
    let (blocking, socket) = (pair.blocking, pair.worker_socket);
    if blocking {
        // SAFETY: as the caller promises.
        unsafe { socket_read_sync(socket, buffer, length) }
    } else {
        // SAFETY: as the caller promises.
        let Some(socket) = (unsafe { Socket::from_ptr(socket) }) else {
            return -1;
        };
        let fd = socket.fd();
        // SAFETY: as the caller promises.
        unsafe { libc::read(fd, buffer, length) }
    }
}

/// `ssize_t SocketPair::write(const void *, size_t) const`: to the master end.
///
/// # Safety
/// `this` is a live pair and `buffer` denotes `length` readable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole10SocketPair5writeEPKvm")
)]
#[must_use]
pub unsafe extern "C" fn write(
    this: *const SocketPair,
    buffer: *const c_void,
    length: usize,
) -> isize {
    // SAFETY: as the caller promises.
    let pair = unsafe { &*this };
    let (blocking, socket) = (pair.blocking, pair.master_socket);
    if blocking {
        // SAFETY: as the caller promises.
        unsafe { socket_write_sync(socket, buffer, length) }
    } else {
        // SAFETY: as the caller promises.
        let Some(socket) = (unsafe { Socket::from_ptr(socket) }) else {
            return -1;
        };
        let fd = socket.fd();
        // SAFETY: as the caller promises.
        unsafe { libc::write(fd, buffer, length) }
    }
}

/// `void SocketPair::clean() const`: reads and drops whatever the worker end holds.
///
/// # Safety
/// `this` is a live pair whose worker end is a live socket.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole10SocketPair5cleanEv")
)]
pub unsafe extern "C" fn clean(this: *const SocketPair) {
    // SAFETY: as the caller promises.
    let socket = unsafe { (*this).worker_socket };
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(socket) }) else {
        return;
    };
    let mut buffer = [0_u8; 1024];
    while socket.wait_event(0, EVENT_READ) == OK {
        // SAFETY: a local buffer of that size.
        if unsafe { libc::read(socket.fd(), buffer.as_mut_ptr().cast(), buffer.len()) } <= 0 {
            break;
        }
    }
}

/// Frees the end in `slot`; false when it is already closed.
///
/// # Safety
/// `slot` is null or a live socket nothing else uses.
unsafe fn close_end(slot: &mut *mut SocketRecord) -> bool {
    if slot.is_null() {
        return false;
    }
    // SAFETY: as the caller promises.
    unsafe { socket_free(*slot) };
    *slot = ptr::null_mut();
    true
}

/// `bool SocketPair::close(int which)`: the master end, the worker end, or (any other
/// value) both.
///
/// # Safety
/// `this` is a live pair.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10SocketPair5closeEi")
)]
pub unsafe extern "C-unwind" fn close(this: *mut SocketPair, which: c_int) -> bool {
    // SAFETY: as the caller promises.
    let pair = unsafe { &mut *this };
    match which {
        // SAFETY: as the caller promises.
        CLOSE_MASTER => unsafe { close_end(&mut pair.master_socket) },
        // SAFETY: as the caller promises.
        CLOSE_WORKER => unsafe { close_end(&mut pair.worker_socket) },
        _ => {
            // SAFETY: as the caller promises.
            unsafe { close_end(&mut pair.master_socket) };
            // SAFETY: as the caller promises.
            unsafe { close_end(&mut pair.worker_socket) };
            true
        }
    }
}

/// `~SocketPair()`: closes whichever ends are open.
///
/// # Safety
/// `this` is a live pair, not used again.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10SocketPairD1Ev")
)]
pub unsafe extern "C-unwind" fn drop(this: *mut SocketPair) -> *mut SocketPair {
    // SAFETY: as the caller promises.
    unsafe { close(this, 0) };
    this
}

/// The base-object destructor of `SocketPair`.
///
/// # Safety
/// As `drop`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10SocketPairD2Ev")
)]
pub unsafe extern "C-unwind" fn drop_base(this: *mut SocketPair) -> *mut SocketPair {
    // SAFETY: as the caller promises.
    unsafe { drop(this) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(new: impl FnOnce(*mut SocketPair)) -> SocketPair {
        let mut storage = std::mem::MaybeUninit::<SocketPair>::uninit();
        new(storage.as_mut_ptr());
        // SAFETY: the constructor initialised it.
        unsafe { storage.assume_init() }
    }

    fn round_trip(mut pair: SocketPair) {
        let this = &raw mut pair;
        assert!(!pair.master_socket.is_null() && !pair.worker_socket.is_null());
        let sent = *b"wire";
        let mut received = [0_u8; 4];
        assert_eq!(unsafe { write(this, sent.as_ptr().cast(), sent.len()) }, 4);
        assert_eq!(
            unsafe { read(this, received.as_mut_ptr().cast(), received.len()) },
            4
        );
        assert_eq!(received, sent);
        assert_eq!(unsafe { write(this, sent.as_ptr().cast(), sent.len()) }, 4);
        unsafe { set_blocking(this, false) };
        unsafe { clean(this) };
        assert!(unsafe { read(this, received.as_mut_ptr().cast(), received.len()) } < 0);
        unsafe { set_timeout(this, 2.5) };
        assert_eq!(pair.timeout, 2.5);
        assert_eq!(unsafe { (*pair.worker_socket).timeouts.read }, 2.5);
        assert!(unsafe { close(this, CLOSE_MASTER) });
        assert!(!unsafe { close(this, CLOSE_MASTER) });
        assert!(unsafe { close(this, 0) });
        assert!(pair.worker_socket.is_null());
        unsafe { drop(this) };
    }

    #[test]
    fn a_pipe_writes_the_master_end_and_reads_the_worker_end() {
        let pair = pair(|this| unsafe {
            pipe_new(this, true);
        });
        assert!(pair.blocking);
        assert_eq!(pair.timeout, unsafe {
            swr_v1_socket_defaults.timeouts.read
        });
        round_trip(pair);
    }

    #[test]
    fn a_unix_socket_is_a_pair_of_the_protocol_asked() {
        for protocol in [libc::SOCK_STREAM, libc::SOCK_DGRAM] {
            let pair = pair(|this| unsafe {
                unix_socket_new(this, true, protocol);
            });
            assert!(unsafe { unix_socket_set_buffer_size(&raw const pair, 65536) });
            round_trip(pair);
        }
        let failed = pair(|this| unsafe {
            unix_socket_new(this, false, -1);
        });
        assert!(failed.master_socket.is_null() && failed.worker_socket.is_null());
    }
}
