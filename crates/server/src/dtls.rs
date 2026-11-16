//! A DTLS port's sessions on the reactor thread: `Server::accept_dtls_connection`
//! and the datagram and read steps over them. crates/tls owns each session's queue
//! and handshake, in its connection's SSL.
use core::ffi::{CStr, c_int};
use std::ffi::CString;
use swoole_rs_corebase::ServerEvent;
use swoole_rs_corebase::constants::{
    SW_BUFFER_SIZE_UDP, SW_SSL_BUFFER_SIZE, SocketFlag, SocketType,
};
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_reactor::Socket;
use swoole_rs_reactor::socket_upstream::make_typed;
use swoole_rs_tls::{
    DtlsSession, dtls_open, dtls_session_listen, dtls_session_receive, swr_v1_tls_dtls_append,
};

use crate::reactor_entry::DgramPacket;

/// `SW_FD_SESSION`.
const FD_SESSION: c_int = 0;

/// `setsockopt(fd, level, option, value)` for an int option.
fn set_option(fd: c_int, level: c_int, option: c_int, value: c_int) {
    drop(swoole_rs_corebase::sys::setsockopt(
        fd, level, option, &value,
    ));
}

/// `Socket::bind(host, port)`: the port's literal address, unresolved.
fn bind(sock: Socket<'_>, kind: c_int, host: &CStr, port: c_int) -> bool {
    let mut address = SocketAddress {
        kind,
        ..SocketAddress::default()
    };
    let assigned = swoole_rs_clients::address::assign(
        &mut address.addr,
        &mut address.len,
        kind,
        host.to_bytes_with_nul(),
        port,
        false,
    );
    if let Err(error) = assigned {
        log::set_last_error(error);
        return false;
    }
    sock.edit(|sock| sock.bind(&address))
}

/// `Server::accept_dtls_connection(port, sa)`: a socket bound to the port and
/// connected to the peer, registered as a connection with a DTLS session: the socket,
/// which the server's connection keeps until it closes, and its SSL the session;
/// `None`, with everything it made undone, when a step fails.
///
/// `listen` is one of `serv`'s DTLS ports.
fn accept(
    serv: crate::view::Server<'_>,
    listen: crate::view::Port<'_>,
    sa: &SocketAddress,
) -> Option<Socket<'static>> {
    let (kind, number, host) = {
        let config = listen.config();
        // NUL-terminated: the address parser reads the host as a C string.
        let host = CString::new(config.host_bytes()).unwrap_or_default();
        (config.r#type, config.port.get(), host)
    };
    let server_fd = listen.fd();
    let socket = make_typed(
        kind,
        FD_SESSION,
        (SocketFlag::SW_SOCK_CLOEXEC | SocketFlag::SW_SOCK_NONBLOCK).bits(),
    )?
    .into_socket();
    let fd = socket.fd();
    set_option(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, 1);
    if cfg!(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    )) {
        set_option(fd, libc::SOL_SOCKET, libc::SO_REUSEPORT, 1);
    }
    if !SocketType::from_raw(kind).is_some_and(SocketType::is_udp) {
        // `OPENSSL_assert(0)`, which reports and aborts.
        swoole_rs_corebase::fatal("OpenSSL internal error: assertion failed: 0");
    }
    let origin = c"Server::accept_dtls_connection";
    let mut conn = None;
    'setup: {
        if !bind(socket, kind, &host, number) {
            log::sys_warning(origin, format_args!("bind() failed"));
            break 'setup;
        }
        if kind == SocketType::SW_SOCK_UDP6.raw() {
            set_option(fd, libc::IPPROTO_IPV6, libc::IPV6_V6ONLY, 0);
        }
        if !sa.connect(fd) {
            let (host, port) = swoole_rs_tls::upstream::peer(sa);
            log::sys_warning(origin, format_args!("connect({host}:{port}) failed"));
            break 'setup;
        }
        socket.edit(|socket| {
            socket.info = *sa;
            socket.chunk_size = SW_SSL_BUFFER_SIZE;
        });
        conn = crate::connection::add(serv, listen, socket, server_fd);
        if conn.is_none() {
            break 'setup;
        }
        // `ListenPort::create_dtls_session(sock)`, over the port's context.
        let context = listen.config().tls.settings.get();
        if context.is_some_and(|context| dtls_open(socket, &context)) {
            return Some(socket);
        }
    }
    if let Some(conn) = conn {
        serv.state()
            .tables_ref()
            .reset_connection(conn.fd(), crate::records::Zeroed::Whole);
    }
    // SAFETY: the heap record made above, which nothing uses afterwards.
    let socket = unsafe { swoole_rs_reactor::OwnedSocket::from_socket(socket) };
    swoole_rs_tls::upstream::socket_free(Some(socket));
    None
}

/// `session->append(...)`, which throws `std::bad_alloc` when the queue can't grow.
fn appended(queued: bool) {
    if !queued {
        std::alloc::handle_alloc_error(core::alloc::Layout::new::<DgramPacket>());
    }
}

/// `ReactorThread_onPacketReceived` on a DTLS port: the datagram `data` from `peer`,
/// which arrived on `port` (a DTLS port of `serv`, on `reactor`), opens a session and
/// starts its handshake; once listened, the new connection is incoming on its reactor
/// thread. False when no session could be made, as on a port without DTLS.
pub(crate) fn on_packet(
    serv: crate::view::Server<'_>,
    single_thread: bool,
    reactor: crate::reactor_handle::Reactor,
    port: crate::view::Port<'_>,
    peer: &SocketAddress,
    data: &[u8],
) -> bool {
    let Some(record) = accept(serv, port, peer) else {
        return false;
    };
    if let Some(session) = DtlsSession::of(record) {
        appended(swr_v1_tls_dtls_append(session, data));
    }
    if !dtls_session_listen(record) {
        return crate::conn_io::close_connection(serv, reactor, Some(record));
    }
    // A session socket's connection is its fd's slot.
    let fd = libc::c_long::from(record.fd());
    let Some(conn) = serv.state().connection(fd, crate::records::Lookup::ByFd) else {
        return false;
    };
    let failed = if single_thread {
        !crate::server_methods::connection_incoming(serv, reactor, conn)
    } else {
        let (session_id, reactor_id) = (conn.session_id(), conn.reactor_id());
        let ev = DataHead {
            kind: ServerEvent::Incoming.code(),
            fd: session_id,
            reactor_id: i16::from(reactor_id),
            ..DataHead::default()
        };
        crate::event_worker::native::send_to_reactor_thread(serv, &ev).is_none()
    };
    if failed {
        // The reactor's own close, which is what the caller passes as `_close`.
        reactor.close(record);
    }
    true
}

/// `ReactorThread_onRead` on a DTLS port: the next datagram on `sock` goes to its
/// session; whether a pending handshake failed (as it does without a session).
pub(crate) fn on_read(sock: Socket<'_>) -> bool {
    let size = usize::try_from(SW_BUFFER_SIZE_UDP).unwrap_or(0);
    let Some(session) = DtlsSession::of(sock) else {
        return true;
    };
    // A `dtls::Buffer` of the socket's next datagram, which the session queues.
    appended(dtls_session_receive(session, sock.fd(), size));
    !session.listened() && !dtls_session_listen(sock)
}
