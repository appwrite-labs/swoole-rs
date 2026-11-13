//! `network::Socket`'s descriptor-level I/O as whole operations, syscall included:
//! `wait_event`'s poll, `clean`, `recvfrom`, `accept` and `connect`. Callers keep
//! the `Socket` fields, the last-error slot and signal dispatch, which they pass in.
use crate::SocketAddress;
use std::io::Error;
use std::os::raw::{c_int, c_void};
use swoole_rs_corebase::CCast;

fn errno() -> c_int {
    Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// An I/O outcome: `status` 0 ready, 1 timed out (`wait`) or in progress
/// (`connect`), 2 failed with `error` (an errno).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SocketIoResult {
    pub status: c_int,
    pub error: c_int,
}

const READY: SocketIoResult = SocketIoResult {
    status: 0,
    error: 0,
};

const fn failed(error: c_int) -> SocketIoResult {
    SocketIoResult { status: 2, error }
}

/// `Socket::wait_event`: poll `fd` for `events` (already translated to poll bits)
/// for `timeout_ms` (negative waits forever). On `EINTR` it fails if
/// `dont_restart`, otherwise runs `swoole_signal_dispatch` and polls again.
#[must_use]
pub fn fd_wait(fd: c_int, events: i16, timeout_ms: c_int, dont_restart: bool) -> SocketIoResult {
    let mut event = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    let timeout = if timeout_ms < 0 { -1 } else { timeout_ms };
    loop {
        let ready = swoole_rs_corebase::sys::poll(std::slice::from_mut(&mut event), timeout);
        if ready == 0 {
            return SocketIoResult {
                status: 1,
                error: libc::EAGAIN,
            };
        }
        if ready > 0 {
            return READY;
        }
        let error = errno();
        if error != libc::EINTR || dont_restart {
            return failed(error);
        }
        swoole_rs_process::signal::swoole_signal_dispatch();
    }
}

/// `Socket::clean`: discard everything already received, without blocking.
pub fn fd_clean(fd: c_int) {
    let mut buffer = [0_u8; 2048];
    while swoole_rs_corebase::sys::recv(fd, &mut buffer, libc::MSG_DONTWAIT) > 0 {}
}

/// `Socket::recvfrom`: recvfrom(2), retried on `EINTR` up to
/// `SW_SOCKET_RETRY_COUNT` times.
///
/// # Safety
/// `buffer` denotes `length` writable bytes; `address` and `address_length` are
/// null or valid for recvfrom(2).
pub unsafe fn fd_recvfrom(
    fd: c_int,
    buffer: *mut c_void,
    length: usize,
    flags: c_int,
    address: *mut libc::sockaddr,
    address_length: *mut libc::socklen_t,
) -> isize {
    let mut received = 0;
    for _ in 0..swoole_rs_corebase::constants::SW_SOCKET_RETRY_COUNT {
        // SAFETY: the caller lends the buffer and address storage for this call.
        received = unsafe { libc::recvfrom(fd, buffer, length, flags, address, address_length) };
        if received >= 0 || errno() != libc::EINTR {
            break;
        }
    }
    received
}

/// `Socket::accept`'s syscall into `address`, whose `len` is the capacity: the
/// accepted fd (close-on-exec, nonblocking if `nonblock`) or -1, and whether the
/// listening socket became close-on-exec. Without accept4 (macOS) it accepts, then
/// makes upstream's `set_fd_option(nonblock, 1)` call, which applies to the
/// listening socket, not the new one.
#[must_use]
pub fn fd_accept(fd: c_int, address: &mut SocketAddress, nonblock: bool) -> (c_int, bool) {
    let length = &raw mut address.len;
    // Upstream's capacity is sizeof(Address), past the storage, so all of it is lent.
    let storage = std::ptr::from_mut(address).cast::<libc::sockaddr>();
    #[cfg(target_os = "linux")]
    {
        let flags = libc::SOCK_CLOEXEC | if nonblock { libc::SOCK_NONBLOCK } else { 0 };
        // SAFETY: the borrowed address, `len` bytes of it.
        (unsafe { libc::accept4(fd, storage, length, flags) }, false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        // SAFETY: the borrowed address, `len` bytes of it.
        let accepted = unsafe { libc::accept(fd, storage, length) };
        let cloexec =
            accepted >= 0 && crate::swr_v1_socket_fd_option(fd, c_int::from(nonblock), 1).ok;
        (accepted, cloexec)
    }
}

/// `connect_async`'s loop: connect(2) to `address`, retried on `EINTR`. Status 1
/// is `EINPROGRESS` (wait for writability), 2 any other error.
#[must_use]
pub fn fd_connect(fd: c_int, address: &SocketAddress) -> SocketIoResult {
    loop {
        if address.connect(fd) == 0 {
            return READY;
        }
        match errno() {
            libc::EINTR => {}
            libc::EINPROGRESS => {
                return SocketIoResult {
                    status: 1,
                    error: libc::EINPROGRESS,
                };
            }
            error => return failed(error),
        }
    }
}

/// `connect_sync`'s `SO_ERROR` read once the socket is writable: ready, or failed
/// with the pending error (or getsockopt's own errno).
#[must_use]
pub fn fd_pending_error(fd: c_int) -> SocketIoResult {
    match socket_error(fd) {
        Ok(0) => READY,
        Ok(error) | Err(error) => failed(error),
    }
}

/// `getsockopt(fd, SOL_SOCKET, SO_ERROR)`: the pending error.
/// # Errors
/// getsockopt's own errno.
pub fn socket_error(fd: c_int) -> Result<c_int, c_int> {
    let mut pending: c_int = 0;
    // Upstream passes sizeof(len), which equals sizeof(int).
    let read =
        swoole_rs_corebase::sys::getsockopt_int(fd, libc::SOL_SOCKET, libc::SO_ERROR, &mut pending);
    if read < 0 { Err(errno()) } else { Ok(pending) }
}

/// `check_sendfile_parameters`: fstat(2) the open `file_fd` and resolve the range
/// `[begin, *end)` to send, where a zero `length` means to the end of the file.
/// Returns 0 with `*end` set, 1 when fstat failed (errno set), 2 for an empty
/// file and 3 (`*end` set) for a range outside it.
pub fn sendfile_range(
    file_fd: c_int,
    begin: libc::off_t,
    length: libc::size_t,
    end: &mut libc::off_t,
) -> c_int {
    let Some(status) = swoole_rs_corebase::sys::fstat(file_fd) else {
        return 1;
    };
    if status.st_size == 0 {
        return 2;
    }
    let stop = if length == 0 {
        status.st_size
    } else {
        begin.wrapping_add(length.c_cast::<libc::off_t>())
    };
    *end = stop;
    if begin < 0 || stop > status.st_size {
        3
    } else {
        0
    }
}

/// `get_sendfile_chunk_size`: what is left of `[begin, end)`, at most
/// `SW_SENDFILE_CHUNK_SIZE`.
#[must_use]
pub fn sendfile_chunk(begin: libc::off_t, end: libc::off_t) -> libc::size_t {
    const CHUNK: libc::size_t =
        swoole_rs_corebase::constants::SW_SENDFILE_CHUNK_SIZE as libc::size_t;
    // A range past its end converts to a huge size_t upstream, so a full chunk.
    libc::size_t::try_from(end.wrapping_sub(begin)).map_or(CHUNK, |left| left.min(CHUNK))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    fn pair() -> [c_int; 2] {
        let mut fds = [0; 2];
        // SAFETY: `fds` holds the two descriptors socketpair writes.
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) },
            0
        );
        fds
    }

    fn close(fds: &[c_int]) {
        for &fd in fds {
            // SAFETY: the test opened these descriptors.
            unsafe { libc::close(fd) };
        }
    }

    fn write(fd: c_int, bytes: &[u8]) {
        // SAFETY: `bytes` is readable for its length.
        let written = unsafe { libc::write(fd, bytes.as_ptr().cast::<c_void>(), bytes.len()) };
        assert_eq!(written, bytes.len().cast_signed(), "the whole write");
    }

    #[test]
    fn wait_times_out_then_sees_data() {
        let [a, b] = pair();
        assert_eq!(
            fd_wait(a, libc::POLLIN, 10, false),
            SocketIoResult {
                status: 1,
                error: libc::EAGAIN
            }
        );
        write(b, b"x");
        assert_eq!(fd_wait(a, libc::POLLIN, -5, false), READY);
        fd_clean(a);
        assert_eq!(
            fd_wait(a, libc::POLLIN, 0, false).status,
            1,
            "clean drained it"
        );
        close(&[a, b]);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no recvfrom")]
    fn recvfrom_reads_what_was_sent() {
        let [a, b] = pair();
        write(b, b"hello");
        let mut buffer = [0_u8; 8];
        // SAFETY: `buffer` is a local; no address is requested.
        let n = unsafe {
            fd_recvfrom(
                a,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(&buffer[..n.cast_unsigned()], b"hello");
        close(&[a, b]);
    }

    #[test]
    fn connect_accept_and_pending_error() {
        // SAFETY: socket(2) has no memory preconditions.
        let listener = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        let mut address: SocketAddress = unsafe { std::mem::zeroed() };
        address.addr.inet_v4.sin_family = libc::AF_INET.try_into().unwrap_or_default();
        address.addr.inet_v4.sin_addr.s_addr = u32::from(std::net::Ipv4Addr::LOCALHOST).to_be();
        let length = (size_of::<libc::sockaddr_in>()).c_cast::<libc::socklen_t>();
        address.len = length;
        let raw = (&raw mut address.addr).cast::<libc::sockaddr>();
        // SAFETY: `address` is a local sockaddr_in of `length` bytes.
        assert_eq!(unsafe { libc::bind(listener, raw, length) }, 0);
        // SAFETY: listen(2) has no memory preconditions.
        assert_eq!(unsafe { libc::listen(listener, 8) }, 0);
        // SAFETY: as above; getsockname fills in the bound port.
        assert_eq!(
            unsafe { libc::getsockname(listener, raw, &raw mut address.len) },
            0
        );

        // SAFETY: socket(2) has no memory preconditions.
        let client = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        assert_eq!(fd_connect(client, &address), READY);
        assert_eq!(fd_pending_error(client), READY);

        let mut peer = address;
        let (accepted, _) = fd_accept(listener, &mut peer, true);
        assert!(accepted >= 0, "accepted");
        if cfg!(target_os = "linux") {
            // SAFETY: fcntl(F_GETFL) has no memory preconditions.
            assert_ne!(
                unsafe { libc::fcntl(accepted, libc::F_GETFL) } & libc::O_NONBLOCK,
                0,
                "nonblocking"
            );
        }
        close(&[accepted, client, listener]);
        // -1, not the closed fd: a parallel test may reuse that number.
        assert_eq!(fd_pending_error(-1).status, 2, "invalid fd");
    }

    #[test]
    fn sendfile_range_and_chunk() {
        let path = std::env::temp_dir().join(format!("swr-sendfile-{}", std::process::id()));
        std::fs::write(&path, [7_u8; 10]).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let empty = std::fs::File::create(path.with_extension("empty")).unwrap();
        let range = |fd, begin, length| {
            let mut end = -1;
            (sendfile_range(fd, begin, length, &mut end), end)
        };
        let fd = file.as_raw_fd();
        assert_eq!(range(fd, 0, 0), (0, 10), "whole file");
        assert_eq!(range(fd, 4, 6), (0, 10), "tail");
        assert_eq!(range(fd, 4, 7), (3, 11), "past the end");
        assert_eq!(range(fd, -1, 1), (3, 0), "negative offset");
        assert_eq!(range(empty.as_raw_fd(), 0, 0), (2, -1), "empty");
        assert_eq!(range(-1, 0, 0), (1, -1), "fstat failed");
        drop((file, empty));
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(path.with_extension("empty")).unwrap();

        assert_eq!(sendfile_chunk(0, 10), 10);
        assert_eq!(sendfile_chunk(0, 1 << 20), 65536);
        assert_eq!(sendfile_chunk(5, 4), 65536, "end before begin");
    }
}
