//! The reactor-thread entry points that were master.cc's: the reactor handlers
//! (`ReactorThread_on*`) and the `Server` and `ReactorThread` methods that build a
//! context and run one operation over it. With `upstream-symbols`, `dispatch_task`
//! keeps its upstream (Itanium) symbol, which crates/ports' protocols link. Every op
//! they make is Rust's.
use core::ffi::{c_int, c_long, c_void};

use swoole_rs_corebase::DataHead;
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_reactor::{Event, Running, SocketRecord};

use crate::reactor_handle::Reactor;
use crate::reactor_ops::ThreadRecord;
use crate::view;

/// `swoole::RecvData`.
#[repr(C)]
pub struct RecvData {
    pub(crate) info: DataHead,
    pub(crate) data: *const c_void,
}

impl RecvData {
    /// The event's head.
    #[must_use]
    pub fn info(&self) -> &DataHead {
        &self.info
    }

    /// Where the event's payload starts: `info.len` bytes, or what the head's
    /// flags say it is.
    #[must_use]
    pub fn data(&self) -> *const c_void {
        self.data
    }

    /// The payload's `info.len` bytes; none for a null `data`.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        if self.data.is_null() {
            return &[];
        }
        // SAFETY: whoever lends a `RecvData` points `data` at `info.len` bytes live
        // for the borrow, and its fields are private to this crate.
        unsafe { core::slice::from_raw_parts(self.data.cast(), self.info.len as usize) }
    }

    /// An `onPacket` payload: the `DgramPacket` head's socket type and sender,
    /// and the datagram that follows it; `None` if the payload is too short.
    #[must_use]
    pub fn datagram(&self) -> Option<(c_int, SocketAddress, &[u8])> {
        let payload = self.payload();
        let head = payload.get(..size_of::<DgramPacket>())?;
        // SAFETY: `size_of::<DgramPacket>()` bytes, read unaligned; every field is
        // plain data.
        let head = unsafe { head.as_ptr().cast::<DgramPacket>().read_unaligned() };
        let start = size_of::<DgramPacket>();
        let data = payload.get(start..start + head.length as usize)?;
        Some((head.socket_type, head.socket_addr, data))
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
        /// `SW_SOCK_UDP`.
        const SOCK_UDP: c_int = 2;
        if self.socket_type == SOCK_UDP {
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

/// `serv->get_thread(id)`.
pub(crate) fn thread(state: &crate::ServerState, id: usize) -> *mut c_void {
    let threads = state.tables_ref().reactor_threads;
    threads.wrapping_byte_add(id * size_of::<ThreadRecord>())
}

/// `reactor->ptr`, a server reactor's server.
///
/// # Safety
/// `reactor` is a server's reactor, whose `ptr` is the live `Server` for `'a`.
pub(crate) unsafe fn server<'a>(reactor: Reactor) -> view::Server<'a> {
    // SAFETY: per the caller.
    unsafe { view::Server::from_raw(reactor.handle().object()) }
}

/// A handler's server (`reactor->ptr`) and reactor.
fn handler<'a>(reactor: Running<'_>) -> (view::Server<'a>, Reactor) {
    let reactor = Reactor::running(reactor);
    // SAFETY: the handlers below are registered only on server reactors.
    (unsafe { server(reactor) }, reactor)
}

// The handlers below are registered only on server reactors, whose `ptr` is the live
// `Server`.

/// `ReactorThread_onPacketReceived`: a UDP port's datagram.
pub extern "C" fn on_packet_received(reactor: Running<'_>, event: &mut Event) -> c_int {
    let (serv, reactor) = handler(reactor);
    crate::reactor_thread::packet_received(serv, reactor, event)
}

/// `ReactorThread_onClose`: the peer closed.
pub extern "C" fn on_close(reactor: Running<'_>, event: &mut Event) -> c_int {
    let (serv, reactor) = handler(reactor);
    crate::conn_io::peer_close(serv, reactor, event)
}

/// `ReactorThread_onPipeWrite`: a worker pipe can be written.
pub extern "C" fn on_pipe_write(reactor: Running<'_>, event: &mut Event) -> c_int {
    let (serv, reactor) = handler(reactor);
    crate::reactor_io::pipe_write(serv, reactor, event)
}

/// `ReactorThread_onRead`: a connection is readable.
pub extern "C" fn on_read(reactor: Running<'_>, event: &mut Event) -> c_int {
    let (serv, reactor) = handler(reactor);
    crate::conn_io::read(serv, reactor, event)
}

/// `ReactorThread_onWrite`: a connection is writable.
pub extern "C" fn on_write(reactor: Running<'_>, event: &mut Event) -> c_int {
    let (serv, reactor) = handler(reactor);
    crate::conn_io::write(serv, reactor, event)
}

/// `static int Server::close_connection(Reactor *, Socket *)`.
///
/// # Safety
/// `reactor` is a live server `Reactor` and `socket` a live session socket of it.
pub unsafe extern "C-unwind" fn close_connection(
    reactor: *mut c_void,
    socket: *mut SocketRecord,
) -> c_int {
    // SAFETY: per the caller.
    let reactor = unsafe { Reactor::of(reactor) };
    // SAFETY: per the caller.
    let serv = unsafe { server(reactor) };
    // SAFETY: per the caller.
    let sock = unsafe { swoole_rs_reactor::Socket::from_ptr(socket) };
    crate::conn_io::close_connection(serv, reactor, sock)
}

/// `void ReactorThread::shutdown(Reactor *)`, over `serv`'s reactor thread `id` and
/// its reactor.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn thread_shutdown(serv: view::Server<'_>, reactor: Reactor, id: usize) {
    let mut ctx = crate::reactor_ops::ReactorContext::new(serv, Some(reactor));
    ctx.thread = thread(serv.state(), id);
    crate::thread_ops::shutdown(&ctx);
}

/// `static int Server::dispatch_task(const Protocol *, Socket *, const RecvData *)`:
/// a data frame to the workers.
///
/// # Safety
/// `proto` is a live server port's protocol, `socket` a live session socket and
/// `rdata` the frame.
#[cfg(all(feature = "upstream-symbols", not(test)))]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZN6swoole6Server13dispatch_taskEPKNS_8ProtocolEPNS_7network6SocketEPKNS_8RecvDataE"
    )
)]
pub unsafe extern "C-unwind" fn dispatch_task(
    proto: *const swoole_rs_corebase::net_types::Protocol,
    socket: *mut c_void,
    rdata: *const c_void,
) -> c_int {
    use core::sync::atomic::Ordering::SeqCst;
    // SAFETY: per the caller; a port's protocol keeps its server.
    let serv = unsafe { (*proto).private_data_2 };
    // SAFETY: per the caller, a live server's.
    let server = unsafe { view::Server::from_raw(serv) };
    let state = server.state();
    // SAFETY: per the caller, a live session socket.
    let Some(socket) = (unsafe { swoole_rs_reactor::Socket::from_ptr(socket.cast()) }) else {
        return -1;
    };
    // A session socket's object is its live connection.
    let conn = socket
        .read(|s| s.object)
        .cast::<crate::records::ConnectionRecord>();
    // SAFETY: per the caller.
    let rdata = unsafe { &*rdata.cast::<RecvData>() };
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
        let task = crate::base_factory::SendData {
            info: DataHead {
                server_fd: crate::base_factory::low16(c.server_fd),
                reactor_id: c.reactor_id.into(),
                ext_flags: rdata.info.ext_flags,
                kind: swoole_rs_corebase::ServerEvent::RecvData.code(),
                time: c.last_recv_time,
                fd: c.fd.into(),
                len,
                ..DataHead::default()
            },
            data: rdata.data.cast(),
        };
        (
            server.port_of_fd(c.fd),
            c.reactor_id,
            task,
            &c.recv_queued_bytes,
        )
    };
    let mode = u64::from(state.config_ref().mode_);
    let dispatched = if mode == crate::MODE_PROCESS {
        crate::factory::dispatch(state, (&raw mut task).cast())
    } else {
        crate::base_factory::dispatch(server, &mut task, mode == crate::MODE_BASE)
    };
    if !dispatched {
        if len > 0 {
            queued.fetch_sub(len, SeqCst);
        }
        return -1;
    }
    if mode == crate::MODE_PROCESS {
        let thread = thread(state, usize::from(reactor_id)).cast::<ThreadRecord>();
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
