//! The `Server` and `ListenPort` objects, which Rust allocates and frees and C++
//! only views (`native/include/swoole_rs_server_view.h`, `native/include/swoole_rs_port_state.h`).
//! A `Server *` is a [`ServerObject`]; a `ListenPort *` is the box of the port's
//! settings. The Rust ports of core-tests' server cases
//! (tests/modules/http/server/src/http1.rs) make and delete theirs here too.
use core::ffi::{c_int, c_void};
use core::ptr;

use crate::port_config::PortConfig;
use crate::shared::ServerShared;
use crate::{ServerConfig, ServerState, ServerTables, view};
use swoole_rs_corebase::log;
use swoole_rs_locks::Barrier;

/// A `Server`: the head C++ and `view` read, then the reactor threads' barrier.
#[repr(C)]
struct ServerObject {
    state: *mut ServerState,
    config: *mut ServerConfig,
    tables: *mut ServerTables,
    callbacks: *mut c_void,
    reactor_thread_barrier: Barrier,
}

/// `new Server(mode)`: the state and its defaults, the shared record, and the
/// message bus, which numbers its messages from the record's counter. This is
/// `SwooleG.server` from now on. Aborts when the record can't be mapped, as
/// C++'s uncaught `std::bad_alloc` did.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_server_new(mode: c_int) -> *mut c_void {
    let state = crate::swr_v1_server_state_new();
    // SAFETY: the fresh state; it lives until `swr_v1_server_delete` frees it.
    let fresh = unsafe { &*state };
    // SAFETY: the fresh state's configuration, which nothing else reads yet.
    let config = unsafe { &mut *fresh.config() };
    config.init_defaults(
        u32::from(swoole_rs_corebase::swr_v1_core_cpu_num()),
        swoole_rs_utilities::timezone(),
        (&raw const crate::pool_native::STD_ALLOCATOR).cast(),
    );
    // SAFETY: as above, its tables.
    let tables = unsafe { &mut *fresh.tables() };
    tables.max_connection = swoole_rs_corebase::max_sockets().min(100_000);
    config.gs = swoole_rs_memory::members::Shm::calloc(1, size_of::<ServerShared>());
    let gs = config.gs.as_ptr();
    if gs.is_null() {
        log::sys_warning(
            c"Server",
            format_args!("[Master] Fatal Error: failed to allocate memory for Server->gs"),
        );
        std::process::abort();
    }
    // SAFETY: a fresh mapping of a whole `ServerGS`, which starts with the counters,
    // freed only by `swr_v1_server_delete` after the message bus.
    let counters = unsafe { crate::swr_v1_server_shared_init(gs) };
    config.mode_ = u8::try_from(mode).unwrap_or_default();
    tables.message_bus = crate::pool_native::new_bus(&counters.pipe_packet_msg_id);
    let object = Box::new(ServerObject {
        state,
        config,
        tables,
        callbacks: fresh.callbacks_ptr(),
        // SAFETY: all-zero is an unset barrier, as C++'s `Barrier reactor_thread_barrier = {}`.
        reactor_thread_barrier: unsafe { core::mem::zeroed() },
    });
    let serv = Box::into_raw(object).cast::<c_void>();
    swoole_rs_corebase::swr_v1_core_set_server(serv);
    serv
}

/// `delete serv`: destroys a server still running in its master, deletes its
/// ports, closes its pipes, then frees the CPU list, the shared record, the
/// message bus, the state and the object. It stops being `SwooleG.server`.
///
/// # Safety
/// `serv` came from [`swr_v1_server_new`] and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn swr_v1_server_delete(serv: *mut c_void) {
    // SAFETY: per the caller, `serv` is a live `Server` for the call.
    let serv = unsafe { view::Server::from_raw(serv) };
    let counters = serv.counters();
    let master = std::process::id().cast_signed() == counters.master_pid;
    if master
        && counters
            .shutdown
            .load(core::sync::atomic::Ordering::Relaxed)
            == 0
    {
        crate::server_methods::destroy(serv);
    }
    for port in serv.ports() {
        // SAFETY: a live port, deleted once.
        unsafe { port_delete(port.raw()) };
    }
    let object_ptr = serv.raw();
    // SAFETY: the object `swr_v1_server_new` boxed, freed once.
    let object = unsafe { Box::from_raw(object_ptr.cast::<ServerObject>()) };
    // SAFETY: the server's state, which this object owns.
    let state = unsafe { &*object.state };
    state.close_pipes();
    // SAFETY: the state's configuration, which nothing else uses now.
    let config = unsafe { &mut *object.config };
    // SAFETY: `swr_v1_server_new`'s bus, freed once.
    unsafe { crate::pool_native::free_bus(state.tables_ref().message_bus) };
    // After the bus, which numbers its messages from a counter in here.
    config.gs.free();
    // SAFETY: `swr_v1_server_new`'s boxed state, not used again.
    drop(unsafe { Box::from_raw(object.state) });
    if swoole_rs_corebase::swr_v1_core_server() == object_ptr {
        swoole_rs_corebase::swr_v1_core_set_server(ptr::null_mut());
    }
}

/// `new ListenPort(serv)`: a port of `serv` with the default settings.
pub(crate) fn port_new(serv: *mut c_void) -> *mut c_void {
    let buffer_size = swoole_rs_reactor::SOCKET_DEFAULTS.buffer_size();
    let mut config = PortConfig::boxed(c_int::try_from(buffer_size).unwrap_or(c_int::MAX));
    config.protocol.private_data_2 = serv;
    Box::into_raw(config).cast()
}

/// `delete port`: its TLS state, its settings, and the record of a socket
/// `destroy` never closed.
///
/// # Safety
/// `port` came from [`port_new`] and is not used again.
pub(crate) unsafe fn port_delete(port: *mut c_void) {
    // SAFETY: as the caller guarantees: the port is its settings' box, which nothing
    // else uses now.
    let mut config = unsafe { Box::from_raw(port.cast::<PortConfig>()) };
    (crate::port_config::methods().tls_free)(&mut config);
    // SAFETY: the record of a port `destroy` never closed (a server never started),
    // which nothing else holds. Its descriptor stays open to the exit, as upstream's.
    unsafe { swoole_rs_reactor::SocketRecord::free(config.socket.cast()) };
}

/// The reactor threads' barrier, or the manager's, which is shared.
pub(crate) fn barrier(serv: view::Server<'_>, manager: bool) -> &Barrier {
    if manager {
        &ServerShared::of(serv).manager_barrier
    } else {
        // SAFETY: a live server is `swr_v1_server_new`'s object.
        unsafe { &(*serv.raw().cast::<ServerObject>()).reactor_thread_barrier }
    }
}
