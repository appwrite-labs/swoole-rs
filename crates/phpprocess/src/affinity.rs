//! `Process::setAffinity()`/`getAffinity()` and the cpu-set conversions
//! `Thread` shares (`HAVE_CPU_AFFINITY`: Linux here).
use crate::report::{fatal, sys};
use swoole_rs_zend::{Array, Call, E_WARNING, Owned, OwnedArray, bailout, message};

fn cpu_num() -> usize {
    usize::from(swoole_rs_corebase::swr_v1_core_cpu_num())
}

/// A `cpu_set_t`.
pub struct CpuSet(libc::cpu_set_t);

impl CpuSet {
    /// No CPU.
    #[must_use]
    pub fn empty() -> Self {
        // SAFETY: an all-zero cpu set is empty.
        Self(unsafe { std::mem::zeroed() })
    }

    /// `php_swoole_array_to_cpu_set`: `None` (after a warning for a bad id)
    /// when the array is empty, too long or names a CPU out of range.
    #[must_use]
    pub fn from_array(array: &Array) -> Option<Self> {
        let set = Self::convert(array);
        bailout::finish();
        set
    }

    fn convert(array: &Array) -> Option<Self> {
        let count = array.count() as usize;
        if count == 0 {
            return None;
        }
        let limit = cpu_num();
        if count > limit {
            fatal(E_WARNING, &message!("More than the number of CPU"));
            return None;
        }
        let mut set = Self::empty();
        for (_, value) in array.entries() {
            let cpu = value.long();
            let index = usize::try_from(cpu).unwrap_or(usize::MAX);
            if index >= limit || index >= libc::CPU_SETSIZE as usize {
                fatal(E_WARNING, &message!("invalid cpu id [{cpu}]"));
                return None;
            }
            // SAFETY: a valid set and an index below CPU_SETSIZE.
            unsafe { libc::CPU_SET(index, &mut set.0) };
        }
        Some(set)
    }

    /// `php_swoole_cpu_set_to_array`.
    #[must_use]
    pub fn to_array(&self) -> Owned {
        let mut array = OwnedArray::default();
        let count = cpu_num().min(libc::CPU_SETSIZE as usize);
        for cpu in 0..count {
            // SAFETY: a valid set and an index below CPU_SETSIZE.
            if unsafe { libc::CPU_ISSET(cpu, &self.0) } {
                array.push_long(i64::try_from(cpu).unwrap_or(i64::MAX));
            }
        }
        array.into_owned()
    }

    #[must_use]
    pub fn as_ptr(&self) -> *const libc::cpu_set_t {
        &raw const self.0
    }

    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut libc::cpu_set_t {
        &raw mut self.0
    }
}

fn set_affinity(call: &Call) {
    let Ok(array) = call.arity(1, 1).and_then(|()| call.array_ref(1)) else {
        return;
    };
    let Some(set) = CpuSet::from_array(array) else {
        call.set_bool(false);
        return;
    };
    let pid = swoole_rs_process::os::getpid();
    // SAFETY: `set` is a valid cpu set.
    if unsafe { libc::sched_setaffinity(pid, size_of::<libc::cpu_set_t>(), set.as_ptr()) } < 0 {
        sys(E_WARNING, &message!("sched_setaffinity() failed"));
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

fn get_affinity(call: &Call) {
    let mut set = CpuSet::empty();
    let pid = swoole_rs_process::os::getpid();
    // SAFETY: `set` is writable.
    if unsafe { libc::sched_getaffinity(pid, size_of::<libc::cpu_set_t>(), set.as_mut_ptr()) } < 0 {
        sys(E_WARNING, &message!("sched_getaffinity() failed"));
        call.set_bool(false);
        return;
    }
    call.set_owned(set.to_array());
}

swoole_rs_zend::handlers! {
    zim_swoole_process_setAffinity => set_affinity;
    zim_swoole_process_getAffinity => get_affinity;
}
