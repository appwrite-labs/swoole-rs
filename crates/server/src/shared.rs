//! Rust-owned shared server and port counters in the existing shared-memory arena.
use crate::records::Lookup;
use core::ffi::c_int;
use core::sync::atomic::{
    AtomicBool, AtomicI32, AtomicI64, AtomicU32, AtomicU64, Ordering::Relaxed,
};
use swoole_rs_memory::members::Shm;
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
/// `connection_nums`' cells, or null when there are none.
fn cells(nums: &Shm) -> *mut AtomicU32 {
    nums.as_ptr().cast()
}

impl PortCounters {
    /// The per-worker connection counts, or null.
    pub(crate) fn cells(&self) -> *mut AtomicU32 {
        cells(&self.connection_nums)
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

    /// The per-worker connection counts, or null.
    pub(crate) fn cells(&self) -> *mut AtomicU32 {
        cells(&self.connection_nums)
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
    pub(crate) pools: [swoole_rs_process::process_pool::PoolRecord; 2],
    pub(crate) manager_barrier: swoole_rs_locks::Barrier,
}

#[cfg_attr(
    not(all(feature = "upstream-symbols", not(test))),
    expect(dead_code, reason = "only the native ops read the pools")
)]
impl ServerShared {
    /// The live server's record, which `swr_v1_server_new` maps for its life.
    pub(crate) fn of(serv: crate::view::Server<'_>) -> &Self {
        let gs = serv.config().gs.as_ptr().cast::<Self>();
        // SAFETY: as above; its writers keep to the server's own synchronization.
        unsafe { &*gs }
    }
    /// `serv->get_task_worker_pool()`.
    pub(crate) fn task_pool(&self) -> &swoole_rs_process::process_pool::PoolRecord {
        &self.pools[0]
    }
    /// `serv->get_event_worker_pool()`.
    pub(crate) fn event_pool(&self) -> &swoole_rs_process::process_pool::PoolRecord {
        &self.pools[1]
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
///
/// # Safety
/// `storage` is fresh writable storage with `ServerCounters` size/alignment and no
/// readers, mapped for as long as the returned counters are used.
#[must_use]
pub unsafe fn swr_v1_server_shared_init(
    storage: *mut core::ffi::c_void,
) -> &'static ServerCounters {
    // SAFETY: per the caller, fresh storage of the right size and alignment that
    // nothing else reads yet, mapped for as long as the counters are used.
    let counters = unsafe { &mut *storage.cast::<core::mem::MaybeUninit<ServerCounters>>() };
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

/// Reads and updates the shared record ([`server_counters`]), ending every borrow before
/// host callbacks.
#[inline(always)]
pub(crate) fn native_fact(state: &crate::ServerState, op: u32, arg: u64) -> Option<u64> {
    use crate::ops::{
        OP_IS_STARTED, OP_LC_CLAIM_START, OP_LC_START_TIME, OP_MANAGER_PID, OP_MAXFD, OP_MINFD,
        OP_SESSION_ROUND, OP_SET_MINFD, OP_ST_GS_CONCURRENCY, OP_ST_GS_CONCURRENCY_SUB,
        OP_ST_GS_CONCURRENCY_ZERO,
    };
    use core::sync::atomic::Ordering;
    if !matches!(
        op,
        OP_IS_STARTED
            | OP_LC_CLAIM_START
            | OP_LC_START_TIME
            | OP_MANAGER_PID
            | OP_MAXFD
            | OP_MINFD
            | OP_SET_MINFD
            | OP_SESSION_ROUND
            | OP_ST_GS_CONCURRENCY_SUB
            | OP_ST_GS_CONCURRENCY
            | OP_ST_GS_CONCURRENCY_ZERO
    ) {
        return None;
    }
    // The fields each op reads follow the native owner's lifetime and synchronization.
    let gs = server_counters(state)?;
    Some(match op {
        OP_IS_STARTED => u64::from(gs.start.load(Ordering::Relaxed)),
        // One server starts.
        OP_LC_CLAIM_START => u64::from(
            gs.start
                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok(),
        ),
        OP_LC_START_TIME => {
            let now = swoole_rs_corebase::time_now();
            gs.start_time.store(now, Relaxed);
            0
        }
        OP_MANAGER_PID => i64::from(gs.manager_pid.load(Relaxed)).cast_unsigned(),
        // Native server code serializes fd-bound updates with its existing lock.
        OP_MAXFD => i64::from(gs.max_fd()).cast_unsigned(),
        OP_MINFD => i64::from(gs.min_fd()).cast_unsigned(),
        OP_SET_MINFD => {
            let value = core::ffi::c_int::from_ne_bytes(
                u32::try_from(arg & u64::from(u32::MAX))
                    .unwrap_or_default()
                    .to_ne_bytes(),
            );
            gs.min_fd.store(value, Relaxed);
            0
        }
        // Callers read the session round under the shared spin lock.
        OP_SESSION_ROUND => gs.session_round.load(Relaxed).cast_unsigned(),
        OP_ST_GS_CONCURRENCY_SUB => {
            let sub = u32::try_from(arg & u64::from(u32::MAX)).unwrap_or_default();
            gs.concurrency.fetch_sub(sub, Ordering::SeqCst);
            0
        }
        OP_ST_GS_CONCURRENCY => u64::from(gs.concurrency.load(Ordering::Relaxed)),
        OP_ST_GS_CONCURRENCY_ZERO => {
            gs.concurrency.store(0, Ordering::Relaxed);
            0
        }
        _ => return None,
    })
}

/// Initializes the native public default-construction view of a port counter record.
///
/// # Safety
/// `storage` is fresh writable storage of the record's size and alignment, without readers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_port_shared_init(storage: *mut core::ffi::c_void) {
    // SAFETY: the native public view lends fresh storage exclusively for initialization.
    unsafe {
        storage
            .cast::<PortCounters>()
            .write(PortCounters::default());
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{OP_MINFD, OP_SET_MINFD, OP_ST_GS_CONCURRENCY, OP_ST_GS_CONCURRENCY_SUB};
    use crate::state::{free_state, swr_v1_server_state_new};
    use core::sync::atomic::Ordering;
    use swoole_rs_memory::members::Shm;

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no shared mappings")]
    fn a_send_counts_on_the_server_and_its_sessions_port() {
        use crate::port_config::{swr_v1_port_config_free, swr_v1_port_config_new};
        use crate::records::{ConnectionRecord, ServerTables, SessionRecord};
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
            (*(*state).config()).gs = core::mem::replace(&mut gs, Shm::NULL);
            *(*state).tables() = ServerTables {
                connection_list: crate::records::tests::naming(&mut connections),
                session_list: crate::records::tests::naming(&mut sessions),
                max_connection: 3,
                ..ServerTables::default()
            };
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
            swr_v1_port_config_free(config);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no shared mappings")]
    fn shared_counter_updates_survive_a_worker_fork() {
        unsafe {
            let gs = swoole_rs_memory::members::Shm::calloc(1, 4096);
            let mapping = gs.as_ptr();
            assert!(!mapping.is_null());
            let counters = swr_v1_server_shared_init(mapping);
            assert_eq!(counters.max_concurrency.load(Relaxed), u32::MAX);
            assert_eq!(counters.pipe_packet_msg_id.load(Ordering::Relaxed), 1);
            counters.concurrency.store(7, Ordering::Relaxed);
            let state = swr_v1_server_state_new();
            (*(*state).config()).gs = gs;
            let pid = libc::fork();
            assert!(pid >= 0);
            if pid == 0 {
                // Only lock-free atomics/plain loaned cells and _exit run after
                // fork; no Rust allocation, callback, destructor or test assertion.
                native_fact(&*state, OP_ST_GS_CONCURRENCY_SUB, 2);
                native_fact(&*state, OP_SET_MINFD, 123);
                libc::_exit(0);
            }
            let mut status = 0;
            assert_eq!(libc::waitpid(pid, &raw mut status, 0), pid);
            assert_eq!(status, 0);
            assert_eq!(native_fact(&*state, OP_ST_GS_CONCURRENCY, 0), Some(5));
            assert_eq!(native_fact(&*state, OP_MINFD, 0), Some(123));
            (*(*state).config()).gs.free();
            free_state(state);
        }
    }
}
