//! Rust-owned shared server and port counters in the existing shared-memory arena.
use crate::records::Lookup;
use core::ffi::c_int;
use core::sync::atomic::{
    AtomicBool, AtomicI32, AtomicI64, AtomicU32, AtomicU64, Ordering::Relaxed,
};
use swoole_rs_memory::members::Shm;
use swoole_rs_process::process_pool::PoolRef;
#[derive(Default)]
#[repr(C)]
pub struct PortCounters {
    pub connection_num: AtomicU32,
    /// An `AtomicU32` per worker.
    pub connection_nums: Shm,
    pub abort_count: AtomicU64,
    pub accept_count: AtomicU64,
    pub close_count: AtomicU64,
    pub dispatch_count: AtomicU64,
    pub request_count: AtomicU64,
    pub response_count: AtomicU64,
    pub total_recv_bytes: AtomicU64,
    pub total_send_bytes: AtomicU64,
}
/// The words of a shared mapping the server made (`connection_nums`' cells, the
/// `task_results` slots): none without one.
pub(crate) fn words(nums: &Shm) -> &[AtomicU32] {
    let base = nums.as_ptr().cast::<AtomicU32>();
    if base.is_null() {
        return &[];
    }
    // SAFETY: only `Shm`'s constructors fill the field: a zeroed shared mapping whose
    // `payload_len` bytes follow `base`, 8-aligned, mapped until its owner frees it at
    // shutdown, and only ever updated atomically.
    unsafe { core::slice::from_raw_parts(base, nums.payload_len() / size_of::<AtomicU32>()) }
}

impl PortCounters {
    /// The per-worker connection counts; none when the server keeps one total.
    pub(crate) fn cells(&self) -> &[AtomicU32] {
        words(&self.connection_nums)
    }
}

impl ServerCounters {
    #[must_use]
    pub fn max_fd(&self) -> c_int {
        self.max_fd.load(Relaxed)
    }

    #[must_use]
    pub fn min_fd(&self) -> c_int {
        self.min_fd.load(Relaxed)
    }

    /// The per-worker connection counts; none when the server keeps one total.
    pub(crate) fn cells(&self) -> &[AtomicU32] {
        words(&self.connection_nums)
    }
}

#[derive(Default)]
#[repr(C)]
pub struct ServerCounters {
    /// `pid_t`s, set by the master and the manager and read by every process.
    pub master_pid: AtomicI32,
    pub manager_pid: AtomicI32,
    /// `long`, read under the shared spin lock.
    pub session_round: AtomicI64,
    pub start: AtomicU32,
    pub shutdown: AtomicU32,
    /// The connection fd range; written under the server lock, read by every thread.
    pub max_fd: AtomicI32,
    pub min_fd: AtomicI32,
    pub onstart_called: AtomicBool,
    /// `time_t`.
    pub start_time: AtomicI64,
    pub connection_num: AtomicU32,
    /// An `AtomicU32` per worker.
    pub connection_nums: Shm,
    pub tasking_num: AtomicU32,
    /// Set by `Server::set()`, read by every worker.
    pub max_concurrency: AtomicU32,
    pub concurrency: AtomicU32,
    pub abort_count: AtomicU64,
    pub accept_count: AtomicU64,
    pub close_count: AtomicU64,
    pub dispatch_count: AtomicU64,
    pub request_count: AtomicU64,
    pub response_count: AtomicU64,
    pub total_recv_bytes: AtomicU64,
    pub total_send_bytes: AtomicU64,
    pub pipe_packet_msg_id: AtomicU64,
    pub task_count: AtomicU64,
    pub spinlock: AtomicU32,
}

/// `ServerGS`, the record a server maps shared (`serv->gs`): the counters, the
/// task and event worker pools, and the manager's barrier. `layout.rs` checks
/// native's against it (record 8).
#[repr(C)]
pub(crate) struct ServerShared {
    pub(crate) counters: ServerCounters,
    /// The task pool, then the event pool: records every process writes in place,
    /// through [`PoolRef`] handles, a field at a time.
    pub(crate) pools: [core::cell::UnsafeCell<swoole_rs_process::process_pool::PoolRecord>; 2],
    pub(crate) manager_barrier: swoole_rs_locks::Barrier,
}

impl ServerShared {
    /// The live server's record, which `swr_v1_server_new` maps for its life.
    pub(crate) fn of(serv: crate::view::Server<'_>) -> &Self {
        let gs = serv.config().gs.as_ptr().cast::<Self>();
        // SAFETY: as above; its writers keep to the server's own synchronization.
        unsafe { &*gs }
    }
    /// `serv->get_task_worker_pool()`.
    pub(crate) fn task_pool(&self) -> PoolRef<'_> {
        let [task, _] = &self.pools;
        PoolRef::from(task)
    }
    /// `serv->get_event_worker_pool()`.
    pub(crate) fn event_pool(&self) -> PoolRef<'_> {
        let [_, event] = &self.pools;
        PoolRef::from(event)
    }
}

/// The server's shared counters (`serv->gs`), once configured.
pub(crate) fn server_counters(state: &crate::ServerState) -> Option<&ServerCounters> {
    let gs = state.config_ref().gs.as_ptr().cast::<ServerCounters>();
    // SAFETY: a non-null `gs` is the server's shared counters, which stay mapped.
    unsafe { gs.as_ref() }
}

/// The counters of the port a session arrived on (`get_port_by_session_id(id)->gs`).
fn session_port_counters(state: &crate::ServerState, session_id: i64) -> Option<&PortCounters> {
    // Kind 1 is `get_connection_by_session_id`, kind 0 `get_connection(server_fd)`.
    let server_fd = state.connection(session_id, Lookup::BySession)?.server_fd();
    state.port_by_server_fd(server_fd)?.config().counters()
}

/// `Server::send`'s counts: the server's and the session's port's responses and bytes,
/// and the current worker's responses.
pub(crate) fn count_send(state: &crate::ServerState, session_id: i64, len: u32) {
    use core::sync::atomic::Ordering::SeqCst;
    if let Some(gs) = server_counters(state) {
        gs.response_count.fetch_add(1, SeqCst);
        gs.total_send_bytes.fetch_add(len.into(), SeqCst);
    }
    if let Some(gs) = session_port_counters(state, session_id) {
        gs.response_count.fetch_add(1, SeqCst);
        gs.total_send_bytes.fetch_add(len.into(), SeqCst);
    }
    swoole_rs_process::worker_globals::with_current(|worker| worker.add_response_count());
}

/// Initializes a new shared allocation's server counter prefix, and returns it.
#[must_use]
pub fn swr_v1_server_shared_init(
    counters: &'static mut core::mem::MaybeUninit<ServerCounters>,
) -> &'static ServerCounters {
    let value = ServerCounters::default();
    value
        .pipe_packet_msg_id
        .store(1, core::sync::atomic::Ordering::Relaxed);
    // The limit is configuration, initialized before publication to any worker.
    let value = ServerCounters {
        max_concurrency: AtomicU32::new(u32::MAX),
        ..value
    };
    counters.write(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{free_state, swr_v1_server_state_new};
    use core::sync::atomic::Ordering;
    use swoole_rs_memory::members::Shm;

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no shared mappings")]
    fn a_send_counts_on_the_server_and_its_sessions_port() {
        use crate::port_config::swr_v1_port_config_new;
        use crate::records::{ConnectionRecord, SessionRecord};
        let mut gs = swoole_rs_memory::members::Shm::calloc(1, size_of::<ServerCounters>());
        let counters = gs.as_ptr().cast::<ServerCounters>();
        // SAFETY: a fresh mapping of a whole `ServerCounters`.
        unsafe { counters.write(ServerCounters::default()) };
        // SAFETY: as above; the mapping outlives every use below.
        let server = unsafe { &*counters };
        let mut port = PortCounters::default();
        // A `ListenPort` is its settings.
        let config = swr_v1_port_config_new(0);
        // SAFETY: the settings were just allocated.
        unsafe { (*config).gs.set((&raw mut port).cast()) };
        // SAFETY: a connection record is plain data; zero is a closed one.
        let mut connections: Vec<ConnectionRecord> =
            (0..3).map(|_| unsafe { core::mem::zeroed() }).collect();
        connections[1].object = config.cast();
        connections[2].server_fd = 1;
        let mut sessions: Vec<SessionRecord> = (0..3)
            .map(|id| SessionRecord {
                id,
                fd: 0,
                reactor_bits: 0,
            })
            .collect();
        sessions[1].fd = 2;
        let state = swr_v1_server_state_new();
        // SAFETY: a fresh state, its configuration and tables, all owned by this test.
        unsafe {
            (*(*state).config()).gs = core::mem::replace(&mut gs, Shm::empty());
            let tables = &mut *(*state).tables();
            tables.connection_list = crate::records::tests::naming(&mut connections);
            tables.session_list = crate::records::tests::naming(&mut sessions);
            tables.max_connection = 3;
            // The server's port, which the test still frees.
            let port = (*state).push_port(Box::from_raw(config));
            (*state).tables_ref().connection_at(1).unwrap().listen(port);
            count_send(&*state, 1, 4);
            count_send(&*state, 0, 3);
        }
        let totals =
            |r: &AtomicU64, b: &AtomicU64| (r.load(Ordering::Relaxed), b.load(Ordering::Relaxed));
        assert_eq!(
            totals(&server.response_count, &server.total_send_bytes),
            (2, 7)
        );
        assert_eq!(
            totals(&port.response_count, &port.total_send_bytes),
            (1, 4),
            "session 0 has no port"
        );
        // SAFETY: allocated above and not used again.
        unsafe {
            (*(*state).config()).gs.free();
            free_state(state);
            drop(Box::from_raw(config));
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no shared mappings")]
    fn shared_counter_updates_survive_a_worker_fork() {
        unsafe {
            let gs = swoole_rs_memory::members::Shm::calloc(1, 4096);
            let mapping = gs.as_ptr();
            assert!(!mapping.is_null());
            let counters = swr_v1_server_shared_init(&mut *mapping.cast());
            assert_eq!(counters.max_concurrency.load(Relaxed), u32::MAX);
            assert_eq!(counters.pipe_packet_msg_id.load(Ordering::Relaxed), 1);
            counters.concurrency.store(7, Ordering::Relaxed);
            let pid = libc::fork();
            assert!(pid >= 0);
            if pid == 0 {
                // Only lock-free atomics/plain loaned cells and _exit run after
                // fork; no Rust allocation, callback, destructor or test assertion.
                counters.concurrency.fetch_sub(2, Ordering::SeqCst);
                counters.min_fd.store(123, Relaxed);
                libc::_exit(0);
            }
            let mut status = 0;
            assert_eq!(libc::waitpid(pid, &raw mut status, 0), pid);
            assert_eq!(status, 0);
            assert_eq!(counters.concurrency.load(Relaxed), 5);
            assert_eq!(counters.min_fd(), 123);
            let mut gs = gs;
            gs.free();
        }
    }
}
