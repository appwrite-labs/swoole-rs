//! `network::Stream`: one request framed as `Length(32bit, network order) + Body` over a fresh
//! connection, then the peer's framed replies until it closes. A socket pool dispatches over it
//! to its stream listener. The connect is non-blocking, like upstream's async client's: a failure
//! the kernel reports at once fails [`Stream::create`]; a later one (a refused TCP connect) is
//! reported when the stream flushes.
use std::ffi::c_int;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::SocketAddrV4;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use swoole_rs_corebase::{log, sys};

/// `SW_INPUT_BUFFER_SIZE`: the client's default `package_max_length`.
const MAX_LENGTH: u32 = 2 * 1024 * 1024;
/// `SW_LOG_WARNING`.
const LOG_WARNING: c_int = 4;
/// `SW_ERROR_SERVER_CONNECT_FAIL`.
const SERVER_CONNECT_FAIL: c_int = 9011;

/// Where a stream connects: a Unix stream socket's path, or a TCP address.
pub enum Peer<'a> {
    Unix(&'a [u8]),
    Tcp(SocketAddrV4),
}

pub struct Stream {
    socket: File,
    /// The request's body.
    buffer: Vec<u8>,
    /// `set_max_length`: the largest reply, its length field included.
    pub max_length: u32,
    /// `errCode`: the connect's error.
    pub err_code: c_int,
}

fn errno() -> c_int {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `Stream_onError`'s warning.
fn connect_failed(code: c_int) {
    log::error_log(
        LOG_WARNING,
        SERVER_CONNECT_FAIL,
        c"Stream_onError",
        format_args!(
            " connect() failed ({code}: {}) while connecting to worker process",
            sys::strerror(code)
        ),
    );
}

fn set_flags(fd: c_int, get: c_int, set: c_int, flag: c_int, on: bool) -> io::Result<()> {
    let flags = sys::fcntl(fd, get, 0);
    let flags = if on { flags | flag } else { flags & !flag };
    if flags < 0 || sys::fcntl(fd, set, flags) < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// A non-blocking connect; `EINPROGRESS` is success.
fn connect<T>(family: c_int, address: &T) -> io::Result<OwnedFd> {
    // SAFETY: plain socket(2).
    let fd = unsafe { libc::socket(family, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a fresh descriptor nothing else owns.
    let socket = unsafe { OwnedFd::from_raw_fd(fd) };
    set_flags(fd, libc::F_GETFD, libc::F_SETFD, libc::FD_CLOEXEC, true)?;
    set_flags(fd, libc::F_GETFL, libc::F_SETFL, libc::O_NONBLOCK, true)?;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a sockaddr's size fits socklen_t"
    )]
    let length = size_of::<T>() as libc::socklen_t;
    // SAFETY: `address` is a whole sockaddr of `family`, `length` bytes long.
    if unsafe { libc::connect(fd, (&raw const *address).cast(), length) } < 0
        && errno() != libc::EINPROGRESS
    {
        return Err(io::Error::last_os_error());
    }
    Ok(socket)
}

fn open(peer: &Peer) -> io::Result<OwnedFd> {
    match peer {
        Peer::Unix(path) => {
            // SAFETY: sockaddr_un is plain data; all zeroes is a valid value.
            let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
            address.sun_family = libc::sa_family_t::try_from(libc::AF_UNIX).unwrap_or_default();
            if path.len() >= address.sun_path.len() {
                return Err(io::ErrorKind::InvalidInput.into());
            }
            for (to, from) in address.sun_path.iter_mut().zip(path.iter()) {
                *to = libc::c_char::from_ne_bytes([*from]);
            }
            connect(libc::AF_UNIX, &address)
        }
        Peer::Tcp(peer) => {
            // SAFETY: sockaddr_in is plain data; all zeroes is a valid value.
            let mut address: libc::sockaddr_in = unsafe { std::mem::zeroed() };
            address.sin_family = libc::sa_family_t::try_from(libc::AF_INET).unwrap_or_default();
            address.sin_port = peer.port().to_be();
            address.sin_addr.s_addr = u32::from(*peer.ip()).to_be();
            connect(libc::AF_INET, &address)
        }
    }
}

impl Stream {
    /// `Stream::create`: None, with upstream's two warnings, when the connect fails at once.
    #[must_use]
    pub fn create(peer: &Peer) -> Option<Self> {
        match open(peer) {
            Ok(socket) => Some(Self {
                socket: File::from(socket),
                buffer: Vec::new(),
                max_length: MAX_LENGTH,
                err_code: 0,
            }),
            Err(error) => {
                let code = error.raw_os_error().unwrap_or(0);
                connect_failed(code);
                let name = match peer {
                    Peer::Unix(path) => format!("{}:0", String::from_utf8_lossy(path)),
                    Peer::Tcp(address) => address.to_string(),
                };
                log::sys_warning_errno(
                    code,
                    c"Stream",
                    format_args!("failed to connect to [{name}]"),
                );
                None
            }
        }
    }

    /// `Stream::send`: append to the request.
    pub fn send(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
    }

    /// The connect's outcome, then the request (upstream's `Stream_onConnect`). False when the
    /// connect failed: its error is in `err_code`, logged as `Stream_onError` does.
    pub fn flush(&mut self) -> bool {
        let fd = self.socket.as_raw_fd();
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        while sys::poll(std::slice::from_mut(&mut poll), -1) < 0 && errno() == libc::EINTR {}
        let mut error: c_int = 0;
        let got = sys::getsockopt_int(fd, libc::SOL_SOCKET, libc::SO_ERROR, &mut error);
        self.err_code = if got < 0 { errno() } else { error };
        if self.err_code != 0 {
            connect_failed(self.err_code);
            return false;
        }
        // The body's length, in upstream's uint32_t; a failed write closes the stream, as upstream's.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream's uint32_t length"
        )]
        let head = (self.buffer.len() as u32).to_be_bytes();
        let _closed = set_flags(fd, libc::F_GETFL, libc::F_SETFL, libc::O_NONBLOCK, false)
            .and_then(|()| self.socket.write_all(&head))
            .and_then(|()| self.socket.write_all(&self.buffer));
        true
    }

    /// Flush, then hand each reply's body to `response` until the peer closes (upstream's event
    /// loop over `Stream_onReceive`). A failed connect is one `response(None)`. A zero-length reply
    /// ends the stream once the peer closes; one over `max_length` closes it.
    pub fn run(mut self, mut response: impl FnMut(&Self, Option<&[u8]>)) {
        if !self.flush() {
            response(&self, None);
            return;
        }
        loop {
            let mut head = [0; 4];
            if self.socket.read_exact(&mut head).is_err() {
                return;
            }
            let length = u32::from_be_bytes(head);
            if length == 0 {
                continue;
            }
            if length.saturating_add(4) > self.max_length {
                return;
            }
            let mut body = vec![0; length as usize];
            if self.socket.read_exact(&mut body).is_err() {
                return;
            }
            response(&self, Some(&body));
        }
    }
}

/// `Stream::recv_sync`: one framed packet into `buf`. Its length, or -1 (`SW_ERR`) for a short
/// read or a length that is not positive as a C `int` or exceeds `buf`.
pub fn recv_sync(socket: &mut impl Read, buf: &mut [u8]) -> isize {
    let mut head = [0; 4];
    if socket.read_exact(&mut head).is_err() {
        return -1;
    }
    let length = usize::try_from(i32::from_be_bytes(head)).unwrap_or(0);
    match buf.get_mut(..length) {
        Some(body) if length > 0 => socket
            .read_exact(body)
            .map_or(-1, |()| length.cast_signed()),
        _ => -1,
    }
}

/// Upstream's `core-tests/src/network/stream.cpp`, one test per TEST, its inputs and expected values.
#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Shutdown, TcpListener};
    use std::os::unix::net::UnixStream;

    #[test]
    #[cfg_attr(miri, ignore = "Miri can't shut down a socketpair")]
    fn recv_sync_rejects_truncated_packet() {
        let mut buffer = [0u8; 2048];

        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        let packet_len = u32::try_from(buffer.len()).unwrap().to_be_bytes();
        sender.write_all(&packet_len[..3]).unwrap();
        sender.shutdown(Shutdown::Write).unwrap();
        assert_eq!(recv_sync(&mut receiver, &mut buffer), -1);

        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        sender.write_all(&8u32.to_be_bytes()).unwrap();
        sender.write_all(b"ab").unwrap();
        sender.shutdown(Shutdown::Write).unwrap();
        assert_eq!(recv_sync(&mut receiver, &mut buffer), -1);
    }

    /// The server replies as upstream's test server does: `"Server: " + body`, framed, then a
    /// zero-length frame. Upstream's serves it with a length-checked `Server` port.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no network sockets")]
    fn send() {
        let buf: Vec<u8> = (0..65536u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let Ok(std::net::SocketAddr::V4(svr)) = listener.local_addr() else {
            unreachable!()
        };
        let expected = buf.clone();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut head = [0; 4];
            conn.read_exact(&mut head).unwrap();
            let mut req_body = vec![0; u32::from_be_bytes(head) as usize];
            conn.read_exact(&mut req_body).unwrap();
            assert_eq!(req_body, expected);
            let pkt = [b"Server: ".as_slice(), &req_body].concat();
            conn.write_all(&u32::try_from(pkt.len()).unwrap().to_be_bytes())
                .unwrap();
            conn.write_all(&pkt).unwrap();
            conn.write_all(&0u32.to_be_bytes()).unwrap();
        });

        // bad request: a regular file, not a socket (upstream's TEST_TMP_FILE)
        let file = std::env::temp_dir().join(format!("swr-stream-{}", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let stream0 = Stream::create(&Peer::Unix(file.as_os_str().as_encoded_bytes()));
        std::fs::remove_file(&file).unwrap();
        assert!(stream0.is_none());

        // bad request
        let stream1 = Stream::create(&Peer::Tcp(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 39999)));
        assert!(stream1.is_some());
        let mut stream1 = stream1.unwrap();
        stream1.send(&buf);
        let mut responses = 0;
        stream1.run(|stream, data| {
            responses += 1;
            assert_eq!(data, None);
            assert_eq!(stream.err_code, libc::ECONNREFUSED);
        });
        assert_eq!(responses, 1);

        // success request
        let stream2 = Stream::create(&Peer::Tcp(svr));
        assert!(stream2.is_some());
        let mut stream2 = stream2.unwrap();
        stream2.max_length = 8 * 1024 * 1024;
        stream2.send(&buf);
        let mut responses = 0;
        stream2.run(|_, data| {
            responses += 1;
            assert_eq!(
                data,
                Some([b"Server: ".as_slice(), &buf].concat().as_slice())
            );
        });
        assert_eq!(responses, 1);
        server.join().unwrap();
    }
}
