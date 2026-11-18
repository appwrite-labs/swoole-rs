//! A reactor thread's context and record, and the connection and socket checks
//! its handlers share.
use core::cell::Cell;
use core::ffi::c_int;

use swoole_rs_corebase::TrustedAllocator;
use swoole_rs_corebase::buffer::cpp::CppBuffer;
use swoole_rs_corebase::reactor::{SocketFields, SocketRecord};

use crate::reactor_handle::Reactor;
use crate::records::Connection;
use crate::view;

/// `SW_FD_SESSION`.
const FD_SESSION: c_int = 0;
/// The objects a reactor-thread op acts on: a live server, the reactor the op runs
/// under, and its thread record. The entry points (`reactor_entry`) build it;
/// messages stay in the thread's bus.
pub(crate) struct ReactorContext<'a> {
    pub(crate) serv: view::Server<'a>,
    pub(crate) reactor: Option<Reactor>,
    pub(crate) thread: Option<&'a ThreadRecord>,
}

impl<'a> ReactorContext<'a> {
    /// A context over `serv` and `reactor` with nothing else set.
    pub(crate) fn new(serv: view::Server<'a>, reactor: Option<Reactor>) -> Self {
        Self {
            serv,
            reactor,
            thread: None,
        }
    }

    /// The context's reactor thread, once an op has set it.
    pub(crate) fn thread_record(&self) -> Option<&'a ThreadRecord> {
        self.thread
    }
}

pub(crate) use swoole_rs_corebase::ossupport::pthread_self;

/// A socket's output buffer, or `None` without one.
pub(crate) fn out_buffer(socket: &SocketFields) -> Option<&CppBuffer> {
    socket.output.buffer()
}

/// `Server::is_valid_connection`.
pub(crate) fn valid(conn: Option<Connection<'_>>) -> bool {
    let Some(conn) = conn.filter(|conn| conn.is_active()) else {
        return false;
    };
    conn.socket()
        .is_some_and(|socket| socket.read(|s| s.fd_type == FD_SESSION))
}

/// `ReactorThread`. Its own thread writes it, so the fields it sets are cells behind a
/// shared reference.
pub(crate) struct ThreadRecord {
    pub(crate) id: Cell<c_int>,
    /// The worker whose pipe tells the thread to shut down: the first it reads.
    pub(crate) notify_worker: Cell<Option<u16>>,
    pub(crate) dispatch_count: Cell<u64>,
    pub(crate) pipe_command: Cell<*mut SocketRecord>,
    pub(crate) heartbeat_timer: Cell<Option<swoole_rs_corebase::timer_key::Held>>,
    /// `message_bus`: its Rust bus, then the allocator of its packets.
    pub(crate) bus: PoolBus,
}

/// `SwooleG.std_allocator`: libc's.
pub(crate) const STD: TrustedAllocator = TrustedAllocator::STD;

/// A `MessageBus` record: the Rust bus, then the allocator its transferable packets
/// come from. Only the constructors here fill one, with `SwooleG.std_allocator` or
/// another record's allocator, and only [`PoolBus::set_allocator`] replaces it, with
/// one a [`TrustedAllocator`] vouched for.
pub(crate) struct PoolBus {
    bus: Box<swoole_rs_bus::MessageBus>,
    /// A cell: [`PoolBus::set_allocator`] replaces it in place.
    allocator: Cell<TrustedAllocator>,
}

impl PoolBus {
    /// A record of `bus` over `SwooleG.std_allocator`.
    pub(crate) fn std(bus: Box<swoole_rs_bus::MessageBus>) -> Self {
        Self {
            bus,
            allocator: Cell::new(STD),
        }
    }

    /// The Rust bus and the allocator of its packets, as they are now.
    pub(crate) fn parts(&self) -> (&swoole_rs_bus::MessageBus, TrustedAllocator) {
        (&self.bus, self.allocator.get())
    }

    /// `set_allocator(allocator)`: the bus allocates its packets with `allocator`
    /// from now on.
    pub(crate) fn set_allocator(&self, allocator: TrustedAllocator) {
        self.allocator.set(allocator);
    }

    /// One operation over the pool's typed bus, without lending it beyond the call.
    pub(crate) fn with_pool<R>(
        pool: swoole_rs_corebase::process::process_pool::PoolRef<'_>,
        f: impl FnOnce(&Self) -> R,
    ) -> Option<R> {
        pool.with_message_bus(|bus| bus.downcast_ref::<Self>().map(f))?
    }
}

/// `pool->message_bus->set_allocator(allocator)`, when the pool has a bus: its
/// packets come from `allocator` from now on.
pub fn pool_bus_set_allocator(
    pool: swoole_rs_corebase::process::process_pool::PoolRef<'_>,
    allocator: TrustedAllocator,
) {
    PoolBus::with_pool(pool, |bus| bus.set_allocator(allocator));
}

impl ThreadRecord {
    /// `message_bus`.
    pub(crate) fn message_bus(&self) -> crate::objects::Bus<'_> {
        crate::objects::Bus::over(&self.bus)
    }
}

impl ThreadRecord {
    /// `new ReactorThread[count]()`: each with a fresh bus over the allocator of `like`.
    pub(crate) fn new_array(count: u16, like: crate::objects::Bus<'_>) -> Box<[Self]> {
        let size = swoole_rs_corebase::constants::SW_BUFFER_SIZE_STD as usize;
        (0..count)
            .map(|_| Self {
                id: Cell::new(0),
                notify_worker: Cell::new(None),
                dispatch_count: Cell::new(0),
                pipe_command: Cell::new(core::ptr::null_mut()),
                heartbeat_timer: Cell::new(None),
                bus: PoolBus {
                    bus: swoole_rs_bus::MessageBus::create(size),
                    // A record's own allocator, which every `Bus` carries.
                    allocator: Cell::new(like.allocator()),
                },
            })
            .collect()
    }
}
