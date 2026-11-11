//! `Process::setAffinity()`/`getAffinity()` and the cpu-set conversions
//! `Thread` shares (`HAVE_CPU_AFFINITY`: Linux here).
use crate::report::{fatal, sys};
use swoole_rs_ossupport::CpuSet;
use swoole_rs_zend::{Array, Call, E_WARNING, Owned, OwnedArray, bailout, message};

fn cpu_num() -> usize {
    usize::from(swoole_rs_corebase::swr_v1_core_cpu_num())
}

/// `php_swoole_array_to_cpu_set`: `None` (after a warning for a bad id) when
/// the array is empty, too long or names a CPU out of range.
#[must_use]
pub fn cpu_set_from_array(array: &Array) -> Option<CpuSet> {
    let set = convert(array);
    bailout::finish();
    set
}

fn convert(array: &Array) -> Option<CpuSet> {
    let count = array.count() as usize;
    if count == 0 {
        return None;
    }
    let limit = cpu_num();
    if count > limit {
        fatal(E_WARNING, &message!("More than the number of CPU"));
        return None;
    }
    let mut set = CpuSet::empty();
    for (_, value) in array.entries() {
        let cpu = value.long();
        let index = usize::try_from(cpu).unwrap_or(usize::MAX);
        if index >= limit || index >= CpuSet::SIZE {
            fatal(E_WARNING, &message!("invalid cpu id [{cpu}]"));
            return None;
        }
        set.insert(index);
    }
    Some(set)
}

/// `php_swoole_cpu_set_to_array`.
#[must_use]
pub fn cpu_set_to_array(set: &CpuSet) -> Owned {
    let mut array = OwnedArray::default();
    let count = cpu_num();
    for cpu in set.cpus().take_while(|&cpu| cpu < count) {
        array.push_long(i64::try_from(cpu).unwrap_or(i64::MAX));
    }
    array.into_owned()
}

fn set_affinity(call: &Call) {
    let Ok(array) = call.arity(1, 1).and_then(|()| call.array_ref(1)) else {
        return;
    };
    let Some(set) = cpu_set_from_array(array) else {
        call.set_bool(false);
        return;
    };
    if set.set_process(swoole_rs_process::os::getpid()) < 0 {
        sys(E_WARNING, &message!("sched_setaffinity() failed"));
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

fn get_affinity(call: &Call) {
    let mut set = CpuSet::empty();
    if set.get_process(swoole_rs_process::os::getpid()) < 0 {
        sys(E_WARNING, &message!("sched_getaffinity() failed"));
        call.set_bool(false);
        return;
    }
    call.set_owned(cpu_set_to_array(&set));
}

swoole_rs_zend::handlers! {
    zim_swoole_process_setAffinity => set_affinity;
    zim_swoole_process_getAffinity => get_affinity;
}
