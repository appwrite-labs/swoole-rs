//! `Swoole\Lock`: a process-shared mutex, rwlock or spinlock in Swoole's global
//! memory pool, over the locks crate. Like upstream, a lock outlives its object:
//! forked workers may still hold it.
use std::ffi::{c_int, c_void};
use swoole_rs_locks::{
    swr_v1_locks_align, swr_v1_locks_init, swr_v1_locks_lock, swr_v1_locks_size,
    swr_v1_locks_unlock,
};
use swoole_rs_memory::members::pool;
use swoole_rs_phpstubs::lock;
use swoole_rs_zend::{
    Call, Class, Minit, Object, method_table,
    sys::{
        ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, zend_class_entry, zend_object,
        zend_object_handlers,
    },
    unset_property_deny,
};

/// `swoole::Lock::Type`.
pub(crate) const RW_LOCK: i64 = 1;
pub(crate) const MUTEX: i64 = 3;
pub(crate) const SPIN_LOCK: i64 = 5;
/// `LOCK_EX` from `<sys/file.h>` (Linux and macOS).
pub(crate) const LOCK_EX: i64 = 2;
/// `alignof(std::max_align_t)`: what the pool's allocations guarantee.
const MAX_ALIGN: usize = 16;

/// Bit `1 << type` for each lock type the build has: upstream's `HAVE_RWLOCK`
/// (both platforms) and `HAVE_SPINLOCK` (Linux; macOS has no `pthread_spin_lock`).
pub(crate) const KINDS: u32 = 1 << MUTEX
    | 1 << RW_LOCK
    | if cfg!(target_os = "linux") {
        1 << SPIN_LOCK
    } else {
        0
    };
/// The timed-lock capabilities `swr_v1_locks_lock` takes: `HAVE_MUTEX_TIMEDLOCK`
/// (1) and `HAVE_RWLOCK_TIMEDRDLOCK` (2), which Linux has and macOS lacks.
pub(crate) const CAPABILITIES: u32 = if cfg!(target_os = "linux") { 3 } else { 0 };
/// `SW_ERROR_WRONG_OPERATION`.
const WRONG_OPERATION: c_int = 509;
static LOCK: Class = Class::new();

/// `struct LockObject { Lock *lock; zend_object std; }`: the lock's storage and
/// type, null until `__construct`.
#[repr(C)]
struct Held {
    storage: *mut c_void,
    kind: i32,
}

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class or a subclass of it;
    // zeroed `Held` is the unconstructed lock.
    unsafe { Object::<Held>::create(&LOCK, ce) }
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees an object this class created.
    unsafe { Object::<Held>::dtor(std) };
}

fn held(call: &Call) -> &'static mut Held {
    // SAFETY: Lock methods are non-static, so `$this` is one of our objects.
    unsafe { Object::<Held>::data(call.this()) }
}

/// The constructed lock, or Swoole's fatal error (which exits).
fn constructed(call: &Call) -> Option<&'static mut Held> {
    let held = held(call);
    if held.storage.is_null() {
        // SAFETY: Swoole's fatal error hook; the handler's frames own nothing.
        unsafe { swoole_rs_corebase::fatal_error(WRONG_OPERATION, c"must call constructor first") };
        return None;
    }
    Some(held)
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_lock___construct, &lock::arginfo_class_Swoole_Lock___construct, ZEND_ACC_PUBLIC;
        c"lock" => zim_swoole_lock_lock, &lock::arginfo_class_Swoole_Lock_lock, ZEND_ACC_PUBLIC;
        c"unlock" => zim_swoole_lock_unlock, &lock::arginfo_class_Swoole_Lock_unlock, ZEND_ACC_PUBLIC;
    ];
}

/// `php_swoole_lock_minit`: register the class and its constants.
/// # Safety
/// Called once, at MINIT, after Swoole's exception class.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_lock_minit(module_number: c_int) {
    // SAFETY: Zend's MINIT of this module, on this thread.
    let minit = unsafe { Minit::new(module_number) };
    minit.register(&LOCK, c"Swoole\\Lock", &METHODS.0, configure);
    let types = [
        (c"MUTEX", c"SWOOLE_MUTEX", MUTEX),
        (c"RWLOCK", c"SWOOLE_RWLOCK", RW_LOCK),
        (c"SPINLOCK", c"SWOOLE_SPINLOCK", SPIN_LOCK),
    ];
    let enabled = |kind: i64| KINDS & (1 << kind) != 0;
    for (name, _, kind) in types.iter().filter(|(_, _, kind)| enabled(*kind)) {
        minit.class_constant_long(&LOCK, name, *kind);
    }
    minit.property_long(&LOCK, c"errCode", 0, ZEND_ACC_PUBLIC);
    for (_, name, kind) in types.iter().filter(|(_, _, kind)| enabled(*kind)) {
        minit.long_constant(name, *kind);
    }
}

/// Not serializable, clone and unset denied, custom object.
fn configure(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    Object::<Held>::install(entry, handlers, create, free);
}

/// A shared lock of `kind` in the pool, as `new Mutex(true)` (and its `RWLock`
/// and `SpinLock` twins) makes one. Upstream throws a C++ exception, which
/// terminates the process, when the pool or the lock's init fails.
fn new_lock(kind: i32) -> *mut c_void {
    let size = swr_v1_locks_size(kind);
    if size == 0 || swr_v1_locks_align(kind) > MAX_ALIGN {
        std::process::abort();
    }
    let Ok(size) = u32::try_from(size) else {
        std::process::abort();
    };
    let storage = pool::alloc(size);
    // SAFETY: `size` fresh bytes, aligned to `MAX_ALIGN`, shared across forks.
    if storage.is_null() || unsafe { swr_v1_locks_init(storage, kind, 1) } != 0 {
        std::process::abort();
    }
    storage
}

/// `__construct(int $type = SWOOLE_MUTEX)`.
fn construct(call: &Call) {
    let held = held(call);
    if !held.storage.is_null() {
        call.throw_constructed_twice();
        call.set_bool(false);
        return;
    }
    let Ok(kind) = call.arity(0, 1).and_then(|()| call.long(1, MUTEX)) else {
        call.set_bool(false);
        return;
    };
    let supported = (0..32).contains(&kind) && KINDS & (1 << kind) != 0;
    let Some(kind) = i32::try_from(kind).ok().filter(|_| supported) else {
        // Upstream passes the format unformatted, with the type as the code.
        call.throw(
            swoole_rs_phpswoole::exception(),
            c"lock type[%d] is not support",
            kind,
        );
        call.set_bool(false);
        return;
    };
    *held = Held {
        storage: new_lock(kind),
        kind,
    };
    call.set_bool(true);
}

/// `SW_LOCK_CHECK_RETURN`: true on success, else false with `$this->errCode`.
pub(crate) fn check(call: &Call, result: i32) {
    if result != 0 {
        call.update_property_long(None, c"errCode", result.into());
    }
    call.set_bool(result == 0);
}

/// `swoole::sec2msec(double)`, narrowed to the `int` `Lock::lock` takes.
#[expect(
    clippy::cast_possible_truncation,
    reason = "upstream casts to long, then passes an int"
)]
pub(crate) fn milliseconds(seconds: f64) -> i32 {
    (seconds * 1000.0) as i64 as i32
}

/// `lock(int $operation = LOCK_EX, float $timeout = -1): bool`.
fn lock(call: &Call) {
    let parsed = call
        .arity(0, 2)
        .and_then(|()| Ok((call.long(1, LOCK_EX)?, call.double(2, -1.0)?)));
    let Ok((operation, timeout)) = parsed else {
        call.set_bool(false);
        return;
    };
    let Some(held) = constructed(call) else {
        return;
    };
    #[expect(clippy::cast_possible_truncation, reason = "upstream passes an int")]
    // SAFETY: a lock `new_lock` initialised in the pool.
    let result = unsafe {
        swr_v1_locks_lock(
            held.storage,
            held.kind,
            operation as i32,
            milliseconds(timeout),
            CAPABILITIES,
        )
    };
    check(call, result);
}

/// `unlock(): bool`.
fn unlock(call: &Call) {
    if let Some(held) = constructed(call) {
        // SAFETY: as in `lock`.
        check(call, unsafe {
            swr_v1_locks_unlock(held.storage, held.kind)
        });
    }
}

swoole_rs_zend::handlers! {
    zim_swoole_lock___construct => construct;
    zim_swoole_lock_lock => lock;
    zim_swoole_lock_unlock => unlock;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeouts_convert_like_sec2msec() {
        assert_eq!(milliseconds(-1.0), -1000, "the default blocks");
        assert_eq!(milliseconds(0.0015), 1, "truncates toward zero");
        assert_eq!(milliseconds(2.5), 2500, "seconds to milliseconds");
    }
}
