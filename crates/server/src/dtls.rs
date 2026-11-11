//! A DTLS port's sessions on the reactor thread: `Server::accept_dtls_connection`
//! and the datagram, read and close steps over them. crates/tls owns each
//! session's queue and handshake; crates/ports stores them by fd.
use core::ffi::{CStr, c_char, c_int, c_long, c_void};
use core::ptr;
use std::ffi::CString;

use swoole_rs_clients::address::swr_v1_clients_address_assign;
use swoole_rs_corebase::constants::{SW_BUFFER_SIZE_UDP, SW_SSL_BUFFER_SIZE};
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_reactor::Socket;
use swoole_rs_reactor::socket_upstream::make_socket_typed;
use swoole_rs_tls::{
    DtlsSession, dtls_fields, swr_v1_tls_dtls_append, swr_v1_tls_dtls_append_buffer,
    swr_v1_tls_dtls_session_listen,
};

use crate::ops::EVENT_INCOMING;
use crate::port_config;
use crate::records::ConnectionRecord;

// SAFETY: libcrypto's.
unsafe extern "C" {
    fn OPENSSL_die(assertion: *const c_char, file: *const c_char, line: c_int) -> !;
}

/// `SW_FD_SESSION`, `SW_SOCK_NONBLOCK`, `SW_SOCK_CLOEXEC`, `SW_SOCK_UDP` and `SW_SOCK_UDP6`.
const FD_SESSION: c_int = 0;
const SOCK_NONBLOCK: c_int = 1 << 2;
const SOCK_CLOEXEC: c_int = 1 << 3;
const SOCK_UDP: c_int = 2;
const SOCK_UDP6: c_int = 4;

/// `DgramPacket`: the datagram the reactor thread received, and its peer.
#[repr(C)]
pub(crate) struct DgramPacket {
    socket_type: c_int,
    socket_addr: SocketAddress,
    length: u32,
    data: [u8; 0],
}

/// `setsockopt(fd, level, option, value)` for an int option.
fn set_option(fd: c_int, level: c_int, option: c_int, value: c_int) {
    let size = libc::socklen_t::try_from(size_of::<c_int>()).unwrap_or_default();
    // SAFETY: `value` is a live c_int of `size` bytes.
    unsafe { libc::setsockopt(fd, level, option, (&raw const value).cast(), size) };
}

/// `Socket::bind(host, port)`: the port's literal address, unresolved.
fn bind(sock: Socket<'_>, kind: c_int, host: &CStr, port: c_int) -> bool {
    let mut address = SocketAddress {
        // SAFETY: the sockaddr union is plain data.
        addr: unsafe { core::mem::zeroed() },
        len: 0,
        kind,
    };
    // SAFETY: the union and its length, and the host's bytes.
    let error = unsafe {
        swr_v1_clients_address_assign(
            (&raw mut address.addr).cast(),
            &raw mut address.len,
            kind,
            host.as_ptr(),
            host.count_bytes(),
            port,
            None,
        )
    };
    if error != 0 {
        log::set_last_error(error);
        return false;
    }
    sock.edit(|sock| sock.bind(&address)) >= 0
}

/// `Server::accept_dtls_connection(port, sa)`: a socket bound to the port and
/// connected to the peer, registered as a connection with a DTLS session; null,
/// with everything it made undone, when a step fails.
///
/// # Safety
/// `serv` is a live server, `port` one of its live DTLS ports.
unsafe fn accept(serv: *mut c_void, port: *mut c_void, sa: &SocketAddress) -> *mut DtlsSession {
    // SAFETY: as the caller guarantees.
    let Some(listen) = (unsafe { crate::view::Port::from_raw(port) }) else {
        return ptr::null_mut();
    };
    let (kind, number, host) = {
        let config = listen.config();
        // NUL-terminated: the address parser reads the host as a C string.
        let host = CString::new(config.host_bytes()).unwrap_or_default();
        (config.r#type, config.port, host)
    };
    let server_fd = listen.fd();
    let sock = make_socket_typed(kind, FD_SESSION, SOCK_CLOEXEC | SOCK_NONBLOCK);
    // SAFETY: null, or the socket just made, which only this thread uses.
    let Some(socket) = (unsafe { Socket::from_ptr(sock) }) else {
        return ptr::null_mut();
    };
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
    if !matches!(kind, SOCK_UDP | SOCK_UDP6) {
        // SAFETY: OPENSSL_assert(0)'s report, which does not return.
        unsafe { OPENSSL_die(c"assertion failed: 0".as_ptr(), c"master.cc".as_ptr(), 0) };
    }
    let origin = c"Server::accept_dtls_connection";
    let mut conn: *mut ConnectionRecord = ptr::null_mut();
    'setup: {
        if !bind(socket, kind, &host, number) {
            log::sys_warning(origin, format_args!("bind() failed"));
            break 'setup;
        }
        if kind == SOCK_UDP6 {
            set_option(fd, libc::IPPROTO_IPV6, libc::IPV6_V6ONLY, 0);
        }
        let peer = (&raw const sa.addr).cast::<libc::sockaddr>();
        // SAFETY: the peer's address of its length.
        if unsafe { libc::connect(fd, peer, sa.len) } < 0 {
            let (host, port) = swoole_rs_tls::upstream::peer(sa);
            log::sys_warning(origin, format_args!("connect({host}:{port}) failed"));
            break 'setup;
        }
        socket.edit(|socket| {
            socket.info = *sa;
            socket.chunk_size = SW_SSL_BUFFER_SIZE;
        });
        // SAFETY: the live server and port, and the socket just made.
        conn = unsafe { crate::server_methods::add_connection(serv, port, sock.cast(), server_fd) }
            .cast();
        if conn.is_null() {
            break 'setup;
        }
        let session = (port_config::methods().dtls_create)(port, sock);
        if !session.is_null() {
            return session;
        }
    }
    if !conn.is_null() {
        // SAFETY: the connection added above, which nothing else uses yet.
        unsafe { ptr::write_bytes(conn, 0, 1) };
    }
    // SAFETY: the socket just made, which nothing uses afterwards.
    unsafe { swoole_rs_tls::upstream::socket_free(sock) };
    ptr::null_mut()
}

/// `session->append(...)`, which throws `std::bad_alloc` when the queue can't grow.
fn appended(status: c_int) {
    if status != 0 {
        std::alloc::handle_alloc_error(core::alloc::Layout::new::<DgramPacket>());
    }
}

/// `ReactorThread_onPacketReceived` on a DTLS port: the datagram (`length` bytes)
/// opens a session and starts its handshake; once listened, the new connection is
/// incoming on its reactor thread. `SW_ERR` when no session could be made.
///
/// # Safety
/// `serv` and `reactor` are live, `port` is the DTLS port the packet arrived on and
/// `pkt` the received packet.
pub(crate) unsafe fn on_packet(
    serv: *mut c_void,
    single_thread: bool,
    reactor: *mut c_void,
    close: Option<swoole_rs_reactor::ReactorClose>,
    port: *mut c_void,
    pkt: *mut DgramPacket,
    length: u64,
) -> i64 {
    // SAFETY: the received packet.
    let pkt = unsafe { &mut *pkt };
    // SAFETY: as the caller guarantees.
    let session = unsafe { accept(serv, port, &pkt.socket_addr) };
    if session.is_null() {
        return -1;
    }
    let data = (&raw const pkt.data).cast::<u8>();
    // SAFETY: the live session, and the packet's `length` received bytes.
    appended(unsafe {
        swr_v1_tls_dtls_append(session, data, usize::try_from(length).unwrap_or(0))
    });
    // SAFETY: the live session.
    let fields = dtls_fields(unsafe { &*session });
    // SAFETY: as above.
    let socket = unsafe { (*fields).socket };
    // SAFETY: the live session over its live socket.
    if !unsafe { swr_v1_tls_dtls_session_listen(session) } {
        // SAFETY: the reactor and the session's socket.
        return i64::from(unsafe { crate::reactor_entry::close_connection(reactor, socket) });
    }
    // SAFETY: the live session's socket.
    let Some(record) = (unsafe { Socket::from_ptr(socket) }) else {
        return -1;
    };
    // A session socket's object is its live connection.
    let conn = record.read(|s| s.object.cast::<ConnectionRecord>());
    let failed = if single_thread {
        // SAFETY: the live server, reactor and connection.
        let incoming = unsafe { crate::server_methods::connection_incoming(serv, reactor, conn) };
        incoming < 0
    } else {
        // SAFETY: as above.
        let conn = unsafe { &*conn };
        let (session_id, reactor_id) = (conn.session_id, conn.reactor_id);
        let ev = DataHead {
            kind: u8::try_from(EVENT_INCOMING).unwrap_or_default(),
            fd: session_id,
            reactor_id: i16::from(reactor_id),
            ..DataHead::default()
        };
        let ev = (&raw const ev).cast::<c_void>();
        // SAFETY: the live server and the event, which outlives the call.
        let sent = unsafe {
            swoole_rs_process::workers::entry::send_to_reactor_thread(
                serv,
                ev,
                size_of::<DataHead>(),
                session_id as c_long,
            )
        };
        sent < 0
    };
    if failed && let Some(close) = close {
        // SAFETY: the live reactor and the session's socket.
        unsafe { close(reactor, socket.cast()) };
    }
    0
}

/// `ReactorThread_onRead` on a DTLS port: the next datagram goes to the socket's
/// session; whether a pending handshake failed.
///
/// # Safety
/// `port` is the live DTLS port, `sock` its session socket on `fd`.
pub(crate) unsafe fn on_read(port: *mut c_void, sock: Socket<'_>, fd: c_int) -> bool {
    let size = usize::try_from(SW_BUFFER_SIZE_UDP).unwrap_or(0);
    // SAFETY: a `dtls::Buffer` header and its payload; the session frees it.
    let buffer = unsafe { libc::malloc(2 + size) }.cast::<u16>();
    // SAFETY: the payload follows the header.
    let data = unsafe { buffer.add(1) }.cast::<c_void>();
    // SAFETY: the live socket and the payload's `size` bytes.
    let read = unsafe { libc::read(sock.fd(), data, size) };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "C++'s uint16_t length"
    )]
    // SAFETY: the header just allocated.
    unsafe {
        *buffer = read as u16;
    }
    // SAFETY: the port's live record.
    let session = (port_config::methods().dtls_session)(unsafe { port_config::of(port) }, fd);
    // SAFETY: the socket's live session, which takes the buffer.
    appended(unsafe { swr_v1_tls_dtls_append_buffer(session, buffer.cast()) });
    // SAFETY: as above.
    let fields = dtls_fields(unsafe { &*session });
    // SAFETY: as above.
    let listened = unsafe { (*fields).listened };
    // SAFETY: as above.
    !listened && !unsafe { swr_v1_tls_dtls_session_listen(session) }
}

/// `Server::close_connection`'s `delete` of the socket's DTLS session.
///
/// # Safety
/// `port` is the live DTLS port; nothing uses the session afterwards.
pub(crate) unsafe fn free(port: *mut c_void, fd: c_int) {
    // SAFETY: the port's live record, as the caller guarantees.
    (port_config::methods().dtls_free)(unsafe { port_config::of(port) }, fd);
}
