//! `network::Socket`'s I/O pieces shared by its reads and writes: the TLS call a
//! record's `recv`, `send` and `peek` (`socket_methods.rs`) drive, their EINTR test,
//! `catch_error`'s errno classification and `ssl_sendfile`.
use crate::constants::ReturnCode;
use std::io::Error;
use std::os::raw::c_int;
use swoole_rs_error::ErrorCode;

pub(crate) fn interrupted() -> bool {
    Error::last_os_error().raw_os_error() == Some(libc::EINTR)
}

const SSL_BAD_CLIENT: c_int = ErrorCode::SW_ERROR_SSL_BAD_CLIENT as c_int;
const SSL_RESET: c_int = ErrorCode::SW_ERROR_SSL_RESET as c_int;

/// What [`catch_error`] makes of an errno: `swReturnCode`'s `SW_WAIT`, `SW_CLOSE`,
/// `SW_ERROR`, `SW_INVALID` and `SW_REDUCE_SIZE`, at their numbers.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caught {
    Wait = ReturnCode::SW_WAIT as i32,
    Close = ReturnCode::SW_CLOSE as i32,
    Error = ReturnCode::SW_ERROR as i32,
    Invalid = ReturnCode::SW_INVALID as i32,
    ReduceSize = ReturnCode::SW_REDUCE_SIZE as i32,
}

/// `Socket::catch_error`: what an errno (or Swoole TLS error) means for the socket.
/// `ENOBUFS` waits and sets `kernel_nobufs`; `EFAULT` aborts, as upstream does.
pub fn catch_error(error: c_int, kernel_nobufs: &mut bool) -> Caught {
    #[expect(
        unreachable_patterns,
        reason = "EWOULDBLOCK equals EAGAIN on these platforms"
    )]
    match error {
        libc::EFAULT => crate::fatal("a socket call got EFAULT, a bad buffer address"),
        libc::EBADF | libc::ENOENT => Caught::Invalid,
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
        | SSL_RESET => Caught::Close,
        libc::EAGAIN | libc::EWOULDBLOCK | 0 => Caught::Wait,
        libc::ENOBUFS => {
            *kernel_nobufs = true;
            Caught::Wait
        }
        _ => Caught::Error,
    }
}

/// `Socket::catch_write_pipe_error`: a pipe write that is too large reduces its
/// size (`EMSGSIZE`, and on Linux `ENOBUFS`, which also sets `kernel_nobufs`);
/// anything else is [`catch_error`]'s.
pub fn catch_pipe_error(error: c_int, kernel_nobufs: &mut bool) -> Caught {
    match error {
        libc::EMSGSIZE => Caught::ReduceSize,
        #[cfg(target_os = "linux")]
        libc::ENOBUFS => {
            *kernel_nobufs = true;
            Caught::ReduceSize
        }
        _ => catch_error(error, kernel_nobufs),
    }
}

/// An `IOVector`'s cursor: how many descriptors are left (the last ones), and the
/// descriptor index and byte offset where the transfers stopped.
#[derive(Clone, Copy)]
pub struct IovState {
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
    /// How many bytes `send` sent, or None (errno set) when the read or send failed.
    pub value: Option<usize>,
    pub warn: Option<SslSendfileWarning>,
}

/// `Socket::ssl_sendfile`: pread(2) up to `SW_BUFFER_SIZE_BIG` bytes of `file_fd`
/// at `*offset`, hand them to `send` (the socket's TLS write) once, and advance
/// `*offset` by what it sent. A failed send warns when `catch_error` says it is an error.
pub fn ssl_sendfile(
    file_fd: c_int,
    offset: &mut libc::off_t,
    size: usize,
    send: impl FnOnce(&[u8]) -> Option<usize>,
    kernel_nobufs: &mut bool,
) -> SslSendfileResult {
    let mut buffer = vec![0_u8; size.min(crate::constants::SW_BUFFER_SIZE_BIG as usize)];
    let at = *offset;
    let Some(n) = crate::sys::pread(file_fd, &mut buffer, at) else {
        return SslSendfileResult {
            value: None,
            warn: Some(SslSendfileWarning::Pread),
        };
    };
    if n == 0 {
        return SslSendfileResult {
            value: Some(0),
            warn: None,
        };
    }
    // pread(2) fills at most the buffer; a longer answer sends the buffer.
    let Some(sent) = send(buffer.get(..n).unwrap_or(&buffer)) else {
        let error = Error::last_os_error().raw_os_error().unwrap_or(0);
        let failed = catch_error(error, kernel_nobufs) == Caught::Error;
        return SslSendfileResult {
            value: None,
            warn: failed.then_some(SslSendfileWarning::Write),
        };
    };
    *offset = at + libc::off_t::try_from(sent).unwrap_or(libc::off_t::MAX);
    SslSendfileResult {
        value: Some(sent),
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
    type Read = fn(&mut crate::lent::Spare<'_>) -> Option<usize>;
    type Write = fn(crate::reactor::Drain<'_>) -> Option<usize>;
    const NO_TLS: Option<&(Read, Write)> = None;

    #[test]
    #[cfg_attr(miri, ignore = "Miri's socketpair can't peek")]
    fn plain_send_recv_and_peek() {
        use crate::reactor::lent::raw::{
            socket_record_peek, socket_record_recv, socket_record_send,
        };
        use crate::reactor::{Socket, SocketRecord, steady_ms};
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
        use crate::reactor::lent::raw::socket_record_recv;
        use crate::reactor::{Socket, SocketRecord, steady_ms};
        let mut buffer = [0_u8; 8];
        LEFT.set(7);
        let mut record = SocketRecord::with_fd(-1, 0);
        // The stub never reads the `SSL`.
        record.set_ssl(Some(crate::reactor::SocketSsl::stub()));
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
        // The dangling `SSL` goes unfreed.
        socket.edit(|record| record.set_ssl(None));
    }

    #[test]
    fn errors_classify_as_upstream() {
        let mut nobufs = false;
        let mut classify = |error| catch_error(error, &mut nobufs);
        assert_eq!(classify(libc::EAGAIN), Caught::Wait);
        assert_eq!(classify(0), Caught::Wait);
        assert_eq!(classify(libc::EPIPE), Caught::Close);
        assert_eq!(classify(SSL_RESET), Caught::Close);
        assert_eq!(classify(libc::EBADF), Caught::Invalid);
        assert_eq!(classify(libc::EINVAL), Caught::Error);
        assert_eq!(classify(libc::ENOBUFS), Caught::Wait);
        assert!(nobufs, "ENOBUFS records kernel_nobufs");
    }

    #[test]
    fn pipe_errors_reduce_the_size() {
        let mut nobufs = false;
        let mut classify = |error| catch_pipe_error(error, &mut nobufs);
        assert_eq!(classify(libc::EMSGSIZE), Caught::ReduceSize);
        assert_eq!(classify(libc::EAGAIN), Caught::Wait);
        assert_eq!(classify(libc::EPIPE), Caught::Close);
        let expected = if cfg!(target_os = "linux") {
            Caught::ReduceSize
        } else {
            Caught::Wait
        };
        assert_eq!(classify(libc::ENOBUFS), expected);
        assert!(nobufs, "ENOBUFS records kernel_nobufs");
    }

    #[test]
    fn iov_plain_and_tls() {
        use crate::reactor::{
            Plain, ReactorIov, ReadVec, swr_v1_reactor_iov_view, swr_v1_socket_iov_owned,
        };
        use std::io::IoSlice;
        let [a, b] = pair();
        let (first, second) = (*b"abc", *b"defg");
        let iov = [IoSlice::new(&first), IoSlice::new(&second)];
        let mut owner = ReactorIov::writing(&iov).unwrap();
        let sent = swr_v1_socket_iov_owned(Plain::Fd(a), &mut owner, NO_TLS);
        let state = swr_v1_reactor_iov_view(&owner);
        assert_eq!((sent, state.remain_count), (Some(7), 0));

        let mut into = [0_u8; 5];
        let mut tail = [0_u8; 5];
        let mut iov = [ReadVec::new(&mut into), ReadVec::new(&mut tail)];
        let mut owner = ReactorIov::reading(&mut iov).unwrap();
        let read = swr_v1_socket_iov_owned(Plain::Fd(b), &mut owner, NO_TLS);
        let state = swr_v1_reactor_iov_view(&owner);
        drop(owner);
        assert_eq!(read, Some(7));
        assert_eq!((&into, &tail[..2]), (b"abcde", &b"fg"[..]));
        assert_eq!(
            (state.remain_count, state.index, state.offset_bytes),
            (1, 1, 2)
        );

        // TLS: two bytes per call until 7 are gone, then a 0 that ends the loop.
        let mut left: usize = 7;
        let mut into = [0_u8; 4];
        let mut tail = [0_u8; 6];
        let mut iov = [ReadVec::new(&mut into), ReadVec::new(&mut tail)];
        let mut owner = ReactorIov::reading(&mut iov).unwrap();
        // The counter stands in for the socket; the iovecs are local.
        let counter: *mut c_void = (&raw mut left).cast();
        // The stub writes what it returns: the same call, with the tokens' layout and
        // its result as what it wrote.
        let stub: crate::reactor::lent::raw::TlsIo = tls_in_pieces;
        let tls: for<'id> unsafe extern "C" fn(
            *mut c_void,
            crate::lent::Out<'id>,
            crate::lent::Count<'id>,
        ) -> crate::lent::Wrote<'id> = unsafe { std::mem::transmute(stub) };
        let read = swr_v1_socket_iov_owned(
            Plain::Fd(-1),
            &mut owner,
            Some(&(
                |spare: &mut crate::lent::Spare<'_>| {
                    usize::try_from(spare.lend(|at, n| unsafe { tls(counter, at, n) })).ok()
                },
                |_: crate::reactor::Drain<'_>| unreachable!("a read"),
            )),
        );
        let state = swr_v1_reactor_iov_view(&owner);
        assert_eq!((read, left), (Some(7), 0));
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

    #[expect(
        clippy::unnecessary_wraps,
        reason = "a TLS send, whose type returns None on error"
    )]
    fn sends_three(data: &[u8]) -> Option<usize> {
        Some(data.len().min(3))
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
                    value: Some(3),
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
                value: Some(0),
                warn: None
            }
        );
        std::fs::remove_file(&path).unwrap();
    }
}
