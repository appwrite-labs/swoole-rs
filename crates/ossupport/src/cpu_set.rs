//! A `cpu_set_t` and the affinity calls over it, as safe functions.
use std::ffi::{c_int, c_ulong};

/// The set's words: the kernel's CPU mask is an array of `unsigned long`.
const WORDS: usize = size_of::<libc::cpu_set_t>() / size_of::<c_ulong>();
const _: () = assert!(
    size_of::<CpuSet>() == size_of::<libc::cpu_set_t>()
        && align_of::<CpuSet>() == align_of::<libc::cpu_set_t>(),
    "a `cpu_set_t`'s size and alignment"
);

/// A `cpu_set_t`: the CPUs a process or thread may run on, CPU `n` being bit
/// `n % BITS` of word `n / BITS`.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct CpuSet([c_ulong; WORDS]);

impl CpuSet {
    const BYTES: usize = size_of::<Self>();
    /// `CPU_SETSIZE`.
    pub const SIZE: usize = 8 * Self::BYTES;

    /// No CPU.
    #[must_use]
    pub fn empty() -> Self {
        Self([0; WORDS])
    }

    /// `CPU_SET`; a `cpu` from `SIZE` on is outside the set and ignored.
    pub fn insert(&mut self, cpu: usize) {
        if let Some(word) = self.0.get_mut(cpu / c_ulong::BITS as usize) {
            *word |= 1 << (cpu % c_ulong::BITS as usize);
        }
    }

    /// `CPU_ISSET`; false from `SIZE` on.
    #[must_use]
    pub fn contains(&self, cpu: usize) -> bool {
        self.0
            .get(cpu / c_ulong::BITS as usize)
            .is_some_and(|word| word & (1 << (cpu % c_ulong::BITS as usize)) != 0)
    }

    /// `sched_getaffinity(pid)` into this set: 0, or -1 with `errno` set.
    pub fn get_process(&mut self, pid: libc::pid_t) -> c_int {
        // SAFETY: a writable set of the size passed.
        unsafe { libc::sched_getaffinity(pid, Self::BYTES, self.0.as_mut_ptr().cast()) }
    }

    /// `sched_setaffinity(pid)`: 0, or -1 with `errno` set.
    #[must_use]
    pub fn set_process(&self, pid: libc::pid_t) -> c_int {
        // SAFETY: a valid set of the size passed.
        unsafe { libc::sched_setaffinity(pid, Self::BYTES, self.0.as_ptr().cast()) }
    }

    /// `pthread_getaffinity_np(pthread_self())` into this set: 0 or an error number.
    pub fn get_thread(&mut self) -> c_int {
        let thread = crate::pthread_self();
        // SAFETY: a writable set of the size passed, for the calling thread.
        unsafe { libc::pthread_getaffinity_np(thread, Self::BYTES, self.0.as_mut_ptr().cast()) }
    }

    /// `pthread_setaffinity_np(pthread_self())`: 0 or an error number.
    #[must_use]
    pub fn set_thread(&self) -> c_int {
        let thread = crate::pthread_self();
        // SAFETY: a valid set of the size passed, for the calling thread.
        unsafe { libc::pthread_setaffinity_np(thread, Self::BYTES, self.0.as_ptr().cast()) }
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
