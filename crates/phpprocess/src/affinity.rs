//! `Process::setAffinity()`/`getAffinity()` and the cpu-set conversions
//! `Thread` shares (`HAVE_CPU_AFFINITY`: Linux here).
use crate::report::{fatal, sys};
use std::ffi::c_void;
use swoole_rs_zend::{
    Call, E_WARNING, add_next_index_long, each_value, message,
    sys::{swr_zend_array_init, zval},
};

fn cpu_num() -> usize {
    usize::from(swoole_rs_corebase::swr_v1_core_cpu_num())
}

/// `php_swoole_array_to_cpu_set`: false (after a warning for a bad id) when
/// the array is empty, too long or names a CPU out of range.
/// # Safety
/// `array` is a live array zval and `set` a writable `cpu_set_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpprocess_array_to_cpu_set(
    array: *const zval,
    set: *mut c_void,
) -> bool {
    // SAFETY: per the caller.
    let valid = unsafe { array_to_cpu_set(array, set) };
    swoole_rs_zend::bailout::finish();
    valid
}

/// # Safety
/// As `swr_v1_phpprocess_array_to_cpu_set`.
unsafe fn array_to_cpu_set(array: *const zval, set: *mut c_void) -> bool {
    // SAFETY: per the caller.
    let value = unsafe { (*array).value };
    // SAFETY: an array zval holds its table.
    let table = unsafe { value.arr };
    // SAFETY: a live table.
    let count = unsafe { (*table).nNumOfElements } as usize;
    if count == 0 {
        return false;
    }
    let limit = cpu_num();
    if count > limit {
        fatal(E_WARNING, &message!("More than the number of CPU"));
        return false;
    }
    // SAFETY: per the caller, a writable cpu set.
    let set = unsafe { &mut *set.cast::<libc::cpu_set_t>() };
    // SAFETY: as above.
    unsafe { libc::CPU_ZERO(set) };
    let mut valid = true;
    let each = |value: *mut zval| {
        // SAFETY: a live element.
        let cpu = unsafe { swoole_rs_zend::value::get_long(value) };
        let index = usize::try_from(cpu).unwrap_or(usize::MAX);
        if index >= limit || index >= libc::CPU_SETSIZE as usize {
            fatal(E_WARNING, &message!("invalid cpu id [{cpu}]"));
            valid = false;
            return false;
        }
        // SAFETY: per the caller.
        unsafe { libc::CPU_SET(index, set) };
        true
    };
    // SAFETY: a live table the loop does not modify.
    unsafe { each_value(table, each) };
    valid
}

/// `php_swoole_cpu_set_to_array`.
/// # Safety
/// `array` is a writable zval and `set` a valid `cpu_set_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpprocess_cpu_set_to_array(array: *mut zval, set: *const c_void) {
    // SAFETY: per the caller.
    unsafe { swr_zend_array_init(array) };
    // SAFETY: per the caller.
    let set = unsafe { &*set.cast::<libc::cpu_set_t>() };
    let count = cpu_num().min(libc::CPU_SETSIZE as usize);
    for cpu in 0..count {
        // SAFETY: a valid set and an index below CPU_SETSIZE.
        if unsafe { libc::CPU_ISSET(cpu, set) } {
            // SAFETY: the array initialised above.
            unsafe { add_next_index_long(array, i64::try_from(cpu).unwrap_or(i64::MAX)) };
        }
    }
}

fn set_affinity(call: &Call) {
    let Ok(Some(_)) = call.arity(1, 1).and_then(|()| call.array_table(1)) else {
        return;
    };
    // SAFETY: an all-zero cpu set is empty.
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    let argument = call.zval(1);
    // SAFETY: the array argument and a local set.
    if !unsafe { swr_v1_phpprocess_array_to_cpu_set(argument, (&raw mut set).cast()) } {
        call.set_bool(false);
        return;
    }
    // SAFETY: getpid has no preconditions.
    let pid = unsafe { libc::getpid() };
    // SAFETY: `set` is a valid cpu set.
    if unsafe { libc::sched_setaffinity(pid, size_of::<libc::cpu_set_t>(), &raw const set) } < 0 {
        sys(E_WARNING, &message!("sched_setaffinity() failed"));
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

fn get_affinity(call: &Call) {
    // SAFETY: an all-zero cpu set is empty.
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    // SAFETY: getpid has no preconditions.
    let pid = unsafe { libc::getpid() };
    // SAFETY: `set` is writable.
    if unsafe { libc::sched_getaffinity(pid, size_of::<libc::cpu_set_t>(), &raw mut set) } < 0 {
        sys(E_WARNING, &message!("sched_getaffinity() failed"));
        call.set_bool(false);
        return;
    }
    // SAFETY: the return slot and a valid set.
    unsafe { swr_v1_phpprocess_cpu_set_to_array(call.return_value(), (&raw const set).cast()) };
}

swoole_rs_zend::handlers! {
    zim_swoole_process_setAffinity => set_affinity;
    zim_swoole_process_getAffinity => get_affinity;
}
