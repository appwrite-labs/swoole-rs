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
    /// The whole packet the bus holds.
    #[inline]
    pub(crate) fn packet(self) -> BusPacket {
        self.rust().packet()
    }
    /// `f` over the bytes of the packet the bus holds, which stay put while it runs.
    pub(crate) fn with_packet<R>(self, f: impl FnOnce(&[u8]) -> R) -> R {
        self.with_message(&DataHead::default(), |message| f(message.payload()))
    }
    /// `f` over the message the bus holds: a copy of `info`, its `len` the packet's,
    /// and the packet's bytes, which stay put while it runs.
    #[inline]
    pub(crate) fn with_message<R>(
        self,
        info: &DataHead,
        f: impl FnOnce(&crate::base_factory::SendData<'_>) -> R,
    ) -> R {
        let packet = self.packet();
        let len = u32::try_from(packet.length).unwrap_or(u32::MAX);
        // SAFETY: the bus's packet, `length` bytes (or null) it keeps until it pops or
        // reads again, which this thread alone would do and not during `f`.
        let message = unsafe {
            crate::base_factory::SendData::from_raw(DataHead { len, ..*info }, packet.data)
        };
        f(&message)
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
        f: impl FnOnce(swoole_rs_process::task_file::Packed<'_>) -> R,
    ) -> Option<R> {
        if !self.rust().holds_whole() {
            return None;
        }
        // SAFETY: the buffer's head and the `len` bytes after it, within the buffer,
        // which keeps them until the bus reads again or frees it, which this thread
        // alone would do and not during `f`.
        let event =
            unsafe { swoole_rs_process::task_file::Packed::from_raw(self.rust().buffer().cast()) };
        Some(f(event))
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
    pub(crate) fn write(self, sock: Socket<'_>, head: &mut DataHead, data: &[u8]) -> bool {
        head.len = u32::try_from(data.len()).unwrap_or(u32::MAX);
        // Every caller passes a worker pipe (a command's reply socket is one), which the
        // server keeps until it shuts down, after its event loop.
        let Some(sock) = crate::reactor_handle::registered(sock) else {
            return false;
        };
        // SAFETY: `data` covers `head.len` bytes (at most its length).
        unsafe { self.rust().send_raw(sock, head, data.as_ptr()) }
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

pub(crate) use crate::reactor_handle::Reactor;

impl Reactor {
    /// `reactor->drain_write_buffer(sock)`.
    pub(crate) fn drain(self, sock: Socket<'_>) {
        if let Some(sock) = crate::reactor_handle::registered(sock) {
            swoole_rs_tls::reactor_write::reactor_drain_write_buffer(self.handle(), sock);
        }
    }
    pub(crate) fn if_exit(self) -> bool {
        reactor_if_exit(self.handle())
    }
    pub(crate) fn wait(self) -> c_int {
        reactor_wait(self.handle())
    }
}
