//! `Swoole\\Coroutine\\Lock`: PHP object ownership over the Rust coroutine lock.
use std::{
    ffi::c_void,
    ptr,
    sync::{
        OnceLock,
        atomic::{AtomicPtr, Ordering},
    },
};
use swoole_rs_locks::coroutine::{self, CoroutineHost, CoroutineLock, CoroutineOutcome};
use swoole_rs_phpstubs::coroutine_lock;
use swoole_rs_zend::{
    Call, Class, Object, bailout, method_table,
    sys::{
        ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, zend_class_entry, zend_object,
        zend_object_handlers,
    },
    throw_constructed_twice, unset_property_deny,
};

static CLASS: Class = Class::new();
static HOST: AtomicPtr<CoroutineHost> = AtomicPtr::new(ptr::null_mut());
static UNCONSTRUCTED: OnceLock<unsafe extern "C" fn(*mut c_void)> = OnceLock::new();

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
    if !lock.is_null() {
        return Some(lock);
    }
    if let Some(report) = UNCONSTRUCTED.get() {
        // SAFETY: the C++ fatal reporter is contained below this Rust frame by Zend's C shim.
        bailout::defer(unsafe { bailout::trap(*report, ptr::null_mut()) });
    }
    None
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
/// # Safety
/// MINIT once, with a process-lived immutable host and a C++ fatal reporter.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phplocks_coroutine_minit(
    host: *const CoroutineHost,
    unconstructed: unsafe extern "C" fn(*mut c_void),
) {
    HOST.store(host.cast_mut(), Ordering::Relaxed);
    UNCONSTRUCTED.get_or_init(|| unconstructed);
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
    // SAFETY: MINIT installed the immutable process-lived host.
    let lock = unsafe { coroutine::coroutine_new(shared, HOST.load(Ordering::Relaxed)) };
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
