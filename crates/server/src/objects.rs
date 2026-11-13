//! Handles over the native objects the worker ops act on: a message bus, a
//! reactor and a socket. Each is made from a pointer its maker knows is live for as
//! long as the handle is used, so the operations on it need no further proof.
use crate::Mode;
use core::ffi::{c_int, c_void};

use swoole_rs_corebase::{DataHead, TrustedAllocator};
use swoole_rs_reactor::Socket;

use swoole_rs_bus::{BusPacket, MessageBus};

use crate::pool_native::PoolBus;
use swoole_rs_reactor::{reactor_if_exit, reactor_wait};

pub(crate) use crate::view::{set_nonblock, worker_pipe};

/// `f` over `serv->get_worker_pipe_*_in_message_bus(worker)`: in thread mode, the
/// thread bus's own end of the worker's `pipe`; `None` without one.
pub(crate) fn in_worker_bus<R>(
    state: &crate::ServerState,
    config: &crate::ServerConfig,
    pipe: Option<Socket<'_>>,
    f: impl FnOnce(Socket<'_>) -> R,
) -> Option<R> {
    let pipe = pipe?;
    if config.mode_ == Mode::Thread {
        crate::master_ops::worker_message_bus(state)
            .pipe_socket(pipe)
            .map(f)
    } else {
        Some(f(pipe))
    }
}

/// Runs `f` over this thread's timer (`SwooleTG.timer`), if it has one.
fn thread_timer<R>(f: impl FnOnce(&swoole_rs_timer::Owner) -> R) -> Option<R> {
    let timer = swoole_rs_corebase::thread_timer().cast::<swoole_rs_timer::Owner>();
    // SAFETY: null, or this thread's live timer, which only this thread frees and
    // neither caller below does; the borrow ends with `f`.
    unsafe { timer.as_ref() }.map(f)
}

/// Whether this thread has a timer that no reactor drives (`!timer->get_reactor()`).
pub(crate) fn timer_detached() -> bool {
    thread_timer(|timer| swoole_rs_timer::owner_reactor(timer).is_null()).unwrap_or(false)
}

/// `SwooleTG.timer->reinit(manually_trigger)`, when this thread has a timer.
pub(crate) fn timer_reinit(manually_trigger: bool) {
    thread_timer(|timer| swoole_rs_timer::owner_reinit(timer, manually_trigger));
}

// Only the upstream-symbol operations, which the unit tests don't link, use these.
#[cfg(not(test))]
impl crate::ServerState {
    /// The server's own `MessageBus` record.
    pub(crate) fn message_bus_record(&self) -> &PoolBus {
        // SAFETY: `swr_v1_server_new`'s boxed record, which lives as long as the
        // server; it frees it in `swr_v1_server_delete`, with the state.
        unsafe { &*self.tables_ref().message_bus.cast::<PoolBus>() }
    }

    /// The server's own `MessageBus`.
    pub(crate) fn message_bus(&self) -> Bus<'_> {
        Bus::over(self.message_bus_record())
    }

    /// `serv->get_worker_message_bus()`'s record: this thread's in thread mode, else
    /// the server's.
    pub(crate) fn worker_bus_record(&self) -> &PoolBus {
        if self.config_ref().mode_ != Mode::Thread {
            return self.message_bus_record();
        }
        let bus = swoole_rs_corebase::thread_message_bus();
        // SAFETY: a thread-mode thread that sends to workers has its bus: a reactor
        // thread's record's, set for its loop, or the one `create_message_bus` boxed
        // for a task or user worker thread until it ends. Either outlives the server's
        // use on this thread, and only this thread uses it.
        unsafe { &*bus.cast::<PoolBus>() }
    }
}

/// A `MessageBus`: its Rust bus and the allocator of its packets, as its record
/// ([`PoolBus`]) paired them when the handle was made.
#[derive(Clone, Copy)]
pub(crate) struct Bus<'a> {
    bus: &'a MessageBus,
    allocator: TrustedAllocator,
}

impl<'a> Bus<'a> {
    /// The bus of `record`, which a thread's own record lends.
    pub(crate) fn over(record: &'a PoolBus) -> Self {
        let (bus, allocator) = record.parts();
        Self { bus, allocator }
    }
    /// The Rust bus, used by this thread alone while the handle is.
    pub(crate) fn rust(&self) -> &MessageBus {
        self.bus
    }
    /// The allocator of its packets.
    pub(crate) fn allocator(self) -> TrustedAllocator {
        self.allocator
    }
    /// The bus's buffer, which starts with the head of its message.
    pub(crate) fn buffer(self) -> *mut DataHead {
        self.rust().buffer()
    }
    pub(crate) fn head(self) -> DataHead {
        // SAFETY: the live bus's buffer.
        unsafe { *self.buffer() }
    }
    /// The whole packet the bus holds.
    #[inline]
    pub(crate) fn packet(self) -> BusPacket {
        self.rust().packet()
    }
    /// `f` over the bytes of the packet the bus holds, which stay put while it runs.
    pub(crate) fn with_packet<R>(self, f: impl FnOnce(&[u8]) -> R) -> R {
        let packet = self.packet();
        if packet.data.is_null() {
            return f(&[]);
        }
        // SAFETY: the bus's packet, `length` bytes it keeps until it pops or reads again,
        // which this thread alone would do and not during `f`.
        f(unsafe { core::slice::from_raw_parts(packet.data.cast_const().cast(), packet.length) })
    }
    pub(crate) fn pop(self) {
        self.rust().pop();
    }
    pub(crate) fn is_empty(self) -> bool {
        self.rust().count() == 0
    }
    pub(crate) fn clear(self) {
        self.rust().clear();
    }
    /// `MessageBus::read(sock)`.
    pub(crate) fn read(self, sock: Socket<'_>) -> isize {
        self.rust().read_from(self.allocator, sock, false)
    }
    /// `MessageBus::read_with_buffer(sock)`, of a datagram pipe.
    pub(crate) fn read_dgram(self, sock: Socket<'_>) -> isize {
        self.rust().read_from(self.allocator, sock, true)
    }
    /// `MessageBus::write(sock, {head, data})`, `head.len` set to the data's length:
    /// whether the whole message went.
    pub(crate) fn write(self, sock: Socket<'_>, head: &mut DataHead, data: &[u8]) -> bool {
        // SAFETY: every caller passes a worker pipe (a command's reply socket is
        // one), which the server keeps until it shuts down, after its event loop.
        unsafe { self.rust().send(sock, head, data) }
    }
    /// `MessageBus::set_always_chunked_transfer()`.
    pub(crate) fn set_always_chunked(self) {
        self.rust().set_always_chunked();
    }
    /// `MessageBus::init_pipe_socket(sock)`.
    pub(crate) fn init_pipe_socket(self, fd: c_int) {
        self.rust().init_pipe_socket(fd);
    }
    /// `MessageBus::alloc_buffer()`, after `set_buffer_size(size)`: whether the
    /// allocator gave the bus its buffer.
    pub(crate) fn alloc_buffer(self, size: usize) -> bool {
        self.rust().alloc_buffer(size, self.allocator)
    }
    /// `MessageBus::free_buffer()`.
    pub(crate) fn free_buffer(self) {
        self.rust().free_buffer();
    }
    /// `get_pipe_socket(sock)`: this bus's own end of the pipe `sock`.
    pub(crate) fn pipe_socket(&self, sock: Socket<'_>) -> Option<Socket<'_>> {
        self.rust().pipe(sock.fd())
    }
    /// This bus's own end of the pipe `fd`.
    pub(crate) fn pipe_fd(self, fd: c_int) -> *mut c_void {
        self.rust().pipe_socket(fd).cast()
    }
}

pub(crate) use crate::reactor_handle::{EVENT_READ, Reactor};

impl Reactor {
    /// `reactor->drain_write_buffer(sock)`.
    pub(crate) fn drain(self, sock: Socket<'_>) {
        swoole_rs_tls::reactor_write::reactor_drain_write_buffer(self.handle(), sock);
    }
    pub(crate) fn if_exit(self) -> bool {
        reactor_if_exit(self.handle())
    }
    pub(crate) fn wait(self) -> c_int {
        reactor_wait(self.handle())
    }
}
