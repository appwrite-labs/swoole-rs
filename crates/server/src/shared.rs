//! Rust-owned shared server and port counters in the existing shared-memory arena.
use core::sync::atomic::{AtomicU32, AtomicU64};
#[derive(Default)]
#[repr(C)]
pub struct PortCounters {
    pub connection_num: AtomicU32,
    pub connection_nums: *mut AtomicU32,
    pub abort_count: AtomicU64,
    pub accept_count: AtomicU64,
    pub close_count: AtomicU64,
    pub dispatch_count: AtomicU64,
    pub request_count: AtomicU64,
    pub response_count: AtomicU64,
    pub total_recv_bytes: AtomicU64,
    pub total_send_bytes: AtomicU64,
}
#[derive(Default)]
#[repr(C)]
pub struct ServerCounters {
    pub master_pid: libc::pid_t,
    pub manager_pid: libc::pid_t,
    pub session_round: libc::c_long,
    pub start: AtomicU32,
    pub shutdown: AtomicU32,
    pub max_fd: core::ffi::c_int,
    pub min_fd: core::ffi::c_int,
    pub onstart_called: bool,
    pub start_time: libc::time_t,
    pub connection_num: AtomicU32,
    pub connection_nums: *mut AtomicU32,
    pub tasking_num: AtomicU32,
    pub max_concurrency: u32,
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

/// Initializes a new shared allocation's server counter prefix.
///
/// # Safety
/// `counters` is fresh writable storage with `ServerCounters` size/alignment and no readers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_shared_init(storage: *mut core::ffi::c_void) {
    let counters = storage.cast::<ServerCounters>();
    let value = ServerCounters::default();
    value
        .pipe_packet_msg_id
        .store(1, core::sync::atomic::Ordering::Relaxed);
    // The limit is configuration, initialized before publication to any worker.
    let value = ServerCounters {
        max_concurrency: u32::MAX,
        ..value
    };
    // SAFETY: initialization owns this new shared prefix exclusively.
    unsafe { counters.write(value) };
}

/// Reads and updates the shared record, ending every borrow before host callbacks.
///
/// # Safety
/// The state is live and native callers keep the shared allocation mapped for this operation.
pub(crate) unsafe fn native_fact(
    state: *const crate::ServerState,
    op: u32,
    arg: u64,
) -> Option<u64> {
    use crate::ops::{
        OP_IS_STARTED, OP_MANAGER_PID, OP_MASTER_PID, OP_MAXFD, OP_MINFD, OP_SESSION_ROUND,
        OP_SET_MAXFD, OP_SET_MINFD, OP_ST_GS_CONCURRENCY, OP_ST_GS_CONCURRENCY_SUB,
        OP_ST_GS_CONCURRENCY_ZERO,
    };
    use core::sync::atomic::Ordering;
    if !matches!(
        op,
        OP_IS_STARTED
            | OP_MASTER_PID
            | OP_MANAGER_PID
            | OP_MAXFD
            | OP_MINFD
            | OP_SET_MAXFD
            | OP_SET_MINFD
            | OP_SESSION_ROUND
            | OP_ST_GS_CONCURRENCY_SUB
            | OP_ST_GS_CONCURRENCY
            | OP_ST_GS_CONCURRENCY_ZERO
    ) {
        return None;
    }
    // SAFETY: the state owns a stable configuration allocation for this call.
    let config = unsafe { &*state }.config();
    if config.is_null() {
        return None;
    }
    // SAFETY: the config pointer is live; its shared record remains mapped for this call.
    let gs = unsafe { (*config).gs }.cast::<ServerCounters>();
    if gs.is_null() {
        return None;
    }
    Some(match op {
        // SAFETY: each leaf field read uses the native owner's lifetime and synchronization.
        OP_IS_STARTED => u64::from(unsafe { (*gs).start.load(Ordering::Relaxed) }),
        // SAFETY: master pid is assigned before workers can read it.
        OP_MASTER_PID => i64::from(unsafe { (*gs).master_pid }).cast_unsigned(),
        // SAFETY: manager pid follows the original native lifecycle synchronization.
        OP_MANAGER_PID => i64::from(unsafe { (*gs).manager_pid }).cast_unsigned(),
        // SAFETY: native server code serializes fd-bound updates with its existing lock.
        OP_MAXFD => i64::from(unsafe { (*gs).max_fd }).cast_unsigned(),
        // SAFETY: native server code serializes fd-bound updates with its existing lock.
        OP_MINFD => i64::from(unsafe { (*gs).min_fd }).cast_unsigned(),
        OP_SET_MAXFD | OP_SET_MINFD => {
            let value = core::ffi::c_int::from_ne_bytes(
                u32::try_from(arg & u64::from(u32::MAX))
                    .unwrap_or_default()
                    .to_ne_bytes(),
            );
            if op == OP_SET_MAXFD {
                // SAFETY: caller owns the native server fd-bound lock.
                unsafe { (*gs).max_fd = value };
            } else {
                // SAFETY: caller owns the native server fd-bound lock.
                unsafe { (*gs).min_fd = value };
            }
            0
        }
        // SAFETY: callers read the session round under the shared spin lock.
        OP_SESSION_ROUND => unsafe { (*gs).session_round }.cast_unsigned(),
        OP_ST_GS_CONCURRENCY_SUB => {
            // SAFETY: concurrency is aligned atomic storage in the mapped shared prefix.
            unsafe {
                (*gs).concurrency.fetch_sub(
                    u32::try_from(arg & u64::from(u32::MAX)).unwrap_or_default(),
                    Ordering::SeqCst,
                )
            };
            0
        }
        // SAFETY: concurrency is aligned atomic storage in the shared prefix.
        OP_ST_GS_CONCURRENCY => u64::from(unsafe { (*gs).concurrency.load(Ordering::Relaxed) }),
        OP_ST_GS_CONCURRENCY_ZERO => {
            // SAFETY: same shared atomic cell; reset preserves the original store operation.
            unsafe { (*gs).concurrency.store(0, Ordering::Relaxed) };
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

/// Reads the active per-worker counts, or the single shared count in process mode.
///
/// # Safety
/// `counters` is a mapped live port record; its non-null `connection_nums` loan has
/// `workers` initialized aligned atomic cells for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_port_connections(
    storage: *const core::ffi::c_void,
    workers: u32,
) -> usize {
    let counters = storage.cast::<PortCounters>();
    // SAFETY: the caller lends the live port counter record.
    let cells = unsafe { (*counters).connection_nums };
    if cells.is_null() {
        // SAFETY: the record's aligned atomic count is live for the call.
        return unsafe {
            (*counters)
                .connection_num
                .load(core::sync::atomic::Ordering::Relaxed)
        } as usize;
    }
    let mut count = 0_usize;
    for index in 0..workers as usize {
        // SAFETY: index is below the caller's loaned atomic-cell count.
        let cell = unsafe { &*cells.wrapping_add(index) };
        count = count.wrapping_add(cell.load(core::sync::atomic::Ordering::Relaxed) as usize);
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{OP_MAXFD, OP_SET_MAXFD, OP_ST_GS_CONCURRENCY, OP_ST_GS_CONCURRENCY_SUB};
    use crate::state::{swr_v1_server_state_free, swr_v1_server_state_new};
    use core::sync::atomic::Ordering;

    #[test]
    fn shared_counter_updates_survive_a_worker_fork() {
        unsafe {
            let mapping = libc::mmap(
                core::ptr::null_mut(),
                4096,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_ANON,
                -1,
                0,
            );
            assert_ne!(mapping, libc::MAP_FAILED);
            swr_v1_server_shared_init(mapping);
            let counters = mapping.cast::<ServerCounters>();
            assert_eq!((*counters).max_concurrency, u32::MAX);
            assert_eq!((*counters).pipe_packet_msg_id.load(Ordering::Relaxed), 1);
            (*counters).concurrency.store(7, Ordering::Relaxed);
            let state = swr_v1_server_state_new();
            (*(*state).config()).gs = mapping;
            let pid = libc::fork();
            assert!(pid >= 0);
            if pid == 0 {
                // Only lock-free atomics/plain loaned cells and _exit run after
                // fork; no Rust allocation, callback, destructor or test assertion.
                native_fact(state, OP_ST_GS_CONCURRENCY_SUB, 2);
                native_fact(state, OP_SET_MAXFD, 123);
                libc::_exit(0);
            }
            let mut status = 0;
            assert_eq!(libc::waitpid(pid, &raw mut status, 0), pid);
            assert_eq!(status, 0);
            assert_eq!(native_fact(state, OP_ST_GS_CONCURRENCY, 0), Some(5));
            assert_eq!(native_fact(state, OP_MAXFD, 0), Some(123));
            swr_v1_server_state_free(state);
            assert_eq!(libc::munmap(mapping, 4096), 0);
        }
    }

    #[test]
    fn port_counts_use_the_shared_worker_cells_when_present() {
        let mut counters = PortCounters::default();
        counters.connection_num.store(99, Ordering::Relaxed);
        let storage = core::ptr::from_ref(&counters).cast();
        assert_eq!(unsafe { swr_v1_server_port_connections(storage, 2) }, 99);
        let mut cells = [AtomicU32::new(3), AtomicU32::new(5)];
        counters.connection_nums = cells.as_mut_ptr();
        let storage = core::ptr::from_ref(&counters).cast();
        assert_eq!(unsafe { swr_v1_server_port_connections(storage, 2) }, 8);
        assert_eq!(unsafe { swr_v1_server_port_connections(storage, 0) }, 0);
    }
}
