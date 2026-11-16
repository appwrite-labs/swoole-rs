//! The reactor-thread entry points that were master.cc's: the reactor handlers
//! (`ReactorThread_on*`) and the `Server` and `ReactorThread` methods that build a
//! context and run one operation over it. With `upstream-symbols`, `dispatch_task`
//! is the `onPackage` crates/ports' protocols hold. Every op they make is Rust's.
use core::ffi::{c_int, c_long};

use swoole_rs_corebase::constants::SocketType;
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_reactor::{Event, Running};

use crate::base_factory::SendData;
use crate::reactor_handle::Reactor;
use crate::view;

/// `swoole::RecvData`: the head and payload a receive or dispatch callback reads,
/// which is a [`SendData`]'s.
pub type RecvData<'a> = SendData<'a>;

impl SendData<'_> {
    /// An `onPacket` payload: the `DgramPacket` head's socket type and sender,
    /// and the datagram that follows it; `None` if the payload is too short.
    #[must_use]
    pub fn datagram(&self) -> Option<(c_int, SocketAddress, &[u8])> {
        let head = self.datagram_head()?;
        let start = size_of::<DgramPacket>();
        let data = self.payload().get(start..start + head.length as usize)?;
        Some((head.socket_type, head.socket_addr, data))
    }

    /// The sender an `onPacket` payload's `DgramPacket` head names; `None` if the
    /// payload ends before the head does.
    pub(crate) fn datagram_sender(&self) -> Option<SocketAddress> {
        Some(self.datagram_head()?.socket_addr)
    }

    /// An `onPacket` payload's `DgramPacket` head; `None` if the payload is shorter.
    fn datagram_head(&self) -> Option<DgramPacket> {
        let head = self.payload().get(..size_of::<DgramPacket>())?;
        // SAFETY: `size_of::<DgramPacket>()` bytes, read unaligned; every field is
        // plain data.
        Some(unsafe { head.as_ptr().cast::<DgramPacket>().read_unaligned() })
    }
}

/// `DgramPacket`'s head; the datagram follows.
#[repr(C)]
pub(crate) struct DgramPacket {
    pub(crate) socket_type: c_int,
    pub(crate) socket_addr: SocketAddress,
    pub(crate) length: u32,
}

impl DgramPacket {
    /// The session a datagram is dispatched under: an IPv4 sender's address, else
    /// the CRC32 of its host text.
    pub(crate) fn sender(&self) -> c_long {
        if self.socket_type == SocketType::SW_SOCK_UDP.raw() {
            let ip = self.socket_addr.addr.v4().sin_addr.s_addr;
            return c_long::from(ip.cast_signed());
        }
        let host = swoole_rs_clients::address::swr_v1_clients_address_host(&self.socket_addr);
        let length = self.socket_addr.len as usize;
        if host.is_null() || length == 0 {
            return 0;
        }
        // SAFETY: upstream hashes as many bytes of the text as the address is long.
        let text = unsafe { std::slice::from_raw_parts(host.cast::<u8>(), length) };
        c_long::from(swoole_rs_crc32::crc32(text))
    }
}

/// `reactor->ptr`, a server reactor's server, unchecked: the per-event handlers' read,
/// where [`Reactor::server`]'s check would cost every request.
///
/// # Safety
/// `reactor` holds a live server ([`Reactor::hold_server`]) for `'a`.
pub(crate) unsafe fn server<'a>(reactor: Reactor) -> view::Server<'a> {
    // SAFETY: per the caller.
    unsafe { view::Server::from_raw(reactor.handle().object()) }
}

/// `f` over a handler's server (`reactor->ptr`) and reactor.
#[inline]
fn handler(reactor: Running<'_>, f: impl FnOnce(view::Server<'_>, Reactor) -> c_int) -> c_int {
    let reactor = Reactor::running(reactor);
    // SAFETY: the handlers below are registered only on reactors that hold a server.
    f(unsafe { server(reactor) }, reactor)
}

// The handlers below are registered only on reactors that hold a server.

/// `ReactorThread_onPacketReceived`: a UDP port's datagram.
pub extern "C" fn on_packet_received(reactor: Running<'_>, event: &mut Event) -> c_int {
    handler(reactor, |serv, reactor| {
        crate::conn_io::status(crate::reactor_thread::packet_received(serv, reactor, event))
    })
}

/// `ReactorThread_onClose`: the peer closed.
pub extern "C" fn on_close(reactor: Running<'_>, event: &mut Event) -> c_int {
    handler(reactor, |serv, reactor| {
        crate::conn_io::status(crate::conn_io::peer_close(serv, reactor, event))
    })
}

/// `ReactorThread_onPipeWrite`: a worker pipe can be written.
pub extern "C" fn on_pipe_write(reactor: Running<'_>, event: &mut Event) -> c_int {
    handler(reactor, |serv, reactor| {
        crate::conn_io::status(crate::reactor_io::pipe_write(serv, reactor, event))
    })
}

/// `ReactorThread_onRead`: a connection is readable.
pub extern "C" fn on_read(reactor: Running<'_>, event: &mut Event) -> c_int {
    handler(reactor, |serv, reactor| {
        crate::conn_io::read(serv, reactor, event)
    })
}

/// `ReactorThread_onWrite`: a connection is writable.
pub extern "C" fn on_write(reactor: Running<'_>, event: &mut Event) -> c_int {
    handler(reactor, |serv, reactor| {
        crate::conn_io::status(crate::conn_io::write(serv, reactor, event))
    })
}

/// `static int Server::close_connection(Reactor *, Socket *)`.
///
/// # Safety
/// `reactor` holds a live server and `socket` is a session socket of it, which nothing
/// uses afterwards.
#[must_use]
pub unsafe extern "C-unwind" fn close_connection(
    reactor: Running<'_>,
    socket: swoole_rs_reactor::Socket<'_>,
) -> c_int {
    let reactor = Reactor::running(reactor);
    // SAFETY: per the caller.
    let serv = unsafe { server(reactor) };
    crate::conn_io::status(crate::conn_io::close_connection(
        serv,
        reactor,
        Some(socket),
    ))
}

/// `void ReactorThread::shutdown(Reactor *)`, over `serv`'s reactor thread `id` and
/// its reactor.
#[cfg(feature = "upstream-symbols")]
pub(crate) fn thread_shutdown(serv: view::Server<'_>, reactor: Reactor, id: usize) {
    let mut ctx = crate::reactor_ops::ReactorContext::new(serv, Some(reactor));
    ctx.thread = serv.state().tables_ref().reactor_thread(id);
    crate::thread_ops::shutdown(&ctx);
}

/// `static int Server::dispatch_task(const Protocol *, Socket *, const RecvData *)`:
/// a data frame to the workers. A port protocol's `onPackage`.
///
/// # Safety
/// `proto` is a live server port's protocol (its `private_data_2` the server) and
/// `socket` a live session socket of that port.
#[cfg(feature = "upstream-symbols")]
#[must_use]
pub unsafe extern "C-unwind" fn dispatch_task(
    proto: *const swoole_rs_clients::protocol::Protocol,
    socket: swoole_rs_reactor::Socket<'_>,
    rdata: &swoole_rs_corebase::net_types::RecvData,
) -> c_int {
    use core::sync::atomic::Ordering::SeqCst;
    // SAFETY: per the caller; a port's protocol keeps its server.
    let serv = unsafe { (*proto).private_data_2 };
    // SAFETY: per the caller, a live server's.
    let server = unsafe { view::Server::from_raw(serv) };
    let state = server.state();
    // A session socket's object is its live connection.
    let conn = socket
        .read(|s| s.object)
        .cast::<crate::records::ConnectionRecord>();
    let len = rdata.info.len;
    // The slot is read only before the dispatch, as upstream does: base and thread mode
    // run the callback inline, and a close there hands the slot to the accept that reuses
    // the fd (`close_connection`'s Release of `closed`), which no later read is ordered before.
    // Only the atomic count of queued bytes, which is shared by reference, is kept past it.
    let (port, reactor_id, mut task, queued) = {
        // SAFETY: the live connection slot, which lives as long as the server; its
        // plain fields are borrowed only for this block.
        let c = unsafe { &*conn };
        if len > 0 {
            c.recv_queued_bytes.fetch_add(len, SeqCst);
        }
        let info = swoole_rs_corebase::DataHead {
            server_fd: crate::base_factory::low16(c.server_fd.get()),
            reactor_id: c.reactor_id.get().into(),
            ext_flags: rdata.info.ext_flags,
            kind: swoole_rs_corebase::ServerEvent::RecvData.code(),
            time: c.last_recv_time.get(),
            fd: c.fd.get().into(),
            len,
            ..swoole_rs_corebase::DataHead::default()
        };
        // SAFETY: the caller's `RecvData`, whose `len` bytes at `data` (or null) stay
        // readable and unwritten until the dispatch returns.
        let task = unsafe { SendData::from_raw(info, rdata.data.cast()) };
        (
            server.port_of_fd(c.fd.get()),
            c.reactor_id.get(),
            task,
            &c.recv_queued_bytes,
        )
    };
    let mode = state.config_ref().mode_;
    let dispatched = if mode == crate::Mode::Process {
        crate::factory::dispatch(server, &mut task)
    } else {
        if mode == crate::Mode::Base {
            // The reader's word that the payload is its whole receive buffer, which
            // only a worker in the reader's own thread may act on.
            task.set_flags(rdata.info.flags & crate::base_factory::POP_PTR);
        }
        crate::base_factory::dispatch(server, &mut task, mode == crate::Mode::Base)
    };
    if !dispatched {
        if len > 0 {
            queued.fetch_sub(len, SeqCst);
        }
        return -1;
    }
    if mode == crate::Mode::Process {
        // `serv->get_thread(id)`, unchecked.
        let threads = state.tables_ref().reactor_threads_ptr();
        let thread = threads
            .cast::<crate::reactor_ops::ThreadRecord>()
            .wrapping_add(usize::from(reactor_id));
        // SAFETY: the connection's live reactor thread.
        let count = unsafe { &(*thread).dispatch_count };
        count.set(count.get() + 1);
    }
    if let Some(gs) = crate::shared::server_counters(state) {
        gs.dispatch_count.fetch_add(1, SeqCst);
    }
    if let Some(gs) = port.and_then(|port| port.counters()) {
        gs.dispatch_count.fetch_add(1, SeqCst);
    }
    0
}
