//! `network::Socket`'s I/O pieces shared by its reads and writes: the TLS call a
//! record's `recv`, `send` and `peek` (`socket_methods.rs`) drive, their EINTR test,
//! `catch_error`'s errno classification and `ssl_sendfile`.
use std::io::Error;
use std::os::raw::c_int;
use swoole_rs_corebase::constants::ReturnCode;
use swoole_rs_error::ErrorCode;

pub(crate) fn interrupted() -> bool {
    Error::last_os_error().raw_os_error() == Some(libc::EINTR)
}

const SSL_BAD_CLIENT: c_int = ErrorCode::SW_ERROR_SSL_BAD_CLIENT as c_int;
const SSL_RESET: c_int = ErrorCode::SW_ERROR_SSL_RESET as c_int;
const WAIT: c_int = ReturnCode::SW_WAIT as c_int;
const CLOSE: c_int = ReturnCode::SW_CLOSE as c_int;
const ERROR: c_int = ReturnCode::SW_ERROR as c_int;
const INVALID: c_int = ReturnCode::SW_INVALID as c_int;
const REDUCE_SIZE: c_int = ReturnCode::SW_REDUCE_SIZE as c_int;

/// `Socket::catch_error`: what an errno (or Swoole TLS error) means for the socket.
/// `ENOBUFS` waits and sets `kernel_nobufs`; `EFAULT` aborts, as upstream does.
pub fn catch_error(error: c_int, kernel_nobufs: &mut bool) -> c_int {
    #[expect(
        unreachable_patterns,
        reason = "EWOULDBLOCK equals EAGAIN on these platforms"
    )]
    match error {
        libc::EFAULT => swoole_rs_corebase::fatal("a socket call got EFAULT, a bad buffer address"),
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

/// The call `Socket::ssl_sendfile` warns failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SslSendfileWarning {
    Write,
    Pread,
}

/// `Socket::ssl_sendfile`'s outcome: what it returns, and the warning the caller
/// writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SslSendfileResult {
    pub value: isize,
    pub warn: Option<SslSendfileWarning>,
}

/// `Socket::ssl_sendfile`: pread(2) up to `SW_BUFFER_SIZE_BIG` bytes of `file_fd`
/// at `*offset`, hand them to `send` (the socket's TLS write) once, and advance
/// `*offset` by what it sent. A failed send warns when `catch_error` says it is an error.
pub fn ssl_sendfile(
    file_fd: c_int,
    offset: &mut libc::off_t,
    size: usize,
    send: impl FnOnce(&[u8]) -> isize,
    kernel_nobufs: &mut bool,
) -> SslSendfileResult {
    const CHUNK: usize = 65536;
    let mut buffer = vec![0_u8; size.min(CHUNK)];
    let at = *offset;
    let n = swoole_rs_corebase::sys::pread(file_fd, &mut buffer, at);
    if n <= 0 {
        return SslSendfileResult {
            value: -1,
            warn: Some(SslSendfileWarning::Pread),
        };
    }
    // pread(2) fills at most the buffer; a longer answer sends the buffer.
    let sent = send(buffer.get(..n.cast_unsigned()).unwrap_or(&buffer));
    if sent < 0 {
        let error = Error::last_os_error().raw_os_error().unwrap_or(0);
        let failed = catch_error(error, kernel_nobufs) == ERROR;
        return SslSendfileResult {
            value: sent,
            warn: failed.then_some(SslSendfileWarning::Write),
        };
    }
    *offset = at + libc::off_t::try_from(sent).unwrap_or(libc::off_t::MAX);
    SslSendfileResult {
        value: sent,
        warn: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;
    use std::os::raw::c_void;

    fn pair() -> [c_int; 2] {
        let mut fds = [0; 2];
        // SAFETY: `fds` holds the two descriptors socketpair writes.
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) },
            0
        );
        fds
    }

    thread_local! {
        /// The bytes [`record_tls_in_pieces`] has left on this thread.
        static LEFT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
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

    /// [`tls_in_pieces`] for a record's read, whose socket is the record: the counter
    /// is this thread's [`LEFT`].
    unsafe extern "C" fn record_tls_in_pieces(
        _socket: *mut c_void,
        buffer: *mut c_void,
        length: usize,
    ) -> isize {
        LEFT.with(|left| unsafe { tls_in_pieces(left.as_ptr().cast(), buffer, length) })
    }

    unsafe extern "C" fn no_tls(_: *mut c_void, _: *mut c_void, _: usize) -> isize {
        unreachable!("plain sockets never take the TLS call")
    }

    /// No TLS calls, for a vector over a plain descriptor.
    type Read = fn(crate::Fill<'_>) -> isize;
    type Write = fn(crate::Drain<'_>) -> isize;
    const NO_TLS: Option<&(Read, Write)> = None;

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no datagram socketpair")]
    fn plain_send_recv_and_peek() {
        use crate::lent::raw::{socket_record_peek, socket_record_recv, socket_record_send};
        use crate::{Socket, SocketRecord, steady_ms};
        let [mut a, mut b] = pair().map(|fd| SocketRecord::with_fd(fd, 0));
        let (a, b) = (Socket::new(&mut a), Socket::new(&mut b));
        // SAFETY: local buffers, no TLS.
        unsafe {
            assert_eq!(
                socket_record_send(b, b"hello".as_ptr().cast(), 5, 0, no_tls, steady_ms),
                5
            );
            let mut buffer = [0_u8; 8];
            assert_eq!(
                socket_record_peek(a, buffer.as_mut_ptr().cast(), 8, 0, no_tls),
                5
            );
            assert_eq!(
                socket_record_recv(a, buffer.as_mut_ptr().cast(), 8, 0, no_tls, steady_ms),
                5
            );
            assert_eq!(&buffer[..5], b"hello");
            libc::close(a.fd());
            libc::close(b.fd());
        }
    }

    #[test]
    fn tls_reads_accumulate_only_when_nonblocking_or_waitall() {
        use crate::lent::raw::socket_record_recv;
        use crate::{Socket, SocketRecord, steady_ms};
        let mut buffer = [0_u8; 8];
        LEFT.set(7);
        let mut record = SocketRecord::with_fd(-1, 0);
        // SAFETY: the stub never reads the `SSL`; the record is forgotten, not freed.
        unsafe { record.set_ssl(std::ptr::dangling_mut()) };
        let socket = Socket::new(&mut record);
        let mut recv = |nonblock: bool| {
            socket.edit(|record| record.output.nonblock = nonblock);
            let at = buffer.as_mut_ptr().cast();
            // SAFETY: local buffer; the stub writes within it.
            unsafe { socket_record_recv(socket, at, 8, 0, record_tls_in_pieces, steady_ms) }
        };
        assert_eq!(recv(false), 2);
        assert_eq!(recv(true), 5);
        assert_eq!(recv(true), 0);
        // SAFETY: the dangling `SSL` goes before anything could free it.
        socket.edit(|record| unsafe { record.set_ssl(std::ptr::null_mut()) });
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
        use crate::{ReactorIov, swr_v1_reactor_iov_view, swr_v1_socket_iov_owned};
        let [a, b] = pair();
        let (mut first, mut second) = (*b"abc", *b"defg");
        let iov = [
            libc::iovec {
                iov_base: first.as_mut_ptr().cast(),
                iov_len: 3,
            },
            libc::iovec {
                iov_base: second.as_mut_ptr().cast(),
                iov_len: 4,
            },
        ];
        let mut owner = ReactorIov::new(&iov).unwrap();
        // SAFETY: the cursor names the two local iovecs.
        let sent = unsafe { swr_v1_socket_iov_owned(a, true, &mut owner, NO_TLS) };
        let state = swr_v1_reactor_iov_view(&owner);
        assert_eq!((sent, state.remain_count), (7, 0));

        let mut into = [0_u8; 5];
        let mut tail = [0_u8; 5];
        let iov = [
            libc::iovec {
                iov_base: into.as_mut_ptr().cast(),
                iov_len: 5,
            },
            libc::iovec {
                iov_base: tail.as_mut_ptr().cast(),
                iov_len: 5,
            },
        ];
        let mut owner = ReactorIov::new(&iov).unwrap();
        // SAFETY: as above, writable.
        let read = unsafe { swr_v1_socket_iov_owned(b, false, &mut owner, NO_TLS) };
        let state = swr_v1_reactor_iov_view(&owner);
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
        let iov = [
            libc::iovec {
                iov_base: into.as_mut_ptr().cast(),
                iov_len: 4,
            },
            libc::iovec {
                iov_base: tail.as_mut_ptr().cast(),
                iov_len: 6,
            },
        ];
        let mut owner = ReactorIov::new(&iov).unwrap();
        // The counter stands in for the socket; the iovecs are local.
        let counter = (&raw mut left).cast();
        let read = unsafe {
            swr_v1_socket_iov_owned(
                -1,
                false,
                &mut owner,
                Some(&(
                    |mut fill: crate::Fill<'_>| {
                        let slots = fill.slots();
                        tls_in_pieces(counter, slots.as_mut_ptr().cast(), slots.len())
                    },
                    |_: crate::Drain<'_>| unreachable!("a read"),
                )),
            )
        };
        let state = swr_v1_reactor_iov_view(&owner);
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

    fn sends_three(data: &[u8]) -> isize {
        data.len().min(3).cast_signed()
    }

    #[test]
    fn ssl_sendfile_reads_then_sends() {
        let path = std::env::temp_dir().join(format!("swr-ssl-sendfile-{}", std::process::id()));
        std::fs::write(&path, b"0123456789").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let mut offset: libc::off_t = 4;
        let mut nobufs = false;
        let result = ssl_sendfile(file.as_raw_fd(), &mut offset, 6, sends_three, &mut nobufs);
        assert_eq!(
            (result, offset),
            (
                SslSendfileResult {
                    value: 3,
                    warn: None
                },
                7
            )
        );
        offset = 10;
        // Nothing is left to read.
        let result = ssl_sendfile(file.as_raw_fd(), &mut offset, 6, sends_three, &mut nobufs);
        assert_eq!(
            result,
            SslSendfileResult {
                value: -1,
                warn: Some(SslSendfileWarning::Pread)
            }
        );
        std::fs::remove_file(&path).unwrap();
    }
}
