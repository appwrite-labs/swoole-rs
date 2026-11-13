//! A reactor thread's context and record, and the connection and socket checks
//! its handlers share.
use core::cell::Cell;
use core::ffi::{c_int, c_void};

use swoole_rs_buffer::cpp::CppBuffer;
use swoole_rs_corebase::DataHead;
use swoole_rs_reactor::{SocketFields, SocketRecord};

use crate::reactor_handle::Reactor;
use crate::records::Connection;
use crate::view;

/// `SW_FD_SESSION`.
const FD_SESSION: c_int = 0;
/// The objects a reactor-thread op acts on: a live server, the reactor the op runs
/// under, and what the ops before it found. The entry points (`reactor_entry`) build
/// it. Each raw field is null or what the op setting it found, live while the context
/// is: one of the server's reactor threads, one of its connection slots, a worker's
/// message in this thread's bus, this thread's buffer. Its readers only test for null.
#[cfg_attr(
    not(all(feature = "upstream-symbols", not(test))),
    expect(dead_code, reason = "only the reactor threads' shutdown reads it")
)]
pub(crate) struct ReactorContext<'a> {
    pub(crate) serv: view::Server<'a>,
    pub(crate) reactor: Option<Reactor>,
    pub(crate) thread: *mut c_void,
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
        let null = core::ptr::null_mut();
        Self {
            serv,
            reactor,
            thread: null,
            resp: core::ptr::null(),
        }
    }

    /// The context's reactor thread, once an op has set it.
    #[cfg_attr(
        not(all(feature = "upstream-symbols", not(test))),
        expect(dead_code, reason = "only the reactor threads' ops call it")
    )]
    pub(crate) fn thread_record(&self) -> Option<&'a ThreadRecord> {
        // SAFETY: null or one of the server's live reactor threads, as the struct
        // says, which only its own thread touches.
        unsafe { self.thread.cast::<ThreadRecord>().as_ref() }
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

pub(crate) use swoole_rs_ossupport::pthread_self;

/// A socket's output buffer, or `None` without one.
pub(crate) fn out_buffer(socket: &SocketFields) -> Option<&CppBuffer> {
    // SAFETY: a socket's output buffer is null or the live `Buffer` it owns.
    unsafe { socket.output.out_buffer.cast::<CppBuffer>().as_ref() }
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
    pub(crate) bus: *mut c_void,
    pub(crate) allocator: *const swoole_rs_corebase::CoreAllocator,
}

#[cfg(all(feature = "upstream-symbols", not(test)))]
impl ThreadRecord {
    /// `message_bus`.
    pub(crate) fn message_bus(&self) -> crate::objects::Bus {
        // SAFETY: a live thread's bus and allocator, made together.
        unsafe { crate::objects::Bus::from_parts(self.bus, self.allocator) }
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
    /// `new ReactorThread[count]()`: each with a fresh bus over `allocator`.
    pub(crate) fn new_array(
        count: usize,
        allocator: *const swoole_rs_corebase::CoreAllocator,
    ) -> *mut c_void {
        let size = swoole_rs_corebase::constants::SW_BUFFER_SIZE_STD as usize;
        let threads: Box<[Self]> = (0..count)
            .map(|_| Self {
                id: Cell::new(0),
                notify_pipe: Cell::new(core::ptr::null_mut()),
                dispatch_count: Cell::new(0),
                pipe_command: Cell::new(core::ptr::null_mut()),
                heartbeat_timer: Cell::new(core::ptr::null_mut()),
                bus: Box::into_raw(swoole_rs_bus::MessageBus::create(size)).cast(),
                allocator,
            })
            .collect();
        Box::into_raw(threads).cast()
    }

    /// `delete[] threads`: the `count` threads [`Self::new_array`] made, and their buses.
    ///
    /// # Safety
    /// `threads` is [`Self::new_array`]'s array of `count`, not used again.
    pub(crate) unsafe fn delete_array(threads: *mut c_void, count: usize) {
        let threads = core::ptr::slice_from_raw_parts_mut(threads.cast::<Self>(), count);
        // SAFETY: as the caller guarantees.
        let threads = unsafe { Box::from_raw(threads) };
        for thread in &threads {
            // SAFETY: the thread's own bus, `new_array` boxed, freed once.
            drop(unsafe { Box::from_raw(thread.bus.cast::<swoole_rs_bus::MessageBus>()) });
        }
    }
}
