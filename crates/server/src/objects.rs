//! Handles over the native objects the worker ops act on: a message bus, a
//! reactor and a socket. Each is made from a pointer its maker knows is live for as
//! long as the handle is used, so the operations on it need no further proof.
use core::ffi::{c_int, c_void};

use swoole_rs_corebase::{CoreAllocator, DataHead};
use swoole_rs_reactor::SocketRecord;

use crate::pool_native::{
    BusPacket, read_bus, swr_v1_bus_buffer, swr_v1_bus_buffer_size, swr_v1_bus_packet,
    swr_v1_bus_pop, swr_v1_bus_set_buffer, swr_v1_bus_set_buffer_size,
};

// SAFETY: crates/protocols' bus, linked into the same archive.
unsafe extern "C" {
    fn swr_v1_bus_count(bus: *mut c_void) -> usize;
    fn swr_v1_bus_clear(bus: *mut c_void);
    fn swr_v1_bus_set_always_chunked(bus: *mut c_void);
    fn swr_v1_bus_init_pipe_socket(bus: *mut c_void, fd: c_int);
    fn swr_v1_bus_pipe_socket(bus: *mut c_void, fd: c_int) -> *mut SocketRecord;
    fn swr_v1_bus_write(
        bus: *mut c_void,
        socket: *mut c_void,
        head: *mut DataHead,
        data: *const u8,
    ) -> bool;
}

// SAFETY: crates/reactor's and crates/tls' bodies, under their upstream symbols.
unsafe extern "C-unwind" {
    #[link_name = "_ZN6swoole7Reactor7if_exitEv"]
    fn reactor_if_exit(reactor: *mut c_void) -> bool;
    #[link_name = "_ZN6swoole7Reactor4waitEv"]
    fn reactor_wait(reactor: *mut c_void) -> c_int;
    #[link_name = "_ZN6swoole7Reactor18drain_write_bufferEPNS_7network6SocketE"]
    fn drain_write_buffer(reactor: *mut c_void, socket: *mut c_void);
}

/// A live socket, or `None` for null.
pub(crate) fn socket<'a>(socket: *mut c_void) -> Option<&'a mut SocketRecord> {
    // SAFETY: the native objects hand out null or live sockets, each used by one thread.
    unsafe { socket.cast::<SocketRecord>().as_mut() }
}

/// `serv->get_worker_pipe_*_in_message_bus(worker)`: in thread mode, the thread
/// bus's own end of the worker's `pipe`.
pub(crate) fn in_worker_bus(
    state: &crate::ServerState,
    config: &crate::ServerConfig,
    pipe: *mut c_void,
) -> *mut c_void {
    match socket(pipe) {
        Some(sock) if u64::from(config.mode_) == crate::MODE_THREAD => {
            crate::master_ops::worker_message_bus(state).pipe_socket(sock)
        }
        _ => pipe,
    }
}

/// `sock->set_nonblock()`.
pub(crate) fn set_nonblock(sock: &mut SocketRecord) {
    // SAFETY: a live socket.
    let _ = unsafe { swoole_rs_reactor::socket_upstream::socket_set_fd_option(sock, 1, -1) };
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
    /// The Rust bus.
    pub(crate) fn raw(self) -> *mut c_void {
        self.bus
    }
    /// The allocator of its packets.
    pub(crate) fn allocator(self) -> *const CoreAllocator {
        self.allocator
    }
    /// The bus's buffer, which starts with the head of its message.
    pub(crate) fn buffer(self) -> *mut DataHead {
        // SAFETY: a live bus.
        unsafe { swr_v1_bus_buffer(self.bus) }
    }
    pub(crate) fn head(self) -> DataHead {
        // SAFETY: the live bus's buffer.
        unsafe { *self.buffer() }
    }
    /// The whole packet the bus holds.
    pub(crate) fn packet(self) -> BusPacket {
        // SAFETY: a live bus.
        unsafe { swr_v1_bus_packet(self.bus) }
    }
    pub(crate) fn pop(self) {
        // SAFETY: a live bus.
        unsafe { swr_v1_bus_pop(self.bus) };
    }
    pub(crate) fn is_empty(self) -> bool {
        // SAFETY: a live bus.
        unsafe { swr_v1_bus_count(self.bus) == 0 }
    }
    pub(crate) fn clear(self) {
        // SAFETY: a live bus.
        unsafe { swr_v1_bus_clear(self.bus) };
    }
    /// `MessageBus::read(sock)`.
    pub(crate) fn read(self, sock: &mut SocketRecord) -> isize {
        read_bus(self.bus, self.allocator, sock, false)
    }
    /// `MessageBus::write(sock, {head, data})`, `head.len` set to the data's length:
    /// whether the whole message went.
    pub(crate) fn write(self, sock: &mut SocketRecord, head: &mut DataHead, data: &[u8]) -> bool {
        head.len = u32::try_from(data.len()).unwrap_or(u32::MAX);
        // SAFETY: a live bus and socket, and `head.len` readable bytes.
        unsafe { swr_v1_bus_write(self.bus, (&raw mut *sock).cast(), head, data.as_ptr()) }
    }
    /// `MessageBus::set_always_chunked_transfer()`.
    pub(crate) fn set_always_chunked(self) {
        // SAFETY: a live bus.
        unsafe { swr_v1_bus_set_always_chunked(self.bus) };
    }
    /// `MessageBus::init_pipe_socket(sock)`.
    pub(crate) fn init_pipe_socket(self, fd: c_int) {
        // SAFETY: a live bus.
        unsafe { swr_v1_bus_init_pipe_socket(self.bus, fd) };
    }
    /// `MessageBus::alloc_buffer()`, after `set_buffer_size(size)`: whether the
    /// allocator gave the bus its buffer.
    pub(crate) fn alloc_buffer(self, size: usize) -> bool {
        // SAFETY: a live bus.
        unsafe { swr_v1_bus_set_buffer_size(self.bus, size) };
        // SAFETY: as above.
        let size = unsafe { swr_v1_bus_buffer_size(self.bus) };
        // SAFETY: the bus's allocator outlives it.
        let malloc = unsafe { (*self.allocator).malloc };
        // SAFETY: the allocator's malloc.
        let buffer = malloc.map_or(core::ptr::null_mut(), |malloc| unsafe { malloc(size) });
        // SAFETY: the bus takes the fresh buffer.
        unsafe { swr_v1_bus_set_buffer(self.bus, buffer) };
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
        // SAFETY: a live bus.
        unsafe { swr_v1_bus_set_buffer(self.bus, core::ptr::null_mut()) };
    }
    /// `get_pipe_socket(sock)`: this bus's own end of the pipe `sock`.
    pub(crate) fn pipe_socket(self, sock: &SocketRecord) -> *mut c_void {
        self.pipe_fd(sock.fd)
    }
    /// This bus's own end of the pipe `fd`.
    pub(crate) fn pipe_fd(self, fd: c_int) -> *mut c_void {
        // SAFETY: a live bus.
        unsafe { swr_v1_bus_pipe_socket(self.bus, fd) }.cast()
    }
}

pub(crate) use crate::reactor_handle::{EVENT_READ, Reactor};

impl Reactor {
    /// `reactor->drain_write_buffer(sock)`.
    pub(crate) fn drain(self, sock: &mut SocketRecord) {
        // SAFETY: as above.
        unsafe { drain_write_buffer(self.0, (&raw mut *sock).cast()) };
    }
    pub(crate) fn if_exit(self) -> bool {
        // SAFETY: a live reactor.
        unsafe { reactor_if_exit(self.0) }
    }
    pub(crate) fn wait(self) -> c_int {
        // SAFETY: a live reactor.
        unsafe { reactor_wait(self.0) }
    }
}
