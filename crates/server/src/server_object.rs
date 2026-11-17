//! The `Server` and `ListenPort` objects, which Rust allocates and frees and C++
//! only views.
//! A `Server *` is a [`ServerObject`]; a `ListenPort *` is the box of the port's
//! settings. The Rust ports of core-tests' server cases
//! (tests/modules/http/server/src/http1.rs) make and delete theirs here too.
use core::ffi::c_int;
use core::ptr;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use crate::Mode;
use crate::shared::ServerShared;
use crate::view::{self, ServerObject};
use swoole_rs_corebase::log;

/// `new Server(mode)`: the state and its defaults, the shared record, and the
/// message bus, which numbers its messages from the record's counter. Aborts
/// when the record can't be mapped, as C++'s uncaught `std::bad_alloc` did.
fn swr_v1_server_new(mode: c_int) -> ServerObject {
    let (mut config, mut tables) = crate::state::fresh_records();
    config.init_defaults(
        u32::from(swoole_rs_corebase::swr_v1_core_cpu_num()),
        swoole_rs_corebase::utilities::timezone(),
        Some(crate::reactor_ops::STD),
    );
    tables.set_max_connection(swoole_rs_corebase::max_sockets().min(100_000));
    config.gs = swoole_rs_memory::members::Shm::calloc(1, size_of::<ServerShared>());
    let gs = config.gs.as_ptr();
    if gs.is_null() {
        log::sys_warning(
            c"Server",
            format_args!("[Master] Fatal Error: failed to allocate memory for Server->gs"),
        );
        swoole_rs_corebase::fatal("out of memory for the server's counters");
    }
    // SAFETY: a fresh, zeroed mapping of a whole `ServerGS` (all-zero is a valid one),
    // which nothing else reads yet, freed only by the last share's drop after the
    // message bus.
    let shared: &'static ServerShared = unsafe { &*gs.cast() };
    let counters = crate::swr_v1_server_shared_init(&shared.counters);
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
    ServerObject::new(fresh)
}

/// A server [`SharedServer::new`] made, which deletes it with its last share.
struct Owner(Box<ServerObject>);

impl Owner {
    /// The server, for as long as the owner is borrowed.
    fn view(&self) -> view::Server<'_> {
        view::Server::of_object(&self.0)
    }
}

#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "the object's record pointers are what the SAFETY comment below covers"
)]
// SAFETY: a `Server *` names records that synchronize themselves (atomics and
// shared memory), as the reactor and worker threads that read it rely on.
unsafe impl Send for Owner {}
// SAFETY: as above.
unsafe impl Sync for Owner {}

/// `delete serv` of the server no share names any more: destroys it if it still
/// runs in its master, closes its pipes, then frees the ports, the CPU list,
/// the shared record, the message bus, the state and the object (which owns the
/// state). It stops being `SwooleG.server`.
impl Drop for Owner {
    fn drop(&mut self) {
        let serv = self.view();
        crate::server_methods::destroy_running(serv);
        crate::reactor_handle::release_server(serv);
        let object_ptr = serv.raw();
        if swoole_rs_corebase::swr_v1_core_server() == object_ptr {
            swoole_rs_corebase::swr_v1_core_set_server(ptr::null_mut());
        }
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
    /// `new Server(mode)` ([`swr_v1_server_new`]), deleted when its last share drops.
    #[must_use]
    pub fn new(mode: c_int) -> Self {
        let owner = Arc::new(Owner(Box::new(swr_v1_server_new(mode))));
        // `SwooleG.server` from now on.
        swoole_rs_corebase::swr_v1_core_set_server(owner.view().raw());
        *LATEST.lock().unwrap_or_else(PoisonError::into_inner) = Arc::downgrade(&owner);
        Self(owner)
    }

    /// `sw_server()` as a share: the process's server (`SwooleG.server`), while it
    /// is one a share still names.
    #[must_use]
    pub fn current() -> Option<Self> {
        Self::latest(swoole_rs_corebase::swr_v1_core_server())
    }

    /// A share in `serv`, or fatal when no share names it: a server thread's, which
    /// outlives the borrow it starts under. A share's server is always the latest.
    pub(crate) fn of(serv: view::Server<'_>) -> Self {
        Self::latest(serv.raw())
            .unwrap_or_else(|| swoole_rs_corebase::fatal("a server thread of an unshared server"))
    }

    /// The latest server's share, when it is the server at `raw`.
    fn latest(raw: *mut core::ffi::c_void) -> Option<Self> {
        let latest = LATEST.lock().unwrap_or_else(PoisonError::into_inner);
        let owner = latest.upgrade()?;
        (owner.view().raw() == raw).then_some(Self(owner))
    }

    /// The server, for as long as this share is borrowed.
    #[must_use]
    pub fn view(&self) -> view::Server<'_> {
        self.0.view()
    }
}

/// The reactor threads' barrier, or the manager's, which is shared.
pub(crate) fn barrier(
    serv: view::Server<'_>,
    manager: bool,
) -> &swoole_rs_corebase::locks::Barrier {
    if manager {
        &ServerShared::of(serv).manager_barrier
    } else {
        &serv.state().reactor_thread_barrier
    }
}
