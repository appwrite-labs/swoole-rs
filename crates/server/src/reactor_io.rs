//! Reactor threads' worker-pipe write path (`src/server/reactor_thread.cc`).
use core::ffi::{c_int, c_long};
use core::mem::offset_of;
use core::sync::atomic::Ordering::SeqCst;

use swoole_rs_buffer::cpp::{swr_cpp_buffer_empty, swr_cpp_buffer_pop, swr_cpp_buffer_typed_front};
use swoole_rs_corebase::{DataHead, ServerEvent, log, microtime};
use swoole_rs_error::ErrorCode as E;
use swoole_rs_reactor::{Event, Socket};

use crate::reactor_handle::Reactor;
use crate::reactor_ops::out_buffer;
use crate::records::Lookup;
use crate::{SW_ERR, SW_OK, ServerState, view};

/// `SW_LOG_NOTICE`, `SW_LOG_WARNING` and `SW_WAIT`.
const SW_LOG_NOTICE: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_NOTICE as c_int;
const SW_LOG_WARNING: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as c_int;
const SW_WAIT: c_int = swoole_rs_corebase::constants::ReturnCode::SW_WAIT as c_int;

/// `ReactorThread_onPipeWrite`: flush the queued messages to a worker, dropping
/// those for connections the server already closed.
pub(crate) fn pipe_write(serv: view::Server<'_>, reactor: Reactor, event: &Event) -> c_int {
    let Some(sock) = event.socket() else {
        return int(SW_ERR);
    };
    while let Some(front) = sock.read(|s| {
        let buffer = out_buffer(s).filter(|b| !swr_cpp_buffer_empty(b))?;
        Some(swr_cpp_buffer_typed_front(buffer))
    }) {
        // SAFETY: the front entry's bytes, a worker message that stays queued, and
        // so live, until this thread pops it.
        let message = unsafe { core::slice::from_raw_parts(front.data, front.length as usize) };
        if !discard(serv.state(), message) && swoole_rs_tls::upstream::send(sock, message, 0) < 0 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            return int(if sock.catch_error(errno) == SW_WAIT {
                SW_OK
            } else {
                SW_ERR
            });
        }
        pop(sock);
    }
    if reactor.interest(sock, swoole_rs_reactor::Interest::RemoveWrite, 0) < 0 {
        let m = format_args!("reactor->set({}) failed", event.fd());
        log::sys_warning(c"ReactorThread_onPipeWrite", m);
    }
    int(SW_OK)
}

fn int(ret: i64) -> c_int {
    c_int::try_from(ret).unwrap_or(-1)
}

fn pop(sock: Socket<'_>) {
    sock.read(|s| out_buffer(s).map(|b| swr_cpp_buffer_pop(b)));
}

/// `N` bytes of `message` at `at`, zero past its end.
fn field<const N: usize>(message: &[u8], at: usize) -> [u8; N] {
    let bytes = message.get(at..).and_then(|rest| rest.get(..N));
    bytes.and_then(|b| b.try_into().ok()).unwrap_or([0; N])
}

/// Whether the worker's `message` is for a session the server already closed, or
/// one that's gone while the server discards timed-out data. A live session's last
/// send time moves on.
fn discard(state: &ServerState, message: &[u8]) -> bool {
    let [kind] = field(message, offset_of!(DataHead, kind));
    if !ServerEvent::from_code(kind).is_some_and(ServerEvent::is_stream) {
        return false;
    }
    let fd = c_long::from_ne_bytes(field(message, offset_of!(DataHead, fd)));
    let origin = c"ReactorThread_onPipeWrite";
    if let Some(conn) = state.connection(fd, Lookup::VerifiedTls) {
        conn.set_last_send_time(microtime());
        let closed = conn.closed().load(SeqCst) != 0;
        if closed {
            let m = format_args!("Session#{fd} is closed by server");
            let code = E::SW_ERROR_SESSION_CLOSED_BY_SERVER as c_int;
            log::error_log(SW_LOG_NOTICE, code, origin, m);
        }
        closed
    } else if state.config_ref().discard_timeout_request.get() {
        let len = u32::from_ne_bytes(field(message, offset_of!(DataHead, len)));
        let m = format_args!("[1] ignore data[{len} bytes] received from session#{fd}");
        let code = E::SW_ERROR_SESSION_DISCARD_TIMEOUT_DATA as c_int;
        log::error_log(SW_LOG_WARNING, code, origin, m);
        true
    } else {
        false
    }
}

/// Whether reactor thread `id` of `reactors` owns `n`, a port's fd or a worker id.
pub(crate) fn owns(n: i64, reactors: u16, id: u16) -> bool {
    reactors != 0 && n.rem_euclid(i64::from(reactors)) == i64::from(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::LiveServer;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixDatagram;
    use swoole_rs_buffer::cpp::swr_cpp_buffer_count;
    use swoole_rs_reactor::SocketRecord;

    use swoole_rs_corebase::ServerEvent::{RecvDgram, SendData};
    const FD_PIPE: c_int = 3;

    #[test]
    fn owns_splits_ports_and_workers_across_threads() {
        // Thread 1 of 2 binds the datagram ports at fds 5 and 7 and takes workers 1 and 3.
        let owned =
            |ns: &[i64]| -> Vec<i64> { ns.iter().copied().filter(|&n| owns(n, 2, 1)).collect() };
        assert_eq!(owned(&[4, 5, 7]), vec![5, 7]);
        assert_eq!(owned(&[0, 1, 2, 3]), vec![1, 3]);
        assert!(!owns(0, 0, 0));
    }

    /// A worker message of `kind` for `session`.
    fn message(kind: ServerEvent, session: c_long, payload: &[u8]) -> Vec<u8> {
        let head = DataHead {
            fd: session,
            kind: kind.code(),
            len: u32::try_from(payload.len()).unwrap(),
            ..DataHead::default()
        };
        // SAFETY: the head's bytes; it has no padding.
        let head = unsafe {
            core::slice::from_raw_parts((&raw const head).cast::<u8>(), size_of::<DataHead>())
        };
        [head, payload].concat()
    }

    /// The live server with session 1 accepted, and a worker pipe: the reactor's end,
    /// whose output queue holds the messages, and the worker's.
    struct Pipe {
        live: LiveServer,
        _client: std::net::TcpStream,
        ours: UnixDatagram,
        theirs: UnixDatagram,
        sock: Socket<'static>,
    }

    impl Pipe {
        fn new(messages: &[Vec<u8>]) -> Self {
            let live = LiveServer::new();
            let client = live.connect();
            // SAFETY: the fixture's listener, live for the event.
            let event = unsafe { Event::new(live.listener_fd, 0, 1, live.listener().cast()) };
            crate::connection::accept(live.server(), live.reactor(), &event);
            let (ours, theirs) = UnixDatagram::pair().unwrap();
            ours.set_nonblocking(true).unwrap();
            theirs.set_nonblocking(true).unwrap();
            let sock = Box::new(SocketRecord::with_fd(ours.as_raw_fd(), FD_PIPE)).into_socket();
            let buffer = swoole_rs_buffer::cpp::new_buffer(0);
            for m in messages {
                buffer.append(m);
            }
            sock.edit(|s| s.output.out_buffer = Some(buffer));
            Self {
                live,
                _client: client,
                ours,
                theirs,
                sock,
            }
        }
        fn write(&self) -> c_int {
            // SAFETY: the pipe's live record.
            let event =
                unsafe { Event::new(self.ours.as_raw_fd(), 0, FD_PIPE, self.sock.as_ptr().cast()) };
            pipe_write(self.live.server(), self.live.reactor(), &event)
        }
        fn conn(&self) -> crate::records::Connection<'_> {
            self.live.state().connection(1, Lookup::Verified).unwrap()
        }
        fn queued(&self) -> usize {
            self.sock
                .read(|s| out_buffer(s).map_or(0, |b| swr_cpp_buffer_count(b)))
        }
        /// The worker's messages so far.
        fn received(&self) -> Vec<Vec<u8>> {
            let mut got = Vec::new();
            let mut buf = [0; 256];
            while let Ok(n) = self.theirs.recv(&mut buf) {
                got.push(buf.get(..n).unwrap().to_vec());
            }
            got
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn pipe_write_sends_every_queued_message() {
        let messages = [
            message(SendData, 1, b"live"),
            message(RecvDgram, 9, b"dgram"),
            message(SendData, 9, b"gone"),
        ];
        let p = Pipe::new(&messages);
        assert_eq!(p.write(), int(SW_OK));
        assert_eq!(p.received(), messages);
        assert_eq!(p.queued(), 0);
        assert!(
            p.conn().last_send_time() > 0.0,
            "a live session's send time moves on"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn pipe_write_drops_closed_and_timed_out_sessions() {
        let messages = [
            message(SendData, 1, b"closed"),
            message(SendData, 9, b"gone"),
            message(RecvDgram, 9, b"dgram"),
        ];
        let p = Pipe::new(&messages);
        p.conn().closed().store(1, SeqCst);
        p.live
            .state()
            .config_ref()
            .discard_timeout_request
            .set(true);
        assert_eq!(p.write(), int(SW_OK));
        assert_eq!(p.received(), messages.get(2..).unwrap());
        assert_eq!(p.queued(), 0);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn a_full_pipe_waits_and_a_closed_one_fails() {
        let p = Pipe::new(&[message(SendData, 9, b"later")]);
        while p.ours.send(b"fill").is_ok() {}
        assert_eq!(
            p.write(),
            int(SW_OK),
            "it waits for the next writable event"
        );
        assert_eq!(p.queued(), 1, "with the message still queued");
        let p = Pipe::new(&[message(SendData, 9, b"never")]);
        p.ours.shutdown(std::net::Shutdown::Write).unwrap();
        assert_eq!(p.write(), int(SW_ERR));
        assert_eq!(p.queued(), 1);
    }
}
