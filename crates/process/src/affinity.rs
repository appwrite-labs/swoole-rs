//! `Worker_set_cpu_affinity`: a worker's CPU pinning when `open_cpu_affinity` is set.
//! The process's allowed CPUs, filtered by `cpu_affinity_ignore`'s remainder, the
//! `worker_id % count`-th of them, and the pin itself (the calling thread in thread
//! mode, the process otherwise). `crates/server/src/event_worker/native.rs` logs the warnings.

/// Why pinning a worker failed, with the errno where there is one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AffinityError {
    /// Reading the process's affinity failed.
    Get(i32),
    /// No CPU left.
    NoCpu,
    /// Setting the affinity failed.
    Set(i32),
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
///
/// # Errors
/// The [`AffinityError`] that stopped the pinning.
#[cfg(target_os = "linux")]
pub fn worker_affinity(
    worker_id: u32,
    available: Option<&[i32]>,
    thread: bool,
) -> Result<(), AffinityError> {
    use swoole_rs_ossupport::CpuSet;
    let errno = || std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    let mut current = CpuSet::empty();
    let pid = crate::os::getpid();
    if !current.get_process(pid) {
        return Err(AffinityError::Get(errno()));
    }
    let allowed = |cpu: usize| {
        current.contains(cpu)
            && available.is_none_or(|cpus| cpus.iter().any(|&c| usize::try_from(c) == Ok(cpu)))
    };
    let Some(cpu) = select(allowed, worker_id, CpuSet::SIZE) else {
        return Err(AffinityError::NoCpu);
    };
    let mut selected = CpuSet::empty();
    selected.insert(cpu);
    if thread {
        selected.set_thread().map_err(AffinityError::Set)?;
    } else if !selected.set_process(pid) {
        return Err(AffinityError::Set(errno()));
    }
    Ok(())
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
        assert_eq!(super::worker_affinity(0, None, true), Ok(()));
    }
}
