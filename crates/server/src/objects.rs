//! Handles over the native objects the worker ops act on: a message bus, a
//! reactor and a socket. Each is made from a pointer its maker knows is live for as
//! long as the handle is used, so the operations on it need no further proof.
use crate::Mode;
use core::ffi::{c_int, c_void};

use swoole_rs_corebase::reactor::{Heap, Socket};
use swoole_rs_corebase::{DataHead, TrustedAllocator};

use swoole_rs_bus::MessageBus;

use crate::pool_native::PoolBus;
use swoole_rs_corebase::reactor::{reactor_if_exit, reactor_wait};

pub(crate) use crate::view::{set_nonblock, worker_pipe};

/// `f` over `serv->get_worker_pipe_*_in_message_bus(worker)`: in thread mode, the
/// thread bus's own end of the worker's `pipe`; `None` without one.
pub(crate) fn in_worker_bus<R>(
    state: &crate::ServerState,
    config: &crate::ServerConfig,
    pipe: Option<Heap<'_>>,
    f: impl FnOnce(Heap<'_>) -> R,
) -> Option<R> {
    let pipe = pipe?;
    if config.mode_ == Mode::Thread {
        crate::master_ops::worker_message_bus(state)
            .pipe_socket(*pipe)
            .map(f)
    } else {
        Some(f(pipe))
    }
}

// Only the upstream-symbol operations use these.
#[cfg(feature = "upstream-symbols")]
impl crate::ServerState {
    /// The server's own `MessageBus` record.
    pub(crate) fn message_bus_record(&self) -> &PoolBus {
        self.tables_ref()
            .message_bus
            .as_deref()
            .unwrap_or_else(|| swoole_rs_corebase::fatal("a server without its message bus"))
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
    /// The head of the message the bus holds.
    #[inline]
    pub(crate) fn head(self) -> DataHead {
        self.rust().head()
    }
    /// `f` over the bytes of the packet the bus holds, which it lends meanwhile.
    #[inline]
    pub(crate) fn with_packet<R>(self, f: impl FnOnce(&[u8]) -> R) -> R {
        self.rust().with_packet(f)
    }
    /// `f` over the message the bus holds: a copy of `info`, its `len` the packet's,
    /// and the packet's bytes, which the bus lends meanwhile.
    #[inline]
    pub(crate) fn with_message<R>(
        self,
        info: &DataHead,
        f: impl FnOnce(&crate::base_factory::SendData<'_>) -> R,
    ) -> R {
        self.with_packet(|data| f(&crate::base_factory::SendData::lent(*info, data)))
    }
    /// `f` over the event the bus just read whole into its buffer, as its sender
    /// packed it: an `EventData`, its head and the payload the head counts. `None`,
    /// without calling `f`, while the bus holds its message elsewhere (chunked into a
    /// packet, or passed by pointer).
    #[cfg_attr(
        not(feature = "upstream-symbols"),
        expect(
            dead_code,
            reason = "only the hybrid's workers and reactors read events"
        )
    )]
    pub(crate) fn with_event<R>(
        self,
        f: impl FnOnce(swoole_rs_corebase::process::task_file::Packed<'_>) -> R,
    ) -> Option<R> {
        self.rust().with_whole(|event| {
            swoole_rs_corebase::process::task_file::Packed::from_bytes(event).map(f)
        })?
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
    /// `MessageBus::read(sock)`: a whole packet's length, `Some(0)` to wait, `None`
    /// (upstream's -1) on an error.
    pub(crate) fn read(self, sock: Socket<'_>) -> Option<usize> {
        self.rust().read_from(self.allocator, sock, false)
    }
    /// `MessageBus::read_with_buffer(sock)`, of a datagram pipe.
    pub(crate) fn read_dgram(self, sock: Socket<'_>) -> Option<usize> {
        self.rust().read_from(self.allocator, sock, true)
    }
    /// `MessageBus::write(sock, {head, data})`, `head.len` set to the data's length:
    /// whether the whole message went.
    pub(crate) fn write(self, sock: Heap<'_>, head: &mut DataHead, data: &[u8]) -> bool {
        head.len = u32::try_from(data.len()).unwrap_or(u32::MAX);
        self.rust().send(sock, head, Some(data))
    }
    /// `MessageBus::set_always_chunked_transfer()`.
    pub(crate) fn set_always_chunked(self) {
        self.rust().set_always_chunked();
    }
    /// `MessageBus::init_pipe_socket(sock)` of each of `fds`.
    pub(crate) fn init_pipe_sockets(self, fds: impl IntoIterator<Item = c_int>) {
        self.rust().init_pipe_sockets(fds);
    }
    /// The reusable receive buffer lives with the bus, including past a PHP
    /// request's bailout. Only transferable packets use the request allocator.
    pub(crate) fn alloc_buffer(self, size: usize) -> bool {
        self.rust().alloc_buffer(size, TrustedAllocator::STD)
    }
    /// `MessageBus::free_buffer()`.
    pub(crate) fn free_buffer(self) {
        self.rust().free_buffer();
    }
    /// `get_pipe_socket(sock)`: this bus's own end of the pipe `sock`.
    pub(crate) fn pipe_socket(&self, sock: Socket<'_>) -> Option<Heap<'_>> {
        self.rust().pipe(sock.fd())
    }
    /// This bus's own end of the pipe `fd`.
    pub(crate) fn pipe_fd(self, fd: c_int) -> *mut c_void {
        self.rust().pipe_socket(fd).cast()
    }
}

pub(crate) use crate::reactor_handle::Reactor;

impl Reactor {
    /// `reactor->drain_write_buffer(sock)`.
    pub(crate) fn drain(self, sock: Socket<'_>) {
        if let Some(sock) = crate::reactor_handle::registered(sock) {
            swoole_rs_corebase::tls::reactor_write::reactor_drain_write_buffer(self.handle(), sock);
        }
    }
    pub(crate) fn if_exit(self) -> bool {
        reactor_if_exit(self.handle())
    }
    /// `reactor->wait()`: whether the loop ended without an error.
    pub(crate) fn wait(self) -> bool {
        reactor_wait(self.handle()) == 0
    }
}
