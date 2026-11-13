//! `Worker_set_cpu_affinity`: a worker's CPU pinning when `open_cpu_affinity` is set.
//! The process's allowed CPUs, filtered by `cpu_affinity_ignore`'s remainder, the
//! `worker_id % count`-th of them, and the pin itself (the calling thread in thread
//! mode, the process otherwise). `crates/server/src/event_worker/native.rs` logs the warnings.

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
    use swoole_rs_ossupport::CpuSet;
    let failed = |status| AffinityOutcome {
        status,
        error: std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
    };
    let mut current = CpuSet::empty();
    let pid = crate::os::getpid();
    if current.get_process(pid) < 0 {
        return failed(1);
    }
    let allowed = |cpu: usize| {
        current.contains(cpu)
            && available.is_none_or(|cpus| cpus.iter().any(|&c| usize::try_from(c) == Ok(cpu)))
    };
    let Some(cpu) = select(allowed, worker_id, CpuSet::SIZE) else {
        return AffinityOutcome {
            status: 2,
            error: 0,
        };
    };
    let mut selected = CpuSet::empty();
    selected.insert(cpu);
    let error = if thread {
        selected.set_thread()
    } else {
        if selected.set_process(pid) < 0 {
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
