// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/os/pipe.cc and src/os/unix_socket.cc
// (Swoole contributors).
//! `swoole::SocketPair`, `Pipe` and `UnixSocket`: two descriptors, each wrapped in a
//! `network::Socket`, the master end writing and the worker end reading. With
//! `upstream-symbols` each member is defined under its upstream (Itanium) symbol, and
//! the process patch deletes `pipe.cc` and `unix_socket.cc`.
use crate::upstream::{read_sync, socket_free, write_sync};
use std::ffi::{CStr, c_int};
use std::mem::MaybeUninit;
use std::ptr;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::log;
use swoole_rs_reactor::socket_upstream::socket_set_fd_option;
use swoole_rs_reactor::{SOCKET_DEFAULTS, Socket, SocketRecord, swr_v1_socket_buffer_size};

/// `SW_FD_PIPE`, `SW_TIMEOUT_ALL` and `SW_OK`.
const FD_PIPE: c_int = 3;
const TIMEOUT_ALL: c_int = 15;
/// `SW_PIPE_CLOSE_MASTER` and `SW_PIPE_CLOSE_WORKER`.
const CLOSE_MASTER: c_int = 1;
const CLOSE_WORKER: c_int = 2;

/// `swoole::SocketPair` (`SWR_SOCKET_PAIR_RECORD_FIELDS`); `Pipe` and `UnixSocket`
/// add nothing to it.
/// The two ends are private: each is `None` or a socket the pair made and frees only
/// in its close, so a handle to one never outlives a borrow of the pair.
#[repr(C)]
pub struct SocketPair {
    pub blocking: bool,
    pub timeout: f64,
    pub socks: [c_int; 2],
    master_socket: Option<Socket<'static>>,
    worker_socket: Option<Socket<'static>>,
}

// SAFETY: a pair owns its two socket records (`open` makes them, `drop` frees them),
// and neither is tied to the thread that made it.
unsafe impl Send for SocketPair {}

const _: () = assert!(
    size_of::<SocketPair>() == 40,
    "SWR_SOCKET_PAIR_RECORD_FIELDS on LP64"
);

/// The constructors: the base's fields, then `pipe(2)` or an `AF_UNIX` `socketpair(2)`
/// of `protocol`, then a socket for each end (`socks[1]` the master, which writes).
/// False, after a warning from `origin`, when the descriptors can't be created.
fn open(blocking: bool, unix: Option<c_int>, origin: &CStr) -> (SocketPair, bool) {
    let timeout = SOCKET_DEFAULTS.read.get();
    let mut pair = SocketPair {
        blocking,
        timeout,
        socks: [0; 2],
        master_socket: None,
        worker_socket: None,
    };
    let socks = match unix {
        Some(protocol) => swoole_rs_corebase::sys::socketpair(libc::AF_UNIX, protocol, 0),
        None => swoole_rs_corebase::sys::pipe(),
    };
    let Some(socks) = socks else {
        let call = if unix.is_some() { "socketpair" } else { "pipe" };
        log::sys_warning(origin, format_args!("{call}() failed"));
        return (pair, false);
    };
    pair.socks = socks;
    let end = |fd| Box::new(SocketRecord::with_fd(fd, FD_PIPE)).into_socket();
    pair.master_socket = Some(end(pair.socks[1]));
    pair.worker_socket = Some(end(pair.socks[0]));
    set_blocking(&mut pair, blocking);
    (pair, true)
}

/// `Pipe(bool blocking)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole4PipeC1Eb")
)]
pub extern "C" fn pipe_new(this: &mut MaybeUninit<SocketPair>, blocking: bool) -> *mut SocketPair {
    this.write(pipe(blocking))
}

/// `Pipe(blocking)` by value: both ends null when it failed.
#[must_use]
pub fn pipe(blocking: bool) -> SocketPair {
    open(blocking, None, c"Pipe").0
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
    this.write(unix_socket(blocking, protocol))
}

/// `UnixSocket(blocking, protocol)` by value: both ends null when it failed.
#[must_use]
pub fn unix_socket(blocking: bool, protocol: c_int) -> SocketPair {
    let (pair, opened) = open(blocking, Some(protocol), c"UnixSocket");
    if opened {
        let size = SOCKET_DEFAULTS.buffer_size();
        #[expect(
            clippy::let_underscore_must_use,
            reason = "upstream's constructor ignores whether the buffers were set"
        )]
        let _ = unix_socket_set_buffer_size(&pair, size as usize);
    }
    pair
}

/// The two ends, master first (None once closed).
fn ends(pair: &SocketPair) -> [Option<Socket<'_>>; 2] {
    [pair.master_socket, pair.worker_socket]
}

impl SocketPair {
    /// The two ends' records, master first: null once closed. For a worker record
    /// that keeps them while the pair lives.
    #[must_use]
    pub fn raw_ends(&self) -> (*mut SocketRecord, *mut SocketRecord) {
        let raw = |end: Option<Socket<'_>>| end.map_or(ptr::null_mut(), Socket::as_ptr);
        (raw(self.master_socket), raw(self.worker_socket))
    }

    /// The master's end, or `None` once closed.
    #[must_use]
    pub fn master_end(&self) -> Option<Socket<'_>> {
        ends(self)[0]
    }

    /// The worker's end, or `None` once closed.
    #[must_use]
    pub fn worker_end(&self) -> Option<Socket<'_>> {
        ends(self)[1]
    }
}

/// `bool UnixSocket::set_buffer_size(size_t) const`: the master's kernel buffers, then
/// the worker's; false at the first that fails.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole10UnixSocket15set_buffer_sizeEm")
)]
#[must_use]
pub extern "C" fn unix_socket_set_buffer_size(this: &SocketPair, size: usize) -> bool {
    let size = size.c_cast::<u32>();
    ends(this).into_iter().all(|socket| {
        socket.is_some_and(|socket| swr_v1_socket_buffer_size(socket.fd(), true, true, size))
    })
}

/// `void SocketPair::set_timeout(double)`: the pair's, then each end's every timeout.
pub fn set_timeout(this: &mut SocketPair, timeout: f64) {
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

/// `ssize_t SocketPair::read(void *, size_t) const` into `buffer`: from the worker end,
/// waiting for data when the pair is blocking. The bytes read, or None.
pub fn read_into(this: &SocketPair, buffer: &mut [MaybeUninit<u8>]) -> Option<usize> {
    let [_, Some(socket)] = ends(this) else {
        return None;
    };
    let read = if this.blocking {
        read_sync(socket, buffer)
    } else {
        swoole_rs_corebase::sys::read(socket.fd(), buffer)
    };
    usize::try_from(read).ok()
}

/// `ssize_t SocketPair::write(const void *, size_t) const` of `data`: to the master
/// end. The bytes written, or None.
#[must_use]
pub fn write_from(this: &SocketPair, data: &[u8]) -> Option<usize> {
    let [Some(socket), _] = ends(this) else {
        return None;
    };
    let written = if this.blocking {
        write_sync(socket, data)
    } else {
        swoole_rs_corebase::sys::write(socket.fd(), data)
    };
    usize::try_from(written).ok()
}

/// `void SocketPair::clean() const`: reads and drops whatever the worker end holds.
pub fn clean(this: &SocketPair) {
    let [_, Some(socket)] = ends(this) else {
        return;
    };
    let mut buffer = [0_u8; 1024];
    while socket.wait_event(0, swoole_rs_reactor::Events::READ) {
        if swoole_rs_corebase::sys::read(socket.fd(), &mut buffer) <= 0 {
            break;
        }
    }
}

/// Frees the end in `slot`; false when it is already closed.
fn close_end(slot: &mut Option<Socket<'static>>) -> bool {
    let Some(socket) = slot.take() else {
        return false;
    };
    // SAFETY: a pair's open end is a live socket, which only its close frees, and
    // the pair no longer holds it.
    unsafe { socket_free(socket.as_ptr()) };
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

    /// [`read_into`] over initialised bytes.
    fn read(pair: &SocketPair, buffer: &mut [u8]) -> Option<usize> {
        let (at, length) = (buffer.as_mut_ptr().cast(), buffer.len());
        read_into(pair, unsafe { std::slice::from_raw_parts_mut(at, length) })
    }

    fn round_trip(mut pair: SocketPair) {
        let this = &raw mut pair;
        assert!(pair.master_socket.is_some() && pair.worker_socket.is_some());
        let sent = *b"wire";
        let mut received = [0_u8; 4];
        assert_eq!(write_from(unsafe { &*this }, &sent), Some(4));
        assert_eq!(read(unsafe { &*this }, &mut received), Some(4));
        assert_eq!(received, sent);
        assert_eq!(write_from(unsafe { &*this }, &sent), Some(4));
        set_blocking(unsafe { &mut *this }, false);
        clean(unsafe { &*this });
        assert!(read(unsafe { &*this }, &mut received).is_none());
        set_timeout(unsafe { &mut *this }, 2.5);
        assert_eq!(pair.timeout, 2.5);
        assert_eq!(unsafe { (*pair.raw_ends().1).fields.timeouts.read }, 2.5);
        assert!(close(unsafe { &mut *this }, CLOSE_MASTER));
        assert!(!close(unsafe { &mut *this }, CLOSE_MASTER));
        assert!(close(unsafe { &mut *this }, 0));
        assert!(pair.worker_socket.is_none());
        drop(&mut pair);
    }

    #[test]
    fn a_pipe_writes_the_master_end_and_reads_the_worker_end() {
        let pair = pair(|this| {
            pipe_new(this, true);
        });
        assert!(pair.blocking);
        assert_eq!(pair.timeout, SOCKET_DEFAULTS.read.get());
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
        assert!(failed.master_socket.is_none() && failed.worker_socket.is_none());
    }
}
