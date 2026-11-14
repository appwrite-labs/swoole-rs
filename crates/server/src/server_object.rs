//! The `Server` and `ListenPort` objects, which Rust allocates and frees and C++
//! only views.
//! A `Server *` is a [`ServerObject`]; a `ListenPort *` is the box of the port's
//! settings. The Rust ports of core-tests' server cases
//! (tests/modules/http/server/src/http1.rs) make and delete theirs here too.
use core::ffi::{c_int, c_void};
use core::ptr;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use crate::port_config::PortConfig;
use crate::shared::ServerShared;
use crate::{Mode, ServerConfig, ServerState, ServerTables, view};
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
    let (mut config, mut tables) = crate::state::fresh_records();
    config.init_defaults(
        u32::from(swoole_rs_corebase::swr_v1_core_cpu_num()),
        swoole_rs_utilities::timezone(),
        (&raw const crate::pool_native::STD_ALLOCATOR).cast(),
    );
    tables.max_connection = swoole_rs_corebase::max_sockets().min(100_000);
    config.gs = swoole_rs_memory::members::Shm::calloc(1, size_of::<ServerShared>());
    let gs = config.gs.as_ptr();
    if gs.is_null() {
        log::sys_warning(
            c"Server",
            format_args!("[Master] Fatal Error: failed to allocate memory for Server->gs"),
        );
        swoole_rs_corebase::fatal("out of memory for the server's counters");
    }
    // SAFETY: a fresh mapping of a whole `ServerGS`, which starts with the counters,
    // freed only by `swr_v1_server_delete` after the message bus.
    let counters = unsafe { crate::swr_v1_server_shared_init(gs) };
    config.mode_ = Mode::from_code(mode.into())
        .unwrap_or_else(|| swoole_rs_corebase::fatal("an unknown server mode"));
    tables.message_bus = Some(Box::new(crate::pool_native::new_bus(
        &counters.pipe_packet_msg_id,
    )));
    let fresh = crate::state::boxed(config, tables);
    fresh
        .msg_ids
        .set(&counters.pipe_packet_msg_id)
        .unwrap_or_default();
    let (config, tables) = (fresh.config(), fresh.tables());
    // The state lives until `swr_v1_server_delete` frees it.
    let state = Box::into_raw(fresh);
    let object = Box::new(ServerObject {
        state,
        config,
        tables,
        callbacks: ServerState::callbacks_ptr(state),
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
    crate::server_methods::destroy_running(serv);
    for port in serv.ports() {
        // SAFETY: a live port is its settings' box, deleted once.
        PortConfig::delete(unsafe { Box::from_raw(port.raw().cast::<PortConfig>()) });
    }
    let object_ptr = serv.raw();
    // The server's state, which its object owns until the last step below.
    let state = serv.state();
    // SAFETY: the object `swr_v1_server_new` boxed, freed once.
    let object = unsafe { Box::from_raw(object_ptr.cast::<ServerObject>()) };
    state.close_pipes();
    // SAFETY: the state's configuration, which nothing else uses now.
    let config = unsafe { &mut *object.config };
    let mut gs = core::mem::take(&mut config.gs);
    // SAFETY: `swr_v1_server_new`'s boxed state, not used again. Its tables own the
    // message bus.
    drop(unsafe { Box::from_raw(object.state) });
    // After the bus, which numbers its messages from a counter in here.
    gs.free();
    if swoole_rs_corebase::swr_v1_core_server() == object_ptr {
        swoole_rs_corebase::swr_v1_core_set_server(ptr::null_mut());
    }
}

/// A server [`SharedServer::new`] made, which deletes it with its last share.
struct Owner(*mut c_void);

// SAFETY: a `Server *` names records that synchronize themselves (atomics and
// shared memory), as the reactor and worker threads that read it rely on.
unsafe impl Send for Owner {}
// SAFETY: as above.
unsafe impl Sync for Owner {}

impl Drop for Owner {
    fn drop(&mut self) {
        // SAFETY: `swr_v1_server_new`'s server, which no share names any more.
        unsafe { swr_v1_server_delete(self.0) };
    }
}

/// The server the last [`SharedServer::new`] made.
static LATEST: Mutex<Weak<Owner>> = Mutex::new(Weak::new());

/// A share in a server: the server lives while any share does, so a holder
/// reads it through [`SharedServer::view`] without a raw pointer. The objects
/// that name a server each hold one. The last share may drop where the server's
/// shutdown callbacks can't run any more: a holder that must run them first calls
/// [`destroy_running`](crate::server_methods::destroy_running).
#[derive(Clone)]
pub struct SharedServer(Arc<Owner>);

impl SharedServer {
    /// `new Server(mode)` ([`swr_v1_server_new`]), deleted
    /// ([`swr_v1_server_delete`]) when its last share drops.
    #[must_use]
    pub fn new(mode: c_int) -> Self {
        let owner = Arc::new(Owner(swr_v1_server_new(mode)));
        *LATEST.lock().unwrap_or_else(PoisonError::into_inner) = Arc::downgrade(&owner);
        Self(owner)
    }

    /// `sw_server()` as a share: the process's server (`SwooleG.server`), while it
    /// is one a share still names.
    #[must_use]
    pub fn current() -> Option<Self> {
        let latest = LATEST.lock().unwrap_or_else(PoisonError::into_inner);
        let owner = latest.upgrade()?;
        (owner.0 == swoole_rs_corebase::swr_v1_core_server()).then_some(Self(owner))
    }

    /// The server, for as long as this share is borrowed.
    #[must_use]
    pub fn view(&self) -> view::Server<'_> {
        // SAFETY: the owner's live server, deleted only once every share is gone.
        unsafe { view::Server::from_raw(self.0.0) }
    }
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
