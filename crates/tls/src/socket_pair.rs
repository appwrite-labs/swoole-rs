// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/os/pipe.cc and src/os/unix_socket.cc
// (Swoole contributors).
//! `swoole::SocketPair`, `Pipe` and `UnixSocket`: two descriptors, each wrapped in a
//! `network::Socket`, the master end writing and the worker end reading. With
//! `upstream-symbols` each member is defined under its upstream (Itanium) symbol, and
//! the process patch deletes `pipe.cc` and `unix_socket.cc`.
use crate::upstream::{socket_free, socket_read_sync, socket_write_sync};
use std::ffi::{CStr, c_int, c_void};
use std::mem::MaybeUninit;
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
fn open<'a>(
    this: &'a mut MaybeUninit<SocketPair>,
    blocking: bool,
    unix: Option<c_int>,
    origin: &CStr,
) -> Option<&'a mut SocketPair> {
    // SAFETY: the defaults are only assigned while no pair is being built.
    let timeout = unsafe { swr_v1_socket_defaults.timeouts.read };
    let pair = SocketPair {
        blocking,
        timeout,
        socks: [0; 2],
        master_socket: ptr::null_mut(),
        worker_socket: ptr::null_mut(),
    };
    let pair = this.write(pair);
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
        return None;
    }
    pair.master_socket = make_socket_fd(pair.socks[1], FD_PIPE);
    pair.worker_socket = make_socket_fd(pair.socks[0], FD_PIPE);
    set_blocking(pair, blocking);
    Some(pair)
}

/// `Pipe(bool blocking)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole4PipeC1Eb")
)]
pub extern "C" fn pipe_new(this: &mut MaybeUninit<SocketPair>, blocking: bool) -> *mut SocketPair {
    open(this, blocking, None, c"Pipe");
    this.as_mut_ptr()
}

/// `UnixSocket(bool blocking, int protocol)`: also sets both ends' kernel buffers to
/// `Socket::default_buffer_size`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10UnixSocketC1Ebi")
)]
pub extern "C" fn unix_socket_new(
    this: &mut MaybeUninit<SocketPair>,
    blocking: bool,
    protocol: c_int,
) -> *mut SocketPair {
    if let Some(pair) = open(this, blocking, Some(protocol), c"UnixSocket") {
        // SAFETY: the defaults are only assigned while no pair is being built.
        let size = unsafe { swr_v1_socket_defaults.buffer_size };
        #[expect(
            clippy::let_underscore_must_use,
            reason = "upstream's constructor ignores whether the buffers were set"
        )]
        let _ = unix_socket_set_buffer_size(pair, size as usize);
    }
    this.as_mut_ptr()
}

/// The two ends, master first (None once closed).
fn ends(pair: &SocketPair) -> [Option<Socket<'_>>; 2] {
    // SAFETY: a pair's open ends are live sockets, freed only by its close.
    [pair.master_socket, pair.worker_socket].map(|end| unsafe { Socket::from_ptr(end) })
}

/// `bool UnixSocket::set_buffer_size(size_t) const`: the master's kernel buffers, then
/// the worker's; false at the first that fails.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole10UnixSocket15set_buffer_sizeEm")
)]
#[must_use]
pub extern "C" fn unix_socket_set_buffer_size(this: &SocketPair, size: usize) -> bool {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Socket::set_buffer_size takes uint32_t"
    )]
    let size = size as u32;
    ends(this).into_iter().all(|socket| {
        socket.is_some_and(|socket| swr_v1_socket_buffer_size(socket.fd(), true, true, size))
    })
}

/// `void SocketPair::set_timeout(double)`: the pair's, then each end's every timeout.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10SocketPair11set_timeoutEd")
)]
pub extern "C" fn set_timeout(this: &mut SocketPair, timeout: f64) {
    this.timeout = timeout;
    for socket in ends(this).into_iter().flatten() {
        socket.edit(|record| record.timeouts.set(timeout, TIMEOUT_ALL));
    }
}

/// `void SocketPair::set_blocking(bool)`: the worker end, then the master end.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10SocketPair12set_blockingEb")
)]
pub extern "C" fn set_blocking(this: &mut SocketPair, blocking: bool) {
    let [master, worker] = ends(this);
    for socket in [worker, master].into_iter().flatten() {
        socket.edit(|record| socket_set_fd_option(record, c_int::from(!blocking), -1));
    }
    this.blocking = blocking;
}

/// `ssize_t SocketPair::read(void *, size_t) const`: from the worker end, waiting for
/// data when the pair is blocking.
///
/// # Safety
/// `buffer` denotes `length` writable bytes.
#[must_use]
pub unsafe fn read(this: &SocketPair, buffer: *mut c_void, length: usize) -> isize {
    let [_, Some(socket)] = ends(this) else {
        return -1;
    };
    if this.blocking {
        // SAFETY: the live end, and the caller's buffer.
        unsafe { socket_read_sync(socket.as_ptr(), buffer, length) }
    } else {
        // SAFETY: as the caller promises.
        unsafe { libc::read(socket.fd(), buffer, length) }
    }
}

/// `ssize_t SocketPair::write(const void *, size_t) const`: to the master end.
///
/// # Safety
/// `buffer` denotes `length` readable bytes.
#[must_use]
pub unsafe fn write(this: &SocketPair, buffer: *const c_void, length: usize) -> isize {
    let [Some(socket), _] = ends(this) else {
        return -1;
    };
    if this.blocking {
        // SAFETY: the live end, and the caller's buffer.
        unsafe { socket_write_sync(socket.as_ptr(), buffer, length) }
    } else {
        // SAFETY: as the caller promises.
        unsafe { libc::write(socket.fd(), buffer, length) }
    }
}

/// `void SocketPair::clean() const`: reads and drops whatever the worker end holds.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole10SocketPair5cleanEv")
)]
pub extern "C" fn clean(this: &SocketPair) {
    let [_, Some(socket)] = ends(this) else {
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
fn close_end(slot: &mut *mut SocketRecord) -> bool {
    if slot.is_null() {
        return false;
    }
    // SAFETY: a pair's open end is a live socket, which only its close frees.
    unsafe { socket_free(*slot) };
    *slot = ptr::null_mut();
    true
}

/// `bool SocketPair::close(int which)`: the master end, the worker end, or (any other
/// value) both.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10SocketPair5closeEi")
)]
pub extern "C-unwind" fn close(pair: &mut SocketPair, which: c_int) -> bool {
    match which {
        CLOSE_MASTER => close_end(&mut pair.master_socket),
        CLOSE_WORKER => close_end(&mut pair.worker_socket),
        _ => {
            close_end(&mut pair.master_socket);
            close_end(&mut pair.worker_socket);
            true
        }
    }
}

/// `~SocketPair()`: closes whichever ends are open.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10SocketPairD1Ev")
)]
pub extern "C-unwind" fn drop(this: &mut SocketPair) -> *mut SocketPair {
    close(this, 0);
    this
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(new: impl FnOnce(&mut MaybeUninit<SocketPair>)) -> SocketPair {
        let mut storage = MaybeUninit::<SocketPair>::uninit();
        new(&mut storage);
        // SAFETY: the constructor initialised it.
        unsafe { storage.assume_init() }
    }

    fn round_trip(mut pair: SocketPair) {
        let this = &raw mut pair;
        assert!(!pair.master_socket.is_null() && !pair.worker_socket.is_null());
        let sent = *b"wire";
        let mut received = [0_u8; 4];
        assert_eq!(
            unsafe { write(&*this, sent.as_ptr().cast(), sent.len()) },
            4
        );
        assert_eq!(
            unsafe { read(&*this, received.as_mut_ptr().cast(), received.len()) },
            4
        );
        assert_eq!(received, sent);
        assert_eq!(
            unsafe { write(&*this, sent.as_ptr().cast(), sent.len()) },
            4
        );
        set_blocking(unsafe { &mut *this }, false);
        clean(unsafe { &*this });
        assert!(unsafe { read(&*this, received.as_mut_ptr().cast(), received.len()) } < 0);
        set_timeout(unsafe { &mut *this }, 2.5);
        assert_eq!(pair.timeout, 2.5);
        assert_eq!(unsafe { (*pair.worker_socket).fields.timeouts.read }, 2.5);
        assert!(close(unsafe { &mut *this }, CLOSE_MASTER));
        assert!(!close(unsafe { &mut *this }, CLOSE_MASTER));
        assert!(close(unsafe { &mut *this }, 0));
        assert!(pair.worker_socket.is_null());
        drop(&mut pair);
    }

    #[test]
    fn a_pipe_writes_the_master_end_and_reads_the_worker_end() {
        let pair = pair(|this| {
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
            let pair = pair(|this| {
                unix_socket_new(this, true, protocol);
            });
            assert!(unix_socket_set_buffer_size(&pair, 65536));
            round_trip(pair);
        }
        let failed = pair(|this| {
            unix_socket_new(this, false, -1);
        });
        assert!(failed.master_socket.is_null() && failed.worker_socket.is_null());
    }
}
