//! `Swoole\Lock`: a process-shared mutex, rwlock or spinlock in Swoole's global
//! memory pool, over the locks crate. Like upstream, a lock outlives its object:
//! forked workers may still hold it.
use std::{
    ffi::c_void,
    ptr,
    sync::{
        OnceLock,
        atomic::{AtomicPtr, Ordering},
    },
};
use swoole_rs_locks::{
    swr_v1_locks_align, swr_v1_locks_init, swr_v1_locks_lock, swr_v1_locks_size,
    swr_v1_locks_unlock,
};
use swoole_rs_phpstubs::lock;
use swoole_rs_zend::{
    Call, Class, Object, method_table, register_long_constant,
    sys::{
        ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, zend_class_entry, zend_object,
        zend_object_handlers,
    },
    throw_constructed_twice, throw_exception, unset_property_deny,
};

/// `swoole::Lock::Type`.
const RW_LOCK: i64 = 1;
const MUTEX: i64 = 3;
const SPIN_LOCK: i64 = 5;
/// `LOCK_EX` from `<sys/file.h>` (Linux and macOS).
const LOCK_EX: i64 = 2;
/// `alignof(std::max_align_t)`: what the pool's allocations guarantee.
const MAX_ALIGN: usize = 16;

/// What only C++ can reach, fixed at MINIT.
struct Native {
    /// Bit `1 << type` for each lock type this build has (`HAVE_RWLOCK`, `HAVE_SPINLOCK`).
    kinds: u32,
    /// The timed-lock capabilities `swr_v1_locks_lock` takes.
    capabilities: u32,
    alloc: unsafe extern "C" fn(size: u32) -> *mut c_void,
    /// `swoole_fatal_error(SW_ERROR_WRONG_OPERATION, "must call constructor first")`.
    unconstructed: unsafe extern "C" fn(),
}

static NATIVE: OnceLock<Native> = OnceLock::new();
static EXCEPTION: AtomicPtr<zend_class_entry> = AtomicPtr::new(ptr::null_mut());
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
        if let Some(native) = NATIVE.get() {
            // SAFETY: C++'s fatal-error report.
            unsafe { (native.unconstructed)() };
        }
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

/// MINIT: register the class and its constants as `php_swoole_lock_minit` did.
/// # Safety
/// Called once, at MINIT, with Swoole's exception class and the global pool's
/// allocator.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phplocks_lock_minit(
    module_number: i32,
    kinds: u32,
    capabilities: u32,
    exception: *mut zend_class_entry,
    alloc: Option<unsafe extern "C" fn(size: u32) -> *mut c_void>,
    unconstructed: Option<unsafe extern "C" fn()>,
) {
    if let (Some(alloc), Some(unconstructed)) = (alloc, unconstructed) {
        NATIVE.get_or_init(|| Native {
            kinds,
            capabilities,
            alloc,
            unconstructed,
        });
    }
    EXCEPTION.store(exception, Ordering::Relaxed);
    // SAFETY: MINIT, with a static table.
    unsafe { LOCK.register(c"Swoole\\Lock", METHODS.as_ptr(), configure) };
    let types = [
        (c"MUTEX", c"SWOOLE_MUTEX", MUTEX),
        (c"RWLOCK", c"SWOOLE_RWLOCK", RW_LOCK),
        (c"SPINLOCK", c"SWOOLE_SPINLOCK", SPIN_LOCK),
    ];
    let enabled = |kind: i64| kinds & (1 << kind) != 0;
    for (name, _, kind) in types.iter().filter(|(_, _, kind)| enabled(*kind)) {
        // SAFETY: MINIT, after registration.
        unsafe { LOCK.constant_long(name, *kind) };
    }
    // SAFETY: as above.
    unsafe { LOCK.property_long(c"errCode", 0) };
    for (_, name, kind) in types.iter().filter(|(_, _, kind)| enabled(*kind)) {
        // SAFETY: MINIT, with the module's number.
        unsafe { register_long_constant(name, *kind, module_number) };
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
fn new_lock(native: &Native, kind: i32) -> *mut c_void {
    let size = swr_v1_locks_size(kind);
    if size == 0 || swr_v1_locks_align(kind) > MAX_ALIGN {
        std::process::abort();
    }
    let Ok(size) = u32::try_from(size) else {
        std::process::abort();
    };
    // SAFETY: the pool's allocator.
    let storage = unsafe { (native.alloc)(size) };
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
        // SAFETY: in a handler, with `$this` live.
        unsafe { throw_constructed_twice(call.this()) };
        call.set_bool(false);
        return;
    }
    let (Ok(kind), Some(native)) = (
        call.arity(0, 1).and_then(|()| call.long(1, MUTEX)),
        NATIVE.get(),
    ) else {
        call.set_bool(false);
        return;
    };
    let supported = (0..32).contains(&kind) && native.kinds & (1 << kind) != 0;
    let Some(kind) = i32::try_from(kind).ok().filter(|_| supported) else {
        // Upstream passes the format unformatted, with the type as the code.
        // SAFETY: in a handler, with Swoole's exception class.
        unsafe {
            throw_exception(
                EXCEPTION.load(Ordering::Relaxed),
                c"lock type[%d] is not support",
                kind,
            );
        }
        call.set_bool(false);
        return;
    };
    *held = Held {
        storage: new_lock(native, kind),
        kind,
    };
    call.set_bool(true);
}

/// `SW_LOCK_CHECK_RETURN`: true on success, else false with `$this->errCode`.
fn check(call: &Call, result: i32) {
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
fn milliseconds(seconds: f64) -> i32 {
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
    let (Some(held), Some(native)) = (constructed(call), NATIVE.get()) else {
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
            native.capabilities,
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
