//! The reactor-thread entry points that were master.cc's: the reactor handlers
//! (`ReactorThread_on*`) and the `Server` and `ReactorThread` methods that build a
//! context and run one operation over it. `dispatch_task`
//! is the `onPackage` crates/ports' protocols hold. Every op they make is Rust's.
use core::ffi::{c_int, c_long};

use swoole_rs_corebase::constants::{SocketType, SocketTypeCode};
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_corebase::reactor::{Event, Running};

use crate::base_factory::SendData;
use crate::reactor_handle::Reactor;
use crate::view;

/// `swoole::RecvData`: the head and payload a receive callback reads, which may take
/// a packet the message owns.
pub use swoole_rs_corebase::process::process_pool::RecvData;

/// An `onPacket` payload: the `DgramPacket` head's socket type and sender, and the
/// datagram that follows it; `None` if the payload is too short.
#[must_use]
pub fn datagram(payload: &[u8]) -> Option<(SocketTypeCode, SocketAddress, &[u8])> {
    let head = DgramPacket::from_bytes(payload.first_chunk()?);
    let start = size_of::<DgramPacket>();
    let data = payload.get(start..start + head.length as usize)?;
    Some((head.socket_type, head.socket_addr, data))
}

impl SendData<'_> {
    /// The sender an `onPacket` payload's `DgramPacket` head names; `None` if the
    /// payload ends before the head does.
    pub(crate) fn datagram_sender(&self) -> Option<SocketAddress> {
        Some(DgramPacket::from_bytes(self.payload().first_chunk()?).socket_addr)
    }
}

/// `DgramPacket`'s head; the datagram follows.
#[repr(C)]
#[derive(Default)]
pub struct DgramPacket {
    pub(crate) socket_type: SocketTypeCode,
    pub socket_addr: SocketAddress,
    pub(crate) length: u32,
}

/// A [`DgramPacket`] head's bytes.
pub(crate) const DGRAM_HEAD: usize = size_of::<DgramPacket>();

/// Where each field of a [`DgramPacket`] head starts, as C lays it out.
const TYPE_AT: usize = core::mem::offset_of!(DgramPacket, socket_type);
const ADDR_AT: usize = core::mem::offset_of!(DgramPacket, socket_addr);
const STORAGE_AT: usize = ADDR_AT + core::mem::offset_of!(SocketAddress, addr);
const ADDR_LEN_AT: usize = ADDR_AT + core::mem::offset_of!(SocketAddress, len);
const KIND_AT: usize = ADDR_AT + core::mem::offset_of!(SocketAddress, kind);
const LENGTH_AT: usize = core::mem::offset_of!(DgramPacket, length);

/// The `N` bytes of `head` at `at`, which the offsets above keep within it.
fn field<const N: usize>(head: &[u8; DGRAM_HEAD], at: usize) -> [u8; N] {
    head.get(at..)
        .and_then(<[u8]>::first_chunk)
        .copied()
        .unwrap_or([0; N])
}

impl DgramPacket {
    /// The head as C lays it out, its padding zero.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; DGRAM_HEAD] {
        let mut head = [0; DGRAM_HEAD];
        let address = &self.socket_addr;
        for (at, bytes) in [
            (TYPE_AT, &self.socket_type.0.to_ne_bytes()[..]),
            (STORAGE_AT, &address.addr.as_bytes()[..]),
            (ADDR_LEN_AT, &address.len.to_ne_bytes()[..]),
            (KIND_AT, &address.kind.to_ne_bytes()[..]),
            (LENGTH_AT, &self.length.to_ne_bytes()[..]),
        ] {
            if let Some(to) = head.get_mut(at..at + bytes.len()) {
                to.copy_from_slice(bytes);
            }
        }
        head
    }

    /// The head [`Self::to_bytes`] laid out.
    pub(crate) fn from_bytes(head: &[u8; DGRAM_HEAD]) -> Self {
        Self {
            socket_type: SocketTypeCode(c_int::from_ne_bytes(field(head, TYPE_AT))),
            socket_addr: SocketAddress {
                addr: swoole_rs_corebase::net_types::AddressStorage {
                    bytes: field(head, STORAGE_AT),
                },
                len: libc::socklen_t::from_ne_bytes(field(head, ADDR_LEN_AT)),
                kind: c_int::from_ne_bytes(field(head, KIND_AT)),
            },
            length: u32::from_ne_bytes(field(head, LENGTH_AT)),
        }
    }

    /// The session a datagram is dispatched under: an IPv4 sender's address, else
    /// the CRC32 of its host text.
    pub(crate) fn sender(&self) -> c_long {
        if self.socket_type.known() == Some(SocketType::SW_SOCK_UDP) {
            let ip = self.socket_addr.addr.v4().sin_addr.s_addr;
            return c_long::from(ip.cast_signed());
        }
        // Upstream hashes as many bytes of the text as the address is long.
        let length = self.socket_addr.len as usize;
        let text = swoole_rs_clients::address::host_bytes(&self.socket_addr, length);
        match text.as_ref().and_then(|text| text.get(..length)) {
            Some(text) if length > 0 => c_long::from(swoole_rs_crc32::crc32(text)),
            _ => 0,
        }
    }
}

/// `f` over a handler's server (`reactor->ptr`) and reactor, as upstream's `SW_OK`/`SW_ERR`;
/// `SW_ERR` on a reactor that holds none, where no handler below is registered.
#[inline]
fn handler(reactor: Running<'_>, f: impl FnOnce(view::Server<'_>, Reactor) -> bool) -> c_int {
    let reactor = Reactor::running(reactor);
    crate::conn_io::status(reactor.server().is_some_and(|serv| f(serv, reactor)))
}

// The handlers below are registered only on reactors that hold a server.

/// `ReactorThread_onPacketReceived`: a UDP port's datagram.
pub fn on_packet_received(reactor: Running<'_>, event: &mut Event) -> c_int {
    handler(reactor, |serv, reactor| {
        crate::reactor_thread::packet_received(serv, reactor, event)
    })
}

/// `ReactorThread_onClose`: the peer closed.
pub fn on_close(reactor: Running<'_>, event: &mut Event) -> c_int {
    handler(reactor, |serv, reactor| {
        crate::conn_io::peer_close(serv, reactor, event)
    })
}

/// `ReactorThread_onPipeWrite`: a worker pipe can be written.
pub fn on_pipe_write(reactor: Running<'_>, event: &mut Event) -> c_int {
    handler(reactor, |serv, reactor| {
        crate::reactor_io::pipe_write(serv, reactor, event)
    })
}

/// `ReactorThread_onRead`: a connection is readable.
pub fn on_read(reactor: Running<'_>, event: &mut Event) -> c_int {
    handler(reactor, |serv, reactor| {
        crate::conn_io::read(serv, reactor, event)
    })
}

/// `ReactorThread_onWrite`: a connection is writable.
pub fn on_write(reactor: Running<'_>, event: &mut Event) -> c_int {
    handler(reactor, |serv, reactor| {
        crate::conn_io::write(serv, reactor, event)
    })
}

/// `static int Server::close_connection(Reactor *, Socket *)`: the reactor's close of
/// one of its server's session sockets.
#[must_use]
pub extern "C-unwind" fn close_connection(
    reactor: Running<'_>,
    socket: swoole_rs_corebase::reactor::OwnedSocket,
) -> c_int {
    // Back to the connection, whose close frees it.
    let socket = socket.leak();
    handler(reactor, |serv, reactor| {
        crate::conn_io::close_connection(serv, reactor, Some(socket))
    })
}

/// `void ReactorThread::shutdown(Reactor *)`, over `serv`'s reactor thread `id` and
/// its reactor.
pub(crate) fn thread_shutdown(serv: view::Server<'_>, reactor: Reactor, id: usize) {
    let mut ctx = crate::reactor_ops::ReactorContext::new(serv, Some(reactor));
    ctx.thread = serv.state().tables_ref().reactor_thread(id);
    crate::thread_ops::shutdown(&ctx);
}

/// `static int Server::dispatch_task(const Protocol *, Socket *, const RecvData *)` over
/// the port's server and the session's connection: `data` to the workers, with the
/// caller's `len`, `ext_flags` and (in base mode) `flags`. Whether it was dispatched.
#[must_use]
pub fn dispatch_data(
    server: view::Server<'_>,
    conn: crate::Connection<'_>,
    data: SendData<'_>,
    port: Option<&crate::port_config::PortConfig>,
) -> bool {
    dispatch_input(server, conn, data, None, port)
}

/// Dispatch a complete receive buffer that a base worker can take without copying.
#[must_use]
pub fn dispatch_buffer(
    server: view::Server<'_>,
    conn: crate::Connection<'_>,
    mut head: swoole_rs_corebase::DataHead,
    buffer: &mut swoole_rs_corebase::buffer::string::StringBuffer,
) -> bool {
    if server.config().mode_ != crate::Mode::Base {
        return dispatch_data(server, conn, SendData::new(head, buffer.view()), None);
    }
    head.len = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
    dispatch_input(server, conn, SendData::empty(head), Some(buffer), None)
}

fn dispatch_input(
    server: view::Server<'_>,
    conn: crate::Connection<'_>,
    data: SendData<'_>,
    buffer: Option<&mut swoole_rs_corebase::buffer::string::StringBuffer>,
    port: Option<&crate::port_config::PortConfig>,
) -> bool {
    use core::sync::atomic::Ordering::SeqCst;
    let state = server.state();
    let head = *data.info();
    let len = head.len;
    // The slot is read only before the dispatch, as upstream does: base and thread mode
    // run the callback inline, and a close there hands the slot to the accept that reuses
    // the fd (`close_connection`'s Release of `closed`), which no later read is ordered before.
    // Only the atomic count of queued bytes, which is shared by reference, is kept past it.
    let queued = conn.recv_queued_bytes();
    if len > 0 {
        queued.fetch_add(len, SeqCst);
    }
    let (fd, reactor_id) = (conn.fd(), conn.reactor_id());
    let mut task = data.rehead(swoole_rs_corebase::DataHead {
        server_fd: crate::base_factory::low16(conn.server_fd()),
        reactor_id: reactor_id.into(),
        ext_flags: head.ext_flags,
        kind: swoole_rs_corebase::ServerEvent::RecvData.code(),
        time: conn.last_recv_time(),
        fd: fd.into(),
        ..swoole_rs_corebase::DataHead::default()
    });
    let port = port.or_else(|| server.port_of_server_fd(conn.server_fd()));
    let mode = state.config_ref().mode_;
    let dispatched = if mode == crate::Mode::Process {
        crate::factory::dispatch(server, &mut task)
    } else {
        if mode == crate::Mode::Base {
            // The reader's word that the payload is its whole receive buffer, which
            // only a worker in the reader's own thread may act on.
            task.set_flags(head.flags & crate::base_factory::POP_PTR);
        }
        crate::base_factory::dispatch_buffer(server, &mut task, mode == crate::Mode::Base, buffer)
    };
    if !dispatched {
        if len > 0 {
            queued.fetch_sub(len, SeqCst);
        }
        return false;
    }
    if mode == crate::Mode::Process
        && let Some(thread) = state.tables_ref().reactor_thread(usize::from(reactor_id))
    {
        thread.dispatch_count.set(thread.dispatch_count.get() + 1);
    }
    if let Some(gs) = crate::shared::server_counters(state) {
        gs.dispatch_count.fetch_add(1, SeqCst);
    }
    if let Some(gs) = port.and_then(|port| port.counters()) {
        gs.dispatch_count.fetch_add(1, SeqCst);
    }
    true
}
