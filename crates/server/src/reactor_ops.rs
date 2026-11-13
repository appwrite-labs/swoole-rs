//! A reactor thread's context and record, and the connection and socket checks
//! its handlers share.
use core::cell::Cell;
use core::ffi::{c_int, c_void};

use swoole_rs_buffer::cpp::CppBuffer;
use swoole_rs_corebase::{CoreAllocator, DataHead, TrustedAllocator};
use swoole_rs_reactor::{SocketFields, SocketRecord};

use crate::reactor_handle::Reactor;
use crate::records::Connection;
use crate::view;

/// `SW_FD_SESSION`.
const FD_SESSION: c_int = 0;
/// The objects a reactor-thread op acts on: a live server, the reactor the op runs
/// under, and what the ops before it found. The entry points (`reactor_entry`) build
/// it: one of the server's reactor threads, and (null or live while the context is) a
/// worker's message in this thread's bus. Its readers only test for null.
#[cfg_attr(
    not(all(feature = "upstream-symbols", not(test))),
    expect(dead_code, reason = "only the reactor threads' shutdown reads it")
)]
pub(crate) struct ReactorContext<'a> {
    pub(crate) serv: view::Server<'a>,
    pub(crate) reactor: Option<Reactor>,
    pub(crate) thread: Option<&'a ThreadRecord>,
    /// The `PipeBuffer` read from a worker, which starts with its head.
    pub(crate) resp: *const DataHead,
}

impl<'a> ReactorContext<'a> {
    /// A context over `serv` and `reactor` with nothing else set.
    #[cfg_attr(
        not(all(feature = "upstream-symbols", not(test))),
        expect(
            dead_code,
            reason = "only the reactor threads' setup and shutdown make one"
        )
    )]
    pub(crate) fn new(serv: view::Server<'a>, reactor: Option<Reactor>) -> Self {
        Self {
            serv,
            reactor,
            thread: None,
            resp: core::ptr::null(),
        }
    }

    /// The context's reactor thread, once an op has set it.
    #[cfg_attr(
        not(all(feature = "upstream-symbols", not(test))),
        expect(dead_code, reason = "only the reactor threads' ops call it")
    )]
    pub(crate) fn thread_record(&self) -> Option<&'a ThreadRecord> {
        self.thread
    }

    /// The worker's message the pipe read stored.
    #[cfg_attr(
        not(all(feature = "upstream-symbols", not(test))),
        expect(dead_code, reason = "only the reactor threads' ops call it")
    )]
    pub(crate) fn message(&self) -> Option<&'a DataHead> {
        // SAFETY: null or the head of this thread's bus buffer, as the struct says.
        unsafe { self.resp.as_ref() }
    }
}

/// `Server::is_stream_event`.
pub(crate) fn stream_event(kind: u8) -> bool {
    // RECV_DATA, SEND_DATA, SEND_FILE, CLOSE, CONNECT, PAUSE_RECV, RESUME_RECV,
    // BUFFER_FULL and BUFFER_EMPTY.
    matches!(kind, 0 | 2..=5 | 11..=14)
}

pub(crate) use swoole_rs_ossupport::pthread_self;

/// A socket's output buffer, or `None` without one.
pub(crate) fn out_buffer(socket: &SocketFields) -> Option<&CppBuffer> {
    socket.output.buffer()
}

/// `Socket::is_dgram(type)`.
pub(crate) fn is_dgram(kind: c_int) -> bool {
    matches!(kind, 2 | 4 | 6)
}

/// `Server::is_valid_connection`.
pub(crate) fn valid(conn: Option<Connection<'_>>) -> bool {
    let Some(conn) = conn.filter(|conn| conn.is_active()) else {
        return false;
    };
    conn.socket()
        .is_some_and(|socket| socket.read(|s| s.fd_type == FD_SESSION))
}

/// `ReactorThread`; server.cc asserts this layout. Its own thread writes it, so the
/// fields it sets are cells behind a shared reference.
#[repr(C)]
pub(crate) struct ThreadRecord {
    pub(crate) id: Cell<c_int>,
    pub(crate) notify_pipe: Cell<*mut SocketRecord>,
    pub(crate) dispatch_count: Cell<u64>,
    pub(crate) pipe_command: Cell<*mut SocketRecord>,
    pub(crate) heartbeat_timer: Cell<*mut c_void>,
    /// `message_bus`: its Rust bus, then the allocator of its buffer and packets.
    pub(crate) bus: PoolBus,
}

/// `SwooleG.std_allocator`.
pub(crate) static STD_ALLOCATOR: CoreAllocator = CoreAllocator {
    malloc: Some(libc::malloc),
    calloc: Some(libc::calloc),
    realloc: Some(libc::realloc),
    free: Some(libc::free),
};

/// A `MessageBus` record: the Rust bus, then the allocator its packets and buffer
/// come from. Only the constructors here fill one, with `SwooleG.std_allocator` or
/// another record's allocator, and only [`PoolBus::set_allocator`] replaces it, with
/// one a [`TrustedAllocator`] vouched for.
#[repr(C)]
pub(crate) struct PoolBus {
    bus: Box<swoole_rs_bus::MessageBus>,
    /// A cell: [`PoolBus::set_allocator`] replaces it in place.
    allocator: Cell<TrustedAllocator>,
}

#[cfg_attr(
    not(all(feature = "upstream-symbols", not(test))),
    expect(dead_code, reason = "only the hybrid makes and reads message buses")
)]
impl PoolBus {
    /// A record of `bus` over `SwooleG.std_allocator`.
    pub(crate) fn std(bus: Box<swoole_rs_bus::MessageBus>) -> Self {
        // SAFETY: libc's `malloc`, `calloc`, `realloc` and `free`, which keep the
        // allocator contract.
        let std = unsafe { TrustedAllocator::new(&STD_ALLOCATOR) };
        Self {
            bus,
            allocator: Cell::new(std),
        }
    }

    /// The Rust bus and the allocator of its buffer and packets, as they are now.
    pub(crate) fn parts(&self) -> (&swoole_rs_bus::MessageBus, TrustedAllocator) {
        (&self.bus, self.allocator.get())
    }

    /// `set_allocator(allocator)`: the bus allocates its packets with `allocator`
    /// from now on.
    pub(crate) fn set_allocator(&self, allocator: TrustedAllocator) {
        self.allocator.set(allocator);
    }

    /// The bus a pool's `create_message_bus` made, or `None` before it or after its
    /// `destroy`.
    pub(crate) fn of_pool(pool: swoole_rs_process::process_pool::PoolRef<'_>) -> Option<&Self> {
        // SAFETY: crates/process keeps a pool's `message_bus` private: null, or what
        // this crate's `PoolNative` answered `Op::NewMessageBus` with, a boxed record
        // the pool keeps until `Op::FreeMessageBus` in its `destroy` frees it and the
        // pool forgets it. The pool, and so its bus, belongs to the process or thread
        // that runs its operations, as this call does.
        unsafe { pool.message_bus().cast::<Self>().as_ref() }
    }
}

/// `pool->message_bus->move_packet()`: the bus's current packet block, now the
/// caller's; null for a pool without a bus or a packet.
#[must_use]
pub fn pool_bus_move_packet(
    pool: swoole_rs_process::process_pool::PoolRef<'_>,
) -> *mut core::ffi::c_char {
    PoolBus::of_pool(pool).map_or(core::ptr::null_mut(), |bus| bus.bus.move_packet())
}

/// `pool->message_bus->set_allocator(allocator)`, when the pool has a bus: its
/// buffer and packets come from `allocator` from now on.
pub fn pool_bus_set_allocator(
    pool: swoole_rs_process::process_pool::PoolRef<'_>,
    allocator: TrustedAllocator,
) {
    if let Some(bus) = PoolBus::of_pool(pool) {
        bus.set_allocator(allocator);
    }
}

#[cfg(all(feature = "upstream-symbols", not(test)))]
impl ThreadRecord {
    /// `message_bus`.
    pub(crate) fn message_bus(&self) -> crate::objects::Bus<'_> {
        crate::objects::Bus::over(&self.bus)
    }
}

const _: () = assert!(
    core::mem::offset_of!(ThreadRecord, heartbeat_timer) == 32
        && core::mem::offset_of!(ThreadRecord, bus) == 40
        && size_of::<ThreadRecord>() == 56,
    "server.cc ReactorThread layout"
);

#[cfg(all(feature = "upstream-symbols", not(test)))]
impl ThreadRecord {
    /// `new ReactorThread[count]()`: each with a fresh bus over the allocator of `like`.
    pub(crate) fn new_array(count: u16, like: crate::objects::Bus<'_>) -> Box<[Self]> {
        let size = swoole_rs_corebase::constants::SW_BUFFER_SIZE_STD as usize;
        (0..count)
            .map(|_| Self {
                id: Cell::new(0),
                notify_pipe: Cell::new(core::ptr::null_mut()),
                dispatch_count: Cell::new(0),
                pipe_command: Cell::new(core::ptr::null_mut()),
                heartbeat_timer: Cell::new(core::ptr::null_mut()),
                bus: PoolBus {
                    bus: swoole_rs_bus::MessageBus::create(size),
                    // A record's own allocator, which every `Bus` carries.
                    allocator: Cell::new(like.allocator()),
                },
            })
            .collect()
    }
}
