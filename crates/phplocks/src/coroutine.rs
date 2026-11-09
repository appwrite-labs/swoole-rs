//! `Swoole\\Coroutine\\Lock`: PHP object ownership over the Rust coroutine lock.
use std::ffi::{c_int, c_void};
use swoole_rs_locks::coroutine::{self, CoroutineHost, CoroutineLock, CoroutineOutcome};
use swoole_rs_memory::members::pool;
use swoole_rs_phpstubs::coroutine_lock;
use swoole_rs_zend::{
    Call, Class, Object, bailout, method_table,
    sys::{
        ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, zend_class_entry, zend_object,
        zend_object_handlers,
    },
    throw_constructed_twice, unset_property_deny,
};

/// `SW_ERROR_WRONG_OPERATION`.
const WRONG_OPERATION: c_int = 509;
static CLASS: Class = Class::new();

/// The lock's effects, as upstream's `CoroutineLock` reaches them: the current
/// coroutine, the global pool for a shared word, and either `System::sleep`
/// backoff or, with `io_uring` (whose engine always has futexes), the ring's futex.
static HOST: CoroutineHost = CoroutineHost {
    current: host::current,
    cid: host::cid,
    alloc_shared: host::alloc_shared,
    free_shared: host::free_shared,
    #[cfg(not(feature = "iouring"))]
    sleep: Some(host::sleep),
    #[cfg(feature = "iouring")]
    sleep: None,
    #[cfg(not(feature = "iouring"))]
    wait: None,
    #[cfg(feature = "iouring")]
    wait: Some(host::wait),
    #[cfg(not(feature = "iouring"))]
    wake: None,
    #[cfg(feature = "iouring")]
    wake: Some(host::wake),
    warning: host::warning,
};

mod host {
    use super::{CoroutineOutcome, c_int, c_void, pool};

    /// No leaf runs PHP, so none bails out.
    const fn done(code: c_int) -> CoroutineOutcome {
        CoroutineOutcome { code, bailed: 0 }
    }
    pub(super) extern "C" fn current() -> *mut c_void {
        swoole_rs_phpcoroutines::current()
    }
    pub(super) extern "C" fn cid(co: *mut c_void) -> i64 {
        swoole_rs_phpcoroutines::cid(co)
    }
    pub(super) extern "C" fn alloc_shared() -> *mut u32 {
        // SAFETY: SwooleG.memory_pool lives from swoole_init, before MINIT.
        unsafe { pool::alloc(4) }.cast()
    }
    /// The global pool frees nothing.
    pub(super) unsafe extern "C" fn free_shared(_word: *mut u32) {}
    #[cfg(not(feature = "iouring"))]
    pub(super) extern "C" fn sleep(seconds: f64) -> CoroutineOutcome {
        // SAFETY: crates/hooks's (upstream symbol).
        unsafe extern "C" {
            #[link_name = "_ZN6swoole9coroutine6System5sleepEd"]
            fn system_sleep(seconds: f64) -> c_int;
        }
        // SAFETY: the lock sleeps only inside a coroutine.
        done(unsafe { system_sleep(seconds) })
    }
    // SAFETY: crates/hooks's engine and its static host.
    #[cfg(feature = "iouring")]
    unsafe extern "C" {
        #[link_name = "swr_v1_uring_native"]
        static IOURING_HOST: c_void;
        fn swr_v1_uring_futex_wait(host: *const c_void, futex: *mut u32) -> c_int;
        fn swr_v1_uring_futex_wakeup(host: *const c_void, futex: *mut u32) -> c_int;
    }
    /// `Iouring::futex_wait`.
    #[cfg(feature = "iouring")]
    pub(super) unsafe extern "C" fn wait(word: *mut u32) -> CoroutineOutcome {
        // SAFETY: the lock's live word, in a coroutine.
        done(unsafe { swr_v1_uring_futex_wait(&raw const IOURING_HOST, word) })
    }
    /// `Iouring::futex_wakeup`.
    #[cfg(feature = "iouring")]
    pub(super) unsafe extern "C" fn wake(word: *mut u32) -> CoroutineOutcome {
        // SAFETY: as in `wait`.
        done(unsafe { swr_v1_uring_futex_wakeup(&raw const IOURING_HOST, word) })
    }
    pub(super) extern "C" fn warning() -> u8 {
        swoole_rs_corebase::log::warning(
            c"swoole::CoroutineLock::lock_impl",
            format_args!("The coroutine lock can only be used in a coroutine environment"),
        );
        0
    }
}

#[repr(C)]
struct Held {
    owner: *mut CoroutineLock,
}

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend supplies this class or a subclass; zeroed Held means unconstructed.
    unsafe { Object::<Held>::create(&CLASS, ce) }
}
unsafe extern "C" fn free(object: *mut zend_object) {
    // SAFETY: Zend supplies an object created with this layout.
    let held = unsafe { Object::<Held>::data(object) };
    if !held.owner.is_null() {
        // SAFETY: the object uniquely owns its Rust lock; no method remains suspended at final release.
        unsafe { coroutine::coroutine_free(held.owner) };
    }
    // SAFETY: the owner has been freed and Zend now destroys its properties.
    unsafe { Object::<Held>::dtor(object) };
}
fn owner(call: &Call) -> *mut CoroutineLock {
    // SAFETY: these non-static methods receive one of our objects. Copy the pointer
    // so no mutable Held borrow spans scheduler suspension or PHP reentry.
    unsafe { Object::<Held>::data(call.this()) }.owner
}
fn constructed(call: &Call) -> Option<*mut CoroutineLock> {
    let lock = owner(call);
    if lock.is_null() {
        // SAFETY: Swoole's fatal error hook; the handler's frames own nothing.
        unsafe { swoole_rs_corebase::fatal_error(WRONG_OPERATION, c"must call constructor first") };
        return None;
    }
    Some(lock)
}
fn configure(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    Object::<Held>::install(entry, handlers, create, free);
}
method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_coroutine_lock___construct, &coroutine_lock::arginfo_class_Swoole_Coroutine_Lock___construct, ZEND_ACC_PUBLIC;
        c"lock" => zim_swoole_coroutine_lock_lock, &coroutine_lock::arginfo_class_Swoole_Coroutine_Lock_lock, ZEND_ACC_PUBLIC;
        c"unlock" => zim_swoole_coroutine_lock_unlock, &coroutine_lock::arginfo_class_Swoole_Coroutine_Lock_unlock, ZEND_ACC_PUBLIC;
    ];
}
/// `php_swoole_coroutine_lock_minit`.
/// # Safety
/// MINIT, once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_coroutine_lock_minit(_module_number: c_int) {
    // SAFETY: MINIT installs a static method table.
    unsafe { CLASS.register(c"Swoole\\Coroutine\\Lock", METHODS.as_ptr(), configure) };
    // SAFETY: the newly registered class owns this public default property.
    unsafe { CLASS.property_long(c"errCode", 0) };
}
fn construct(call: &Call) {
    if !owner(call).is_null() {
        // SAFETY: Zend supplies a live object of this class.
        unsafe { throw_constructed_twice(call.this()) };
        call.set_bool(false);
        return;
    }
    let Ok(shared) = call.arity(0, 1).and_then(|()| call.bool(1, false)) else {
        call.set_bool(false);
        return;
    };
    // SAFETY: the host is a static whose leaves cannot unwind.
    let lock = unsafe { coroutine::coroutine_new(shared, &raw const HOST) };
    // SAFETY: the constructor owns this object's currently empty Held slot.
    unsafe { Object::<Held>::data(call.this()) }.owner = lock;
    call.set_bool(true);
}
fn check(call: &Call, result: CoroutineOutcome) {
    if result.bailed != 0 {
        bailout::Bailout.defer();
        return;
    }
    super::lock::check(call, result.code);
}
fn lock(call: &Call) {
    let Ok(operation) = call
        .arity(0, 1)
        .and_then(|()| call.long(1, super::lock::LOCK_EX))
    else {
        call.set_bool(false);
        return;
    };
    let Some(owner) = constructed(call) else {
        return;
    };
    // LOCK_NB is 4 on both supported platforms; checking the zend_long avoids truncation.
    let blocking = operation & 4 == 0;
    // SAFETY: Zend holds $this throughout the method, including cooperative suspension.
    check(call, unsafe {
        coroutine::coroutine_acquire(owner, blocking)
    });
}
fn unlock(call: &Call) {
    let Some(owner) = constructed(call) else {
        return;
    };
    // SAFETY: Zend holds $this throughout the operation and its native wake leaf.
    check(call, unsafe { coroutine::coroutine_unlock(owner) });
}
swoole_rs_zend::handlers! {
    zim_swoole_coroutine_lock___construct => construct;
    zim_swoole_coroutine_lock_lock => lock;
    zim_swoole_coroutine_lock_unlock => unlock;
}
