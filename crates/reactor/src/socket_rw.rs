//! `network::Socket`'s per-request reads and writes as whole operations, syscall
//! included: `recv`, `send` and `peek` with their EINTR and `dont_restart` rules,
//! and `catch_error`'s errno classification. A TLS socket passes its OpenSSL call
//! (`ssl_recv`, `ssl_send`, `SSL_peek`), which this loop drives.
use std::io::Error;
use std::os::raw::{c_int, c_void};

/// A TLS read or write on `socket`: what `ssl_recv`/`ssl_send`/`SSL_peek` return.
pub type TlsIo =
    unsafe extern "C" fn(socket: *mut c_void, buffer: *mut c_void, length: usize) -> isize;

fn interrupted() -> bool {
    Error::last_os_error().raw_os_error() == Some(libc::EINTR)
}

/// `Socket::recv`: recv(2), or `tls` reads until `length` bytes when the socket is
/// nonblocking or `MSG_WAITALL` is set. Retried on EINTR unless `dont_restart`;
/// errno is left as the last call set it.
///
/// # Safety
/// `buffer` denotes `length` writable bytes; `tls`, when given, is valid for its socket.
#[must_use]
pub unsafe fn socket_recv(
    fd: c_int,
    buffer: *mut c_void,
    length: usize,
    flags: c_int,
    nonblock: bool,
    dont_restart: bool,
    tls: Option<(TlsIo, *mut c_void)>,
) -> isize {
    loop {
        let total = match tls {
            Some((read, socket)) => {
                let mut total: isize = 0;
                while total.cast_unsigned() < length {
                    // SAFETY: `total < length`, so the rest of the caller's buffer starts in bounds.
                    let rest = unsafe { buffer.cast::<u8>().add(total.cast_unsigned()) };
                    // SAFETY: the caller's TLS read for this socket, into the rest of its buffer.
                    let read = unsafe { read(socket, rest.cast(), length - total.cast_unsigned()) };
                    if read <= 0 {
                        if total == 0 {
                            total = read;
                        }
                        break;
                    }
                    total += read;
                    if !(nonblock || flags & libc::MSG_WAITALL != 0) {
                        break;
                    }
                }
                total
            }
            // SAFETY: the caller lends `length` writable bytes.
            None => unsafe { libc::recv(fd, buffer, length, flags) },
        };
        if !(total < 0 && interrupted() && !dont_restart) {
            return total;
        }
    }
}

/// `Socket::send`: send(2) or one `tls` write, retried on EINTR unless `dont_restart`.
///
/// # Safety
/// `buffer` denotes `length` readable bytes; `tls`, when given, is valid for its socket.
#[must_use]
pub unsafe fn socket_send(
    fd: c_int,
    buffer: *const c_void,
    length: usize,
    flags: c_int,
    dont_restart: bool,
    tls: Option<(TlsIo, *mut c_void)>,
) -> isize {
    loop {
        let sent = match tls {
            // SAFETY: the caller's TLS write for this socket; it only reads the buffer.
            Some((write, socket)) => unsafe { write(socket, buffer.cast_mut(), length) },
            // SAFETY: the caller lends `length` readable bytes.
            None => unsafe { libc::send(fd, buffer, length, flags) },
        };
        if !(sent < 0 && interrupted() && !dont_restart) {
            return sent;
        }
    }
}

/// `Socket::peek`: recv(2) with `MSG_PEEK`, or `tls` (`SSL_peek`), retried on EINTR.
///
/// # Safety
/// `buffer` denotes `length` writable bytes; `tls`, when given, is valid for its socket.
#[must_use]
pub unsafe fn socket_peek(
    fd: c_int,
    buffer: *mut c_void,
    length: usize,
    flags: c_int,
    tls: Option<(TlsIo, *mut c_void)>,
) -> isize {
    loop {
        let peeked = match tls {
            // SAFETY: the caller's SSL_peek for this socket, into its buffer.
            Some((peek, socket)) => unsafe { peek(socket, buffer, length) },
            // SAFETY: the caller lends `length` writable bytes.
            None => unsafe { libc::recv(fd, buffer, length, flags | libc::MSG_PEEK) },
        };
        if !(peeked < 0 && interrupted()) {
            return peeked;
        }
    }
}

/// `SW_ERROR_SSL_BAD_CLIENT` and `SW_ERROR_SSL_RESET` (native asserts them).
const SSL_BAD_CLIENT: c_int = 1015;
const SSL_RESET: c_int = 1017;
/// `swReturnCode`.
const WAIT: c_int = 2;
const CLOSE: c_int = 3;
const ERROR: c_int = 4;
const INVALID: c_int = 6;
const REDUCE_SIZE: c_int = 7;

/// `Socket::catch_error`: what an errno (or Swoole TLS error) means for the socket.
/// `ENOBUFS` waits and sets `kernel_nobufs`; `EFAULT` aborts, as upstream does.
pub fn catch_error(error: c_int, kernel_nobufs: &mut bool) -> c_int {
    #[expect(
        unreachable_patterns,
        reason = "EWOULDBLOCK equals EAGAIN on these platforms"
    )]
    match error {
        libc::EFAULT => std::process::abort(),
        libc::EBADF | libc::ENOENT => INVALID,
        libc::ECONNRESET
        | libc::ECONNABORTED
        | libc::EPIPE
        | libc::ENOTCONN
        | libc::ETIMEDOUT
        | libc::ECONNREFUSED
        | libc::ENETDOWN
        | libc::ENETUNREACH
        | libc::EHOSTDOWN
        | libc::EHOSTUNREACH
        | SSL_BAD_CLIENT
        | SSL_RESET => CLOSE,
        libc::EAGAIN | libc::EWOULDBLOCK | 0 => WAIT,
        libc::ENOBUFS => {
            *kernel_nobufs = true;
            WAIT
        }
        _ => ERROR,
    }
}

/// `Socket::catch_write_pipe_error`: a pipe write that is too large reduces its
/// size (`EMSGSIZE`, and on Linux `ENOBUFS`, which also sets `kernel_nobufs`);
/// anything else is [`catch_error`]'s.
pub fn catch_pipe_error(error: c_int, kernel_nobufs: &mut bool) -> c_int {
    match error {
        libc::EMSGSIZE => REDUCE_SIZE,
        #[cfg(target_os = "linux")]
        libc::ENOBUFS => {
            *kernel_nobufs = true;
            REDUCE_SIZE
        }
        _ => catch_error(error, kernel_nobufs),
    }
}

/// An `IOVector`'s cursor, owned with its descriptor allocation.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IovState {
    pub iterator: *mut libc::iovec,
    pub remain_count: c_int,
    pub index: c_int,
    pub offset_bytes: usize,
}

impl IovState {
    /// `IOVector::update_iterator`.
    ///
    /// # Safety
    /// `iterator` names `remain_count` writable initialized iovecs.
    pub(crate) unsafe fn update(&mut self, n: isize) {
        // SAFETY: as the caller promises.
        let iterator = unsafe { crate::iov_owned::span(self.iterator, self.remain_count) };
        let reply = crate::iov_owned::owned_progress(
            iterator,
            n,
            self.remain_count,
            self.index,
            self.offset_bytes,
        );
        if reply.action == 0 {
            return;
        }
        assert!(
            reply.action > 0,
            "iovec progress outside the original domain"
        );
        self.remain_count = reply.remain_count;
        self.index = reply.index;
        self.offset_bytes = reply.offset_bytes;
        if reply.action == 1 {
            self.iterator = self.iterator.wrapping_add(reply.advance);
        }
    }
}

/// `Socket::readv`/`writev`: readv(2)/writev(2) over the remaining iovecs, or
/// `tls` one iovec at a time until one comes up short, advancing `state`;
/// retried on EINTR.
///
/// # Safety
/// `state` names a live `IOVector`'s iovecs, whose buffers are valid for the
/// direction; `tls`, when given, is valid for `socket`.
pub unsafe fn socket_iov(
    fd: c_int,
    write: bool,
    state: &mut IovState,
    tls: Option<TlsIo>,
    socket: *mut c_void,
) -> isize {
    loop {
        let result = if let Some(io) = tls {
            let mut total: isize = 0;
            loop {
                // SAFETY: the current iovec is live while any remain.
                let current = unsafe { *state.iterator };
                // SAFETY: the caller's TLS call for this socket, on that iovec.
                let done = unsafe { io(socket, current.iov_base, current.iov_len) };
                total += done.max(0);
                // SAFETY: the iterator still names `remain_count` iovecs.
                unsafe { state.update(done) };
                if !(done > 0 && state.remain_count > 0) {
                    break if total > 0 { total } else { done };
                }
            }
        } else {
            let done = if write {
                // SAFETY: `remain_count` iovecs from the iterator are the caller's buffers.
                unsafe { libc::writev(fd, state.iterator, state.remain_count) }
            } else {
                // SAFETY: as above, writable.
                unsafe { libc::readv(fd, state.iterator, state.remain_count) }
            };
            // SAFETY: the iterator still names `remain_count` iovecs.
            unsafe { state.update(done) };
            done
        };
        if !(result < 0 && interrupted()) {
            return result;
        }
    }
}

/// `Socket::ssl_sendfile`'s outcome: what it returns, and which warning native code
/// writes (1 `write() failed`, 2 `pread() failed`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SslSendfileResult {
    pub value: isize,
    pub warn: c_int,
}

/// `Socket::ssl_sendfile`: pread(2) up to `SW_BUFFER_SIZE_BIG` bytes of `file_fd`
/// at `*offset`, hand them to `send` once, and advance `*offset` by what it sent.
/// A failed send warns when `catch_error` says it is an error.
///
/// # Safety
/// `offset` is writable and `send` is valid for `socket`, which `send` may
/// access while `offset` is lent.
pub unsafe fn ssl_sendfile(
    file_fd: c_int,
    offset: *mut libc::off_t,
    size: usize,
    send: TlsIo,
    socket: *mut c_void,
    kernel_nobufs: &mut bool,
) -> SslSendfileResult {
    const CHUNK: usize = 65536;
    let mut buffer = vec![0_u8; CHUNK];
    // SAFETY: the caller's offset.
    let at = unsafe { *offset };
    // SAFETY: up to `CHUNK` bytes into the local buffer.
    let n = unsafe { libc::pread(file_fd, buffer.as_mut_ptr().cast(), size.min(CHUNK), at) };
    if n <= 0 {
        return SslSendfileResult { value: -1, warn: 2 };
    }
    // SAFETY: the caller's TLS write of the `n` bytes just read.
    let sent = unsafe { send(socket, buffer.as_mut_ptr().cast(), n.cast_unsigned()) };
    if sent < 0 {
        let error = Error::last_os_error().raw_os_error().unwrap_or(0);
        let failed = catch_error(error, kernel_nobufs) == ERROR;
        return SslSendfileResult {
            value: sent,
            warn: c_int::from(failed),
        };
    }
    // SAFETY: the caller's offset.
    unsafe { *offset = at + libc::off_t::try_from(sent).unwrap_or(libc::off_t::MAX) };
    SslSendfileResult {
        value: sent,
        warn: 0,
    }
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

    unsafe extern "C" fn tls_in_pieces(
        socket: *mut c_void,
        buffer: *mut c_void,
        length: usize,
    ) -> isize {
        // The "TLS" layer hands out two bytes per call from a counter.
        let left = unsafe { &mut *socket.cast::<usize>() };
        if *left == 0 {
            return 0;
        }
        let n = length.min(2).min(*left);
        unsafe { std::ptr::write_bytes(buffer.cast::<u8>(), b'x', n) };
        *left -= n;
        n.cast_signed()
    }

    #[test]
    fn plain_send_recv_and_peek() {
        let [a, b] = pair();
        // SAFETY: local buffers, no TLS.
        unsafe {
            assert_eq!(
                socket_send(b, b"hello".as_ptr().cast(), 5, 0, false, None),
                5
            );
            let mut buffer = [0_u8; 8];
            assert_eq!(socket_peek(a, buffer.as_mut_ptr().cast(), 8, 0, None), 5);
            assert_eq!(
                socket_recv(a, buffer.as_mut_ptr().cast(), 8, 0, false, false, None),
                5
            );
            assert_eq!(&buffer[..5], b"hello");
            libc::close(a);
            libc::close(b);
        }
    }

    #[test]
    fn tls_reads_accumulate_only_when_nonblocking_or_waitall() {
        let mut buffer = [0_u8; 8];
        let mut left: usize = 7;
        let socket = (&raw mut left).cast::<c_void>();
        // SAFETY: local buffer and counter; the stub reads within them.
        unsafe {
            assert_eq!(
                socket_recv(
                    -1,
                    buffer.as_mut_ptr().cast(),
                    8,
                    0,
                    false,
                    false,
                    Some((tls_in_pieces, socket))
                ),
                2
            );
            assert_eq!(
                socket_recv(
                    -1,
                    buffer.as_mut_ptr().cast(),
                    8,
                    0,
                    true,
                    false,
                    Some((tls_in_pieces, socket))
                ),
                5
            );
            assert_eq!(
                socket_recv(
                    -1,
                    buffer.as_mut_ptr().cast(),
                    8,
                    0,
                    true,
                    false,
                    Some((tls_in_pieces, socket))
                ),
                0
            );
        }
    }

    #[test]
    fn errors_classify_as_upstream() {
        let mut nobufs = false;
        let mut classify = |error| catch_error(error, &mut nobufs);
        assert_eq!(classify(libc::EAGAIN), WAIT);
        assert_eq!(classify(0), WAIT);
        assert_eq!(classify(libc::EPIPE), CLOSE);
        assert_eq!(classify(SSL_RESET), CLOSE);
        assert_eq!(classify(libc::EBADF), INVALID);
        assert_eq!(classify(libc::EINVAL), ERROR);
        assert_eq!(classify(libc::ENOBUFS), WAIT);
        assert!(nobufs, "ENOBUFS records kernel_nobufs");
    }

    #[test]
    fn pipe_errors_reduce_the_size() {
        let mut nobufs = false;
        let mut classify = |error| catch_pipe_error(error, &mut nobufs);
        assert_eq!(classify(libc::EMSGSIZE), REDUCE_SIZE);
        assert_eq!(classify(libc::EAGAIN), WAIT);
        assert_eq!(classify(libc::EPIPE), CLOSE);
        let expected = if cfg!(target_os = "linux") {
            REDUCE_SIZE
        } else {
            WAIT
        };
        assert_eq!(classify(libc::ENOBUFS), expected);
        assert!(nobufs, "ENOBUFS records kernel_nobufs");
    }

    #[test]
    fn iov_plain_and_tls() {
        let [a, b] = pair();
        let (mut first, mut second) = (*b"abc", *b"defg");
        let mut iov = [
            libc::iovec {
                iov_base: first.as_mut_ptr().cast(),
                iov_len: 3,
            },
            libc::iovec {
                iov_base: second.as_mut_ptr().cast(),
                iov_len: 4,
            },
        ];
        let mut state = IovState {
            iterator: iov.as_mut_ptr(),
            remain_count: 2,
            index: 0,
            offset_bytes: 0,
        };
        // SAFETY: `state` names the two local iovecs.
        let sent = unsafe { socket_iov(a, true, &mut state, None, std::ptr::null_mut()) };
        assert_eq!((sent, state.remain_count), (7, 0));

        let mut into = [0_u8; 5];
        let mut tail = [0_u8; 5];
        let mut iov = [
            libc::iovec {
                iov_base: into.as_mut_ptr().cast(),
                iov_len: 5,
            },
            libc::iovec {
                iov_base: tail.as_mut_ptr().cast(),
                iov_len: 5,
            },
        ];
        let mut state = IovState {
            iterator: iov.as_mut_ptr(),
            remain_count: 2,
            index: 0,
            offset_bytes: 0,
        };
        // SAFETY: as above, writable.
        let read = unsafe { socket_iov(b, false, &mut state, None, std::ptr::null_mut()) };
        assert_eq!(read, 7);
        assert_eq!((&into, &tail[..2]), (b"abcde", &b"fg"[..]));
        assert_eq!(
            (state.remain_count, state.index, state.offset_bytes),
            (1, 1, 2)
        );

        // TLS: two bytes per call until 7 are gone, then a 0 that ends the loop.
        let mut left: usize = 7;
        let mut into = [0_u8; 4];
        let mut tail = [0_u8; 6];
        let mut iov = [
            libc::iovec {
                iov_base: into.as_mut_ptr().cast(),
                iov_len: 4,
            },
            libc::iovec {
                iov_base: tail.as_mut_ptr().cast(),
                iov_len: 6,
            },
        ];
        let mut state = IovState {
            iterator: iov.as_mut_ptr(),
            remain_count: 2,
            index: 0,
            offset_bytes: 0,
        };
        // SAFETY: the counter stands in for the socket; the iovecs are local.
        let read = unsafe {
            socket_iov(
                -1,
                false,
                &mut state,
                Some(tls_in_pieces),
                (&raw mut left).cast(),
            )
        };
        assert_eq!((read, left), (7, 0));
        assert_eq!((&into, &tail[..3]), (b"xxxx", &b"xxx"[..]));
        // update_iterator keeps the first iovec's offset when that iovec ends exactly.
        assert_eq!(
            (state.remain_count, state.index, state.offset_bytes),
            (1, 1, 5)
        );
        // SAFETY: closes the pair this test opened.
        unsafe {
            libc::close(a);
            libc::close(b);
        }
    }

    unsafe extern "C" fn sends_three(_: *mut c_void, _: *mut c_void, length: usize) -> isize {
        length.min(3).cast_signed()
    }

    #[test]
    fn ssl_sendfile_reads_then_sends() {
        let path = std::env::temp_dir().join(format!("swr-ssl-sendfile-{}", std::process::id()));
        std::fs::write(&path, b"0123456789").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let mut offset: libc::off_t = 4;
        let mut nobufs = false;
        // SAFETY: local offset and flag; the callback ignores its socket.
        let result = unsafe {
            ssl_sendfile(
                file.as_raw_fd(),
                &raw mut offset,
                6,
                sends_three,
                std::ptr::null_mut(),
                &mut nobufs,
            )
        };
        assert_eq!(
            (result, offset),
            (SslSendfileResult { value: 3, warn: 0 }, 7)
        );
        offset = 10;
        // SAFETY: as above; nothing is left to read.
        let result = unsafe {
            ssl_sendfile(
                file.as_raw_fd(),
                &raw mut offset,
                6,
                sends_three,
                std::ptr::null_mut(),
                &mut nobufs,
            )
        };
        assert_eq!(result, SslSendfileResult { value: -1, warn: 2 });
        std::fs::remove_file(&path).unwrap();
    }
}
