//! Reactor threads' worker-pipe write path (`src/server/reactor_thread.cc`).
use core::ffi::c_int;

use swoole_rs_corebase::reactor::{Event, Heap, SocketFields};
use swoole_rs_corebase::{DataHead, ServerEvent, log, microtime};
use swoole_rs_error::ErrorCode as E;

use crate::reactor_handle::Reactor;
use crate::records::Lookup;
use crate::{ServerState, view};
use swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING;

/// `ReactorThread_onPipeWrite`: flush the queued messages to a worker, dropping
/// those for expired sessions when configured; false when the pipe failed.
pub(crate) fn pipe_write(serv: view::Server<'_>, reactor: Reactor, event: &Event) -> bool {
    let Some(sock) = event.socket() else {
        return false;
    };
    if !flush_pipe(serv, sock) {
        return false;
    }
    if !sock.read(SocketFields::output_drained) {
        return true;
    }
    if !reactor.interest(sock, swoole_rs_corebase::reactor::Interest::RemoveWrite) {
        let m = format_args!("reactor->set({}) failed", event.fd());
        log::sys_warning(c"ReactorThread_onPipeWrite", m);
    }
    true
}

fn flush_pipe(serv: view::Server<'_>, sock: Heap<'_>) -> bool {
    // The front message, sent (or discarded) and popped while the queue is lent;
    // `Err(errno)` when the send failed.
    let flush_front = |queue: Option<&mut swoole_rs_corebase::buffer::Buffer>| {
        let queue = queue.filter(|queue| queue.stats().1 != 0)?;
        let message = queue.front_data();
        if !discard(serv.state(), message)
            && swoole_rs_corebase::tls::upstream::send(*sock, message, 0).is_none()
        {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            return Some(Err(errno));
        }
        Some(Ok(queue.pop().ok()))
    };
    while let Some(sent) = sock.with_output_queue(flush_front) {
        match sent {
            Ok(popped) => popped.map_or((), swoole_rs_corebase::buffer::TypedDetached::dispose),
            Err(errno) => {
                return sock.catch_error(errno) == swoole_rs_corebase::reactor::Caught::Wait;
            }
        }
    }
    true
}

/// After PHP request shutdown, service only the worker's native pipe queues
/// until they drain or the original worker exit deadline expires.
pub(crate) fn finish_worker_output(serv: view::Server<'_>, bus: crate::objects::Bus<'_>) {
    let mut pending: Vec<_> = bus
        .rust()
        .pipes()
        .filter(|pipe| !pipe.read(SocketFields::output_drained))
        .map(|pipe| libc::pollfd {
            fd: pipe.fd(),
            events: libc::POLLOUT,
            revents: 0,
        })
        .collect();
    while !pending.is_empty() {
        let timeout = crate::event_worker::native::exit_timeout(serv.config().max_wait_time.get());
        if timeout == 0 {
            break;
        }
        match swoole_rs_corebase::sys::poll(&mut pending, timeout) {
            Ok(0) => break,
            Ok(_) => pending.retain(|fd| {
                fd.revents == 0
                    || bus.rust().pipe(fd.fd).is_some_and(|pipe| {
                        flush_pipe(serv, pipe) && !pipe.read(SocketFields::output_drained)
                    })
            }),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    for pipe in bus.rust().pipes() {
        pipe.with_output_queue(discard_queue);
    }
}

/// Discard packets for missing sessions when configured. Workers apply close ordering
/// to packets already queued for sessions still present in the connection table.
fn discard(state: &ServerState, message: &[u8]) -> bool {
    let Some((header, payload)) = message.split_first_chunk::<{ size_of::<DataHead>() }>() else {
        return false;
    };
    let head = DataHead::from_bytes(header);
    if !head.event().is_some_and(ServerEvent::is_stream) {
        return false;
    }
    if let Some(conn) = state.connection(head.fd, Lookup::VerifiedTls) {
        conn.set_last_send_time(microtime());
        return false;
    }
    if !state.config_ref().discard_timeout_request.get() {
        return false;
    }
    let m = format_args!(
        "[1] ignore data[{} bytes] received from session#{}",
        head.len, head.fd
    );
    let code = E::SW_ERROR_SESSION_DISCARD_TIMEOUT_DATA as c_int;
    log::error_log(SW_LOG_WARNING, code, c"ReactorThread_onPipeWrite", m);
    crate::send::discard_file(&head, payload);
    true
}

/// Release file handoffs after the workers and reactors have stopped, before their
/// pipe descriptors are closed. At this point no receiver can consume a handoff.
pub(crate) fn discard_shutdown_files(serv: view::Server<'_>) {
    let state = serv.state();
    let tables = state.tables_ref();
    let config = serv.config();
    let master = tables.message_bus.as_deref().map(crate::objects::Bus::over);
    let threads = (0..config.reactor_num.get()).filter_map(|id| {
        tables
            .reactor_thread(usize::from(id))
            .map(|thread| thread.message_bus())
    });
    for bus in master.into_iter().chain(threads) {
        for pipe in bus.rust().pipes() {
            pipe.with_output_queue(discard_queue);
        }
    }
    let mut packet = vec![0; config.ipc_max_size() as usize];
    for id in (0..config.worker_num.get()).map_while(|id| u16::try_from(id).ok()) {
        if let Some(pipe) = serv.worker_pipe(id, true) {
            discard_unread(pipe.fd(), &mut packet);
        }
    }
}

fn discard_queue(queue: Option<&mut swoole_rs_corebase::buffer::Buffer>) {
    let Some(queue) = queue else { return };
    while queue.stats().1 != 0 {
        discard_packet(queue.front_data());
        let Ok(entry) = queue.pop() else { break };
        entry.dispose();
    }
}

fn discard_unread(fd: c_int, packet: &mut [u8]) {
    loop {
        match swoole_rs_corebase::sys::recv(fd, packet, libc::MSG_DONTWAIT) {
            Some(0) => break,
            Some(n) => discard_packet(packet.get(..n).unwrap_or_default()),
            None if swoole_rs_corebase::sys::errno() == libc::EINTR => {}
            None => break,
        }
    }
}

fn discard_packet(packet: &[u8]) {
    if let Some((header, payload)) = packet.split_first_chunk::<{ size_of::<DataHead>() }>() {
        crate::send::discard_file(&DataHead::from_bytes(header), payload);
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
    use core::ffi::c_long;
    use core::sync::atomic::Ordering::SeqCst;

    const SW_OK: c_int = swoole_rs_corebase::constants::ResultCode::SW_OK as c_int;
    const SW_ERR: c_int = swoole_rs_corebase::constants::ResultCode::SW_ERR as c_int;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixDatagram;
    use swoole_rs_corebase::reactor::SocketRecord;

    use swoole_rs_corebase::ServerEvent::{RecvDgram, SendData};
    const FD_PIPE: c_int = swoole_rs_corebase::constants::FdType::SW_FD_PIPE as c_int;

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
        [head.to_bytes().as_slice(), payload].concat()
    }

    /// The live server with session 1 accepted, and a worker pipe: the reactor's end,
    /// whose output queue holds the messages, and the worker's.
    struct Pipe {
        live: LiveServer,
        _client: std::net::TcpStream,
        ours: UnixDatagram,
        theirs: UnixDatagram,
        sock: swoole_rs_corebase::reactor::Heap<'static>,
    }

    impl Pipe {
        fn new(messages: &[Vec<u8>]) -> Self {
            let live = LiveServer::new();
            let client = live.connect();
            let event = live.accept_event();
            crate::connection::accept(live.server(), live.reactor(), &event);
            let (ours, theirs) = UnixDatagram::pair().unwrap();
            ours.set_nonblocking(true).unwrap();
            theirs.set_nonblocking(true).unwrap();
            let sock = Box::new(SocketRecord::with_fd(ours.as_raw_fd(), FD_PIPE)).into_socket();
            sock.edit(|s| {
                s.open_output_queue(0);
                for m in messages {
                    s.queue_output(m);
                }
            });
            Self {
                live,
                _client: client,
                ours,
                theirs,
                sock,
            }
        }
        /// The handler's status, as upstream returns it.
        fn write(&self) -> c_int {
            let event = Event::of(self.ours.as_raw_fd(), 0, FD_PIPE, Some(self.sock));
            if pipe_write(self.live.server(), self.live.reactor(), &event) {
                SW_OK
            } else {
                SW_ERR
            }
        }
        fn conn(&self) -> crate::records::Connection<'_> {
            self.live.state().connection(1, Lookup::Verified).unwrap()
        }
        /// The bytes still queued.
        fn queued(&self) -> usize {
            self.sock.read(|s| s.queued_bytes() as usize)
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
    #[cfg_attr(miri, ignore = "Miri's socketpair is stream-only")]
    fn pipe_write_sends_every_queued_message() {
        let messages = [
            message(SendData, 1, b"live"),
            message(RecvDgram, 9, b"dgram"),
            message(SendData, 9, b"gone"),
        ];
        let p = Pipe::new(&messages);
        assert_eq!(p.write(), SW_OK);
        assert_eq!(p.received(), messages);
        assert_eq!(p.queued(), 0);
        assert!(
            p.conn().last_send_time() > 0.0,
            "a live session's send time moves on"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri's socketpair is stream-only")]
    fn pipe_write_keeps_close_order_and_drops_missing_sessions() {
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
        assert_eq!(p.write(), SW_OK);
        assert_eq!(p.received(), [messages[0].clone(), messages[2].clone()]);
        assert_eq!(p.queued(), 0);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri's socketpair is stream-only")]
    fn timed_out_pipe_packets_release_only_owned_file_names() {
        use swoole_rs_corebase::constants::EventDataFlag as Flag;
        let path =
            std::env::temp_dir().join(format!("swoole-pipe-sendfile-{}", std::process::id()));
        let mut payload = vec![0; 16];
        payload.extend(path.as_os_str().as_encoded_bytes());
        payload.push(0);
        let chunk = Flag::SW_EVENT_DATA_CHUNK.bits();
        let begin = Flag::SW_EVENT_DATA_BEGIN.bits();
        for (flags, owned, removed) in [
            (0, false, false),
            (chunk, true, false),
            (chunk | begin, true, true),
            (0, true, true),
        ] {
            std::fs::write(&path, b"abc").unwrap();
            let header = DataHead {
                fd: 9,
                kind: ServerEvent::SendFile.code(),
                flags,
                ext_flags: if owned {
                    crate::send::SENDFILE_DELETE
                } else {
                    0
                },
                len: u32::try_from(payload.len()).unwrap(),
                ..DataHead::default()
            };
            let packet = [header.to_bytes().as_slice(), &payload].concat();
            let p = Pipe::new(&[packet]);
            p.live
                .state()
                .config_ref()
                .discard_timeout_request
                .set(true);
            assert_eq!(p.write(), SW_OK);
            assert_eq!(p.received(), Vec::<Vec<u8>>::new());
            assert_eq!(path.exists(), !removed);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri's socketpair is stream-only")]
    fn shutdown_releases_files_in_buffered_and_unread_packets() {
        let (sender, receiver) = UnixDatagram::pair().unwrap();
        let mut queue = swoole_rs_corebase::buffer::Buffer::new(4096);
        let dir = std::env::temp_dir();
        let paths = ["buffered", "unread", "retained"]
            .map(|kind| dir.join(format!("swoole-shutdown-{kind}-{}", std::process::id())));
        for (i, path) in paths.iter().enumerate() {
            std::fs::write(path, b"abc").unwrap();
            let mut task = vec![0; 16];
            task.extend(path.as_os_str().as_encoded_bytes());
            task.push(0);
            let header = DataHead {
                kind: ServerEvent::SendFile.code(),
                len: u32::try_from(task.len()).unwrap(),
                ext_flags: if i < 2 {
                    crate::send::SENDFILE_DELETE
                } else {
                    0
                },
                ..DataHead::default()
            };
            let packet = [header.to_bytes().as_slice(), &task].concat();
            if i == 0 {
                queue.append(&packet).unwrap();
            } else {
                sender.send(&packet).unwrap();
            }
        }
        discard_queue(Some(&mut queue));
        discard_unread(receiver.as_raw_fd(), &mut [0; 4096]);
        assert_eq!(
            paths.each_ref().map(|path| path.exists()),
            [false, false, true]
        );
        assert_eq!(queue.stats(), (0, 0));
        std::fs::remove_file(&paths[2]).unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri's socketpair is stream-only")]
    fn a_full_pipe_waits_and_a_closed_one_fails() {
        let later = message(SendData, 9, b"later");
        let p = Pipe::new(std::slice::from_ref(&later));
        while p.ours.send(b"fill").is_ok() {}
        assert_eq!(p.write(), SW_OK, "it waits for the next writable event");
        assert_eq!(p.queued(), later.len(), "with the message still queued");
        let never = message(SendData, 9, b"never");
        let p = Pipe::new(std::slice::from_ref(&never));
        p.ours.shutdown(std::net::Shutdown::Write).unwrap();
        assert_eq!(p.write(), SW_ERR);
        assert_eq!(p.queued(), never.len());
    }
}
