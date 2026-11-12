//! Handles over the native objects the worker ops act on: a message bus, a
//! reactor and a socket. Each is made from a pointer its maker knows is live for as
//! long as the handle is used, so the operations on it need no further proof.
use core::ffi::{c_int, c_void};

use swoole_rs_corebase::{CoreAllocator, DataHead};
use swoole_rs_reactor::Socket;

use swoole_rs_bus::{BusPacket, MessageBus};

use crate::pool_native::read_bus;
use swoole_rs_reactor::{reactor_if_exit, reactor_wait};

pub(crate) use crate::view::worker_pipe;

/// `f` over `serv->get_worker_pipe_*_in_message_bus(worker)`: in thread mode, the
/// thread bus's own end of the worker's `pipe`; `None` without one.
pub(crate) fn in_worker_bus<R>(
    state: &crate::ServerState,
    config: &crate::ServerConfig,
    pipe: Option<Socket<'_>>,
    f: impl FnOnce(Socket<'_>) -> R,
) -> Option<R> {
    let pipe = pipe?;
    if u64::from(config.mode_) == crate::MODE_THREAD {
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

/// `sock->set_nonblock()`.
pub(crate) fn set_nonblock(sock: Socket<'_>) {
    let set = swoole_rs_reactor::socket_upstream::socket_set_fd_option;
    let _ = sock.edit(|sock| set(sock, 1, -1));
}

#[cfg(not(test))]
impl crate::ServerState {
    /// The server's own `MessageBus`.
    pub(crate) fn message_bus(&self) -> Bus {
        // SAFETY: `swr_v1_server_new`'s bus, which lives as long as the server.
        unsafe { Bus::of(self.tables_ref().message_bus) }
    }
}

/// A `MessageBus`: its Rust bus and the allocator of its packets.
#[derive(Clone, Copy)]
pub(crate) struct Bus {
    bus: *mut c_void,
    allocator: *const CoreAllocator,
}

impl Bus {
    /// The `MessageBus` at `message_bus`.
    ///
    /// # Safety
    /// `message_bus` is a live `MessageBus` while the handle is used, and only this
    /// thread uses it meanwhile.
    pub(crate) unsafe fn of(message_bus: *mut c_void) -> Self {
        // SAFETY: per the caller; a `MessageBus` starts with these two words.
        let [bus, allocator] = unsafe { *message_bus.cast::<[*mut c_void; 2]>() };
        Self {
            bus,
            allocator: allocator.cast(),
        }
    }
    /// The bus and allocator stored apart.
    ///
    /// # Safety
    /// As for [`Bus::of`]: a live bus and the allocator of its packets.
    pub(crate) unsafe fn from_parts(bus: *mut c_void, allocator: *const CoreAllocator) -> Self {
        Self { bus, allocator }
    }
    /// The Rust bus, used by this thread alone while the handle is.
    #[expect(
        clippy::mut_from_ref,
        reason = "the handle's maker lends the bus to this thread"
    )]
    pub(crate) fn rust(&self) -> &mut MessageBus {
        // SAFETY: `of`'s and `from_parts`' contract: a live bus only this thread uses.
        unsafe { &mut *self.bus.cast::<MessageBus>() }
    }
    /// The allocator of its packets.
    pub(crate) fn allocator(self) -> *const CoreAllocator {
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
        read_bus(self.rust(), self.allocator, sock, false)
    }
    /// `MessageBus::write(sock, {head, data})`, `head.len` set to the data's length:
    /// whether the whole message went.
    pub(crate) fn write(self, sock: Socket<'_>, head: &mut DataHead, data: &[u8]) -> bool {
        head.len = u32::try_from(data.len()).unwrap_or(u32::MAX);
        // SAFETY: a live socket, and `head.len` readable bytes.
        unsafe { self.rust().send(sock.as_ptr(), head, data.as_ptr()) }
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
        let bus = self.rust();
        bus.set_buffer_size(size);
        // SAFETY: the bus's allocator outlives it.
        let malloc = unsafe { (*self.allocator).malloc };
        // SAFETY: the allocator's malloc.
        let buffer = malloc.map_or(core::ptr::null_mut(), |malloc| unsafe {
            malloc(bus.buffer_size())
        });
        // SAFETY: the bus takes the fresh buffer of its size.
        unsafe { bus.set_buffer(buffer) };
        !buffer.is_null()
    }
    /// `MessageBus::free_buffer()`.
    pub(crate) fn free_buffer(self) {
        let buffer = self.buffer();
        // SAFETY: the bus's allocator outlives it.
        if let Some(free) = unsafe { (*self.allocator).free } {
            // SAFETY: the bus's buffer, from this allocator, unused afterwards.
            unsafe { free(buffer.cast()) };
        }
        // SAFETY: null forgets the buffer.
        unsafe { self.rust().set_buffer(core::ptr::null_mut()) };
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
