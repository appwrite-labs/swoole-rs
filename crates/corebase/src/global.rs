// SPDX-License-Identifier: Apache-2.0
//! Process-wide core state: the part of upstream's `SwooleG` Rust owns. Native
//! code reads and writes it only through these accessors. Native object handles
//! (the memory pool, the stdout stream) are opaque: native code allocates and
//! frees them, Rust only holds the pointer.
use std::ffi::{c_int, c_uint, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU16, Ordering};

unsafe extern "C" {
    fn srandom(seed: c_uint);
}

struct Global {
    init: AtomicBool,
    cpu_num: AtomicU16,
    wait_signal: AtomicBool,
    dns_lookup_random: AtomicBool,
    dns_tries: AtomicI32,
    memory_pool: AtomicPtr<c_void>,
    stdout: AtomicPtr<c_void>,
}

static GLOBAL: Global = Global {
    init: AtomicBool::new(false),
    cpu_num: AtomicU16::new(0),
    wait_signal: AtomicBool::new(false),
    dns_lookup_random: AtomicBool::new(false),
    dns_tries: AtomicI32::new(0),
    memory_pool: AtomicPtr::new(ptr::null_mut()),
    stdout: AtomicPtr::new(ptr::null_mut()),
};

/// Zero every field, as upstream's `SwooleG = {}` does.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_reset() {
    GLOBAL.init.store(false, Ordering::Relaxed);
    GLOBAL.cpu_num.store(0, Ordering::Relaxed);
    GLOBAL.wait_signal.store(false, Ordering::Relaxed);
    GLOBAL.dns_lookup_random.store(false, Ordering::Relaxed);
    GLOBAL.dns_tries.store(0, Ordering::Relaxed);
    GLOBAL.memory_pool.store(ptr::null_mut(), Ordering::Release);
    GLOBAL.stdout.store(ptr::null_mut(), Ordering::Release);
}

/// `swoole_init`'s Rust half: false when already initialised. Otherwise resets
/// the record, publishes the guard, then sets the host defaults upstream set
/// (CPU count, DNS tries, the `random()` seed).
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_init() -> bool {
    if GLOBAL.init.load(Ordering::Relaxed) {
        return false;
    }
    swr_v1_core_reset();
    GLOBAL.init.store(true, Ordering::Relaxed);
    // SAFETY: sysconf has no memory preconditions.
    let cpus = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    // Upstream stores SW_MAX(1, cpus) into a uint16_t.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream's uint16_t cpu_num of a positive count"
    )]
    GLOBAL.cpu_num.store(cpus.max(1) as u16, Ordering::Relaxed);
    GLOBAL.dns_tries.store(1, Ordering::Relaxed);
    // SAFETY: time accepts a null output pointer.
    let now = unsafe { libc::time(ptr::null_mut()) };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "srandom(time(nullptr)) narrows as upstream"
    )]
    let seed = now as c_uint;
    // SAFETY: srandom has no memory preconditions.
    unsafe { srandom(seed) };
    true
}

/// The descriptor limit upstream derives from `RLIMIT_NOFILE`: the soft limit,
/// truncated to `uint32_t` as upstream does, capped at `limit`. -1 (errno kept)
/// when getrlimit fails.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_max_sockets(limit: u32) -> i64 {
    let mut rlimit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `rlimit` is a live, writable local.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut rlimit) } < 0 {
        return -1;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's (uint32_t) rlmt.rlim_cur"
    )]
    let current = rlimit.rlim_cur as u32;
    i64::from(current.min(limit))
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_initialized() -> bool {
    GLOBAL.init.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_cpu_num() -> u16 {
    GLOBAL.cpu_num.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_wait_signal() -> bool {
    GLOBAL.wait_signal.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_set_wait_signal(on: bool) {
    GLOBAL.wait_signal.store(on, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_dns_lookup_random() -> bool {
    GLOBAL.dns_lookup_random.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_set_dns_lookup_random(on: bool) {
    GLOBAL.dns_lookup_random.store(on, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_dns_tries() -> c_int {
    GLOBAL.dns_tries.load(Ordering::Relaxed)
}

/// The native `swoole::MemoryPool *`, or null.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_memory_pool() -> *mut c_void {
    GLOBAL.memory_pool.load(Ordering::Acquire)
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_set_memory_pool(pool: *mut c_void) {
    GLOBAL.memory_pool.store(pool, Ordering::Release);
}

/// The native `FILE *` `sw_printf` writes to, or null.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_stdout() -> *mut c_void {
    GLOBAL.stdout.load(Ordering::Acquire)
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_set_stdout(stream: *mut c_void) {
    GLOBAL.stdout.store(stream, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_publishes_defaults_once_and_reset_clears_them() {
        swr_v1_core_reset();
        let pool = ptr::from_mut(&mut 7_u8).cast::<c_void>();
        swr_v1_core_set_memory_pool(pool);
        assert!(swr_v1_core_init());
        assert!(swr_v1_core_initialized());
        // Initialisation starts from a reset record.
        assert!(swr_v1_core_memory_pool().is_null());
        assert!(swr_v1_core_cpu_num() >= 1);
        assert_eq!(swr_v1_core_dns_tries(), 1);
        swr_v1_core_set_wait_signal(true);
        swr_v1_core_set_dns_lookup_random(true);
        swr_v1_core_set_memory_pool(pool);
        swr_v1_core_set_stdout(pool);
        assert!(!swr_v1_core_init(), "a second init is a no-op");
        assert!(swr_v1_core_wait_signal() && swr_v1_core_dns_lookup_random());
        assert_eq!(swr_v1_core_memory_pool(), pool);
        assert_eq!(swr_v1_core_stdout(), pool);
        swr_v1_core_reset();
        assert!(!swr_v1_core_initialized());
        assert_eq!(swr_v1_core_cpu_num(), 0);
        assert!(!swr_v1_core_wait_signal() && !swr_v1_core_dns_lookup_random());
        assert!(swr_v1_core_memory_pool().is_null() && swr_v1_core_stdout().is_null());
    }

    #[test]
    fn max_sockets_caps_the_soft_limit() {
        let one = swr_v1_core_max_sockets(1);
        assert_eq!(one, 1);
        assert!(swr_v1_core_max_sockets(u32::MAX) >= 1);
    }
}
