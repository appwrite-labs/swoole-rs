//! `Worker_set_cpu_affinity`: a worker's CPU pinning when `open_cpu_affinity` is set.
//! The process's allowed CPUs, filtered by `cpu_affinity_ignore`'s remainder, the
//! `worker_id % count`-th of them, and the pin itself (the calling thread in thread
//! mode, the process otherwise). `crates/server/src/event_worker_ops.rs` logs the warnings.

/// What pinning a worker did.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AffinityOutcome {
    /// 0 pinned; 1 reading the process's affinity failed; 2 no CPU left; 3 setting failed.
    pub status: u32,
    /// The errno of status 1 and 3.
    pub error: i32,
}

/// The `worker_id % count`-th of the `count` allowed CPUs, or `None` if there are none.
#[cfg(any(target_os = "linux", test))]
fn select(allowed: impl Fn(usize) -> bool, worker_id: u32, set_size: usize) -> Option<usize> {
    let count = (0..set_size).filter(|&cpu| allowed(cpu)).count();
    let index = usize::try_from(worker_id).ok()?.checked_rem(count)?;
    (0..set_size).filter(|&cpu| allowed(cpu)).nth(index)
}

/// Pins the calling worker. `available` is `Server::cpu_affinity_available` (none for
/// every CPU); `thread` is `Server::is_thread_mode()`.
#[cfg(target_os = "linux")]
#[must_use]
pub fn worker_affinity(worker_id: u32, available: Option<&[i32]>, thread: bool) -> AffinityOutcome {
    use std::mem::{size_of, zeroed};
    let failed = |status| AffinityOutcome {
        status,
        error: std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
    };
    // SAFETY: cpu_set_t is a plain bit array; all zeroes is the empty set.
    let mut current: libc::cpu_set_t = unsafe { zeroed() };
    let pid = crate::os::getpid();
    // SAFETY: `current` is a valid cpu_set_t of the size passed.
    if unsafe { libc::sched_getaffinity(pid, size_of::<libc::cpu_set_t>(), &raw mut current) } < 0 {
        return failed(1);
    }
    let set_size = usize::try_from(libc::CPU_SETSIZE).unwrap_or(0);
    let allowed = |cpu: usize| {
        // SAFETY: `select` asks only for cpu < CPU_SETSIZE.
        let present = unsafe { libc::CPU_ISSET(cpu, &current) };
        present && available.is_none_or(|cpus| cpus.iter().any(|&c| usize::try_from(c) == Ok(cpu)))
    };
    let Some(cpu) = select(allowed, worker_id, set_size) else {
        return AffinityOutcome {
            status: 2,
            error: 0,
        };
    };
    // SAFETY: as above.
    let mut selected: libc::cpu_set_t = unsafe { zeroed() };
    // SAFETY: `select` returned cpu < CPU_SETSIZE.
    unsafe { libc::CPU_SET(cpu, &mut selected) };
    let size = size_of::<libc::cpu_set_t>();
    let error = if thread {
        // SAFETY: pthread_self has no preconditions.
        let me = unsafe { libc::pthread_self() };
        // SAFETY: `selected` is a valid cpu_set_t of the size passed.
        unsafe { libc::pthread_setaffinity_np(me, size, &raw const selected) }
    } else {
        // SAFETY: `selected` is a valid cpu_set_t of the size passed.
        let result = unsafe { libc::sched_setaffinity(pid, size, &raw const selected) };
        if result < 0 {
            return failed(3);
        }
        0
    };
    if error != 0 {
        return AffinityOutcome { status: 3, error };
    }
    AffinityOutcome::default()
}

#[cfg(test)]
mod tests {
    use super::select;

    #[test]
    fn picks_worker_id_modulo_allowed() {
        let odd = |cpu: usize| cpu % 2 == 1;
        assert_eq!(select(odd, 0, 8), Some(1));
        assert_eq!(select(odd, 2, 8), Some(5));
        assert_eq!(select(odd, 5, 8), Some(3));
        assert_eq!(select(|_| false, 3, 8), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no pthread_setaffinity_np")]
    fn pins_the_thread_to_an_allowed_cpu() {
        let outcome = super::worker_affinity(0, None, true);
        assert_eq!(outcome, super::AffinityOutcome::default());
    }
}
