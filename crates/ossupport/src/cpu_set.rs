//! A `cpu_set_t` and the affinity calls over it, as safe functions.
use std::ffi::c_int;

/// A `cpu_set_t`: the CPUs a process or thread may run on.
#[derive(Clone, Copy)]
pub struct CpuSet(libc::cpu_set_t);

impl CpuSet {
    const BYTES: usize = size_of::<libc::cpu_set_t>();
    /// `CPU_SETSIZE`.
    pub const SIZE: usize = 8 * Self::BYTES;

    /// No CPU.
    #[must_use]
    pub fn empty() -> Self {
        // SAFETY: a plain bitset, for which zero is the empty set.
        Self(unsafe { std::mem::zeroed() })
    }

    /// `CPU_SET`; a `cpu` from `SIZE` on is outside the set and ignored.
    pub fn insert(&mut self, cpu: usize) {
        if cpu < Self::SIZE {
            // SAFETY: `cpu` is inside the set.
            unsafe { libc::CPU_SET(cpu, &mut self.0) };
        }
    }

    /// `CPU_ISSET`; false from `SIZE` on.
    #[must_use]
    pub fn contains(&self, cpu: usize) -> bool {
        // SAFETY: `cpu` is inside the set.
        cpu < Self::SIZE && unsafe { libc::CPU_ISSET(cpu, &self.0) }
    }

    /// `sched_getaffinity(pid)` into this set: 0, or -1 with `errno` set.
    pub fn get_process(&mut self, pid: libc::pid_t) -> c_int {
        // SAFETY: a writable set of the size passed.
        unsafe { libc::sched_getaffinity(pid, Self::BYTES, &raw mut self.0) }
    }

    /// `sched_setaffinity(pid)`: 0, or -1 with `errno` set.
    #[must_use]
    pub fn set_process(&self, pid: libc::pid_t) -> c_int {
        // SAFETY: a valid set of the size passed.
        unsafe { libc::sched_setaffinity(pid, Self::BYTES, &raw const self.0) }
    }

    /// `pthread_getaffinity_np(pthread_self())` into this set: 0 or an error number.
    pub fn get_thread(&mut self) -> c_int {
        // SAFETY: a writable set of the size passed, for the calling thread.
        unsafe { libc::pthread_getaffinity_np(crate::pthread_self(), Self::BYTES, &raw mut self.0) }
    }

    /// `pthread_setaffinity_np(pthread_self())`: 0 or an error number.
    #[must_use]
    pub fn set_thread(&self) -> c_int {
        // SAFETY: a valid set of the size passed, for the calling thread.
        unsafe {
            libc::pthread_setaffinity_np(crate::pthread_self(), Self::BYTES, &raw const self.0)
        }
    }

    /// The set's CPUs, ascending.
    pub fn cpus(&self) -> impl Iterator<Item = usize> + '_ {
        (0..Self::SIZE).filter(|&cpu| self.contains(cpu))
    }
}

#[cfg(test)]
mod tests {
    use super::CpuSet;

    #[test]
    fn membership() {
        let mut set = CpuSet::empty();
        assert_eq!(set.cpus().count(), 0);
        set.insert(1);
        set.insert(CpuSet::SIZE - 1);
        set.insert(CpuSet::SIZE);
        assert!(set.contains(1) && !set.contains(0) && !set.contains(CpuSet::SIZE));
        assert_eq!(set.cpus().collect::<Vec<_>>(), [1, CpuSet::SIZE - 1]);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no affinity calls")]
    fn the_calling_thread_and_process() {
        let mut thread = CpuSet::empty();
        assert_eq!(thread.get_thread(), 0);
        assert!(thread.cpus().count() > 0);
        assert_eq!(thread.set_thread(), 0);
        let mut process = CpuSet::empty();
        assert_eq!(process.get_process(0), 0);
        assert!(process.cpus().count() > 0);
    }
}
