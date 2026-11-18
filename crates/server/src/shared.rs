//! Rust-owned shared server and port counters in the existing shared-memory arena.
use crate::records::Lookup;
use core::ffi::c_int;
use core::sync::atomic::{
    AtomicBool, AtomicI32, AtomicI64, AtomicU32, AtomicU64, Ordering::Relaxed,
};
use swoole_rs_corebase::process::process_pool::{PoolObjectKey, PoolRef};
#[derive(Default)]
pub struct PortCounters {
    pub connection_num: AtomicU32,
    pub abort_count: AtomicU64,
    pub accept_count: AtomicU64,
    pub close_count: AtomicU64,
    pub dispatch_count: AtomicU64,
    pub request_count: AtomicU64,
    pub response_count: AtomicU64,
    pub total_recv_bytes: AtomicU64,
    pub total_send_bytes: AtomicU64,
}
/// Descriptor range for scans of the corresponding connection table.
#[derive(Default)]
pub struct FdBounds {
    pub(crate) max_fd: AtomicI32,
    pub(crate) min_fd: AtomicI32,
}

impl FdBounds {
    #[must_use]
    pub fn max_fd(&self) -> c_int {
        self.max_fd.load(Relaxed)
    }

    #[must_use]
    pub fn min_fd(&self) -> c_int {
        self.min_fd.load(Relaxed)
    }
}

#[derive(Default)]
pub struct ServerCounters {
    /// `pid_t`s, set by the master and the manager and read by every process.
    pub master_pid: AtomicI32,
    pub manager_pid: AtomicI32,
    /// `long`, read under the shared spin lock.
    pub session_round: AtomicI64,
    pub start: AtomicBool,
    pub shutdown: AtomicBool,
    /// The connection fd range; written under the server lock, read by every thread.
    pub(crate) bounds: FdBounds,
    pub onstart_called: AtomicBool,
    /// `time_t`.
    pub start_time: AtomicI64,
    pub connection_num: AtomicU32,
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
/// task and event worker pools, and the manager's barrier.
pub(crate) struct ServerShared {
    pub(crate) counters: ServerCounters,
    /// The task pool, then the event pool: records every process writes in place,
    /// through [`PoolRef`] handles, a field at a time.
    pub(crate) pools:
        [core::cell::UnsafeCell<swoole_rs_corebase::process::process_pool::PoolRecord>; 2],
    pub(crate) manager_barrier: swoole_rs_corebase::locks::Barrier,
}

impl ServerShared {
    /// The live server's record, which `swr_v1_server_new` maps for its life.
    #[inline]
    pub(crate) fn of(serv: crate::view::Server<'_>) -> &Self {
        Self::of_state(serv.state())
            .unwrap_or_else(|| swoole_rs_corebase::fatal("a server without its shared record"))
    }

    /// The record `state`'s server maps (`serv->gs`), once it has one.
    #[inline]
    fn of_state(state: &crate::ServerState) -> Option<&Self> {
        state.config_ref().gs.records().first()
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

/// The key a server's own pools hold it under: private, so only [`hold_server`] and
/// [`workers_start`] store an object under it, each a server in its own pools.
static SERVER: PoolObjectKey = PoolObjectKey::new();

/// `pool->ptr = serv` for both of `serv`'s pools, which [`pool_server`] reads back.
/// The pools live in the record `serv` maps for its life and frees before itself.
#[cfg_attr(
    not(feature = "upstream-symbols"),
    expect(
        dead_code,
        reason = "only the hybrid's pools run the hooks that read it"
    )
)]
pub(crate) fn hold_server(serv: crate::view::Server<'_>) {
    let shared = ServerShared::of(serv);
    for pool in [shared.task_pool(), shared.event_pool()] {
        pool.hold(&SERVER, serv.raw());
    }
}

/// Startup's `LC_PREPARE_WORKERS`: publish the event pool, which holds `serv` as
/// [`hold_server`] does, and number the workers.
pub(crate) fn workers_start(serv: crate::view::Server<'_>) {
    let users = (0..).map_while(|index| serv.pending_worker(index));
    // `serv.workers()` lends `serv`'s own event pool.
    swoole_rs_corebase::process::server_workers::swr_v1_process_server_workers_start(
        &serv.workers(),
        (&SERVER, serv.raw()),
        users,
    );
}

/// The server `pool` belongs to, when it is one of a server's own pools that holds it
/// ([`hold_server`]); `None` for any other pool.
#[cfg_attr(
    not(feature = "upstream-symbols"),
    expect(
        dead_code,
        reason = "only the hybrid's pools run the hooks that read it"
    )
)]
#[inline]
pub(crate) fn pool_server(pool: PoolRef<'_>) -> Option<crate::view::Server<'_>> {
    let serv = pool.object_of(&SERVER)?;
    // SAFETY: only `hold_server` and `workers_start` hold an object under the private
    // `SERVER` key, each a server in its own pools. Those live in the shared record the
    // server maps when it is made and frees in its teardown, which runs no pool hook and
    // reads no pool's server: a pool live for `'_` holds a live server.
    Some(unsafe { crate::view::Server::from_raw(serv) })
}

/// The server's shared counters (`serv->gs`), once configured.
pub(crate) fn server_counters(state: &crate::ServerState) -> Option<&ServerCounters> {
    ServerShared::of_state(state).map(|shared| &shared.counters)
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
    count_sent(state, session_port_counters(state, session_id), len);
}

/// [`count_send`] with the session's port's counters, `port`, already found.
pub(crate) fn count_sent(state: &crate::ServerState, port: Option<&PortCounters>, len: u32) {
    use core::sync::atomic::Ordering::SeqCst;
    if let Some(gs) = server_counters(state) {
        gs.response_count.fetch_add(1, SeqCst);
        gs.total_send_bytes.fetch_add(len.into(), SeqCst);
    }
    if let Some(gs) = port {
        gs.response_count.fetch_add(1, SeqCst);
        gs.total_send_bytes.fetch_add(len.into(), SeqCst);
    }
    swoole_rs_corebase::process::worker_globals::with_current(|worker| worker.add_response_count());
}

/// Initializes a new shared allocation's zeroed server counters, and returns them.
pub fn swr_v1_server_shared_init(counters: &ServerCounters) -> &ServerCounters {
    counters
        .pipe_packet_msg_id
        .store(1, core::sync::atomic::Ordering::Relaxed);
    // The limit is configuration, initialized before publication to any worker.
    counters.max_concurrency.store(u32::MAX, Relaxed);
    counters
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestServer;
    use core::sync::atomic::Ordering;

    #[test]
    fn only_base_servers_keep_their_connection_bounds_private() {
        let mut live = crate::fixture::LiveServer::new();
        for mode in [crate::Mode::Base, crate::Mode::Process, crate::Mode::Thread] {
            live.config_mut().mode_ = mode;
            let server = live.server();
            let fd = mode as c_int;
            server.fd_bounds().max_fd.store(fd, Relaxed);
            assert_eq!(server.fd_bounds().max_fd(), fd);
            assert_eq!(
                server.counters().bounds.max_fd(),
                if mode == crate::Mode::Base { 0 } else { fd }
            );
        }
    }

    #[test]
    fn a_send_counts_on_the_server_and_its_sessions_port() {
        use crate::records::{ConnectionRecord, SessionRecord};
        // A `ListenPort` is its settings.
        let config = crate::port_config::PortConfig::boxed(0);
        config.gs.fill(crate::table::Table::calloc(1));
        // SAFETY: a connection record is plain data; zero is a closed one.
        let mut connections: Vec<ConnectionRecord> =
            (0..3).map(|_| unsafe { core::mem::zeroed() }).collect();
        connections[2].server_fd.set(1);
        let mut sessions: Vec<SessionRecord> = (0..3)
            .map(|id| SessionRecord {
                id: core::cell::Cell::new(id),
                fd: core::cell::Cell::new(0),
                reactor_bits: core::cell::Cell::new(0),
            })
            .collect();
        sessions[1].fd.set(2);
        let mut server = TestServer::new();
        server.config_mut().gs = crate::table::Table::calloc(1);
        let tables = server.tables_mut();
        *tables.connection_list_mut() = crate::records::tests::naming(&mut connections);
        *tables.session_list_mut() = crate::records::tests::naming(&mut sessions);
        let state = server.state();
        // SAFETY: the tables just named hold 3 records.
        unsafe { state.tables_ref().set_max_connection_unchecked(3) };
        let port = state.ports.push(config);
        state.hold_port(0, 1).unwrap();
        count_send(state, 1, 4);
        count_send(state, 0, 3);
        let counted = &state.config_ref().gs.records()[0].counters;
        let totals =
            |r: &AtomicU64, b: &AtomicU64| (r.load(Ordering::Relaxed), b.load(Ordering::Relaxed));
        assert_eq!(
            totals(&counted.response_count, &counted.total_send_bytes),
            (2, 7)
        );
        assert_eq!(
            totals(
                &port.config().counters().unwrap().response_count,
                &port.config().counters().unwrap().total_send_bytes
            ),
            (1, 4),
            "session 0 has no port"
        );
        drop(server);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no shared mappings")]
    fn shared_counter_updates_survive_a_worker_fork() {
        unsafe {
            let gs = crate::table::Table::<ServerShared>::calloc(1);
            let mapping = gs.as_ptr();
            assert!(!mapping.is_null());
            let counters = swr_v1_server_shared_init(&(*mapping).counters);
            assert_eq!(counters.max_concurrency.load(Relaxed), u32::MAX);
            assert_eq!(counters.pipe_packet_msg_id.load(Ordering::Relaxed), 1);
            counters.concurrency.store(7, Ordering::Relaxed);
            let pid = libc::fork();
            assert!(pid >= 0);
            if pid == 0 {
                // Only lock-free atomics/plain loaned cells and _exit run after
                // fork; no Rust allocation, callback, destructor or test assertion.
                counters.concurrency.fetch_sub(2, Ordering::SeqCst);
                counters.bounds.min_fd.store(123, Relaxed);
                libc::_exit(0);
            }
            let mut status = 0;
            assert_eq!(libc::waitpid(pid, &raw mut status, 0), pid);
            assert_eq!(status, 0);
            assert_eq!(counters.concurrency.load(Relaxed), 5);
            assert_eq!(counters.bounds.min_fd(), 123);
            let mut gs = gs;
            gs.free();
        }
    }
}
