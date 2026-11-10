//! `Swoole\Thread\Barrier`: a reusable barrier every thread holding the object
//! (or a container value referring to it) waits on.
use crate::{
    host::{self, handlers},
    item::{IS_BARRIER, Item, Value},
};
use std::{
    ptr,
    sync::{Arc, Barrier},
};
use swoole_rs_phpstubs::thread_barrier;
use swoole_rs_zend::{
    Call, Class, Minit, Object, Value as ZValue, set_object,
    sys::{ZEND_ACC_FINAL, ZEND_ACC_NOT_SERIALIZABLE, zend_class_entry, zend_object, zval},
    unset_property_deny,
};
use swoole_rs_zend::{method_table, sys as z};

static BARRIER: Class = Class::new();

/// `SW_ERROR_INVALID_PARAMS`.
const INVALID_PARAMS: i64 = 504;

type BarrierObject = Object<*const Barrier>;

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class; null is valid zeroed data.
    unsafe { BarrierObject::create(&BARRIER, ce) }
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees one of our objects.
    let shared = std::mem::replace(unsafe { BarrierObject::data(std) }, ptr::null());
    if !shared.is_null() {
        // SAFETY: the reference the object held.
        drop(unsafe { Arc::from_raw(shared) });
    }
    // SAFETY: as above.
    unsafe { BarrierObject::dtor(std) };
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_thread_barrier___construct, &thread_barrier::arginfo_class_Swoole_Thread_Barrier___construct, z::ZEND_ACC_PUBLIC;
        c"wait" => zim_swoole_thread_barrier_wait, &thread_barrier::arginfo_class_Swoole_Thread_Barrier_wait, z::ZEND_ACC_PUBLIC;
    ];
}

/// `php_swoole_thread_barrier_minit`.
pub fn barrier_minit(minit: &Minit) {
    minit.register(
        &BARRIER,
        c"Swoole\\Thread\\Barrier",
        &METHODS.0,
        |entry, handlers| {
            entry.ce_flags |= ZEND_ACC_FINAL | ZEND_ACC_NOT_SERIALIZABLE;
            handlers.clone_obj = None;
            handlers.unset_property = Some(unset_property_deny);
            BarrierObject::install(entry, handlers, create, free);
        },
    );
}

/// A Barrier stored in a container, by reference; `None` for other objects.
pub(crate) fn shared(value: &ZValue) -> Option<Item> {
    if value.class()? != BARRIER.entry() {
        return None;
    }
    let std = value.object()?;
    // SAFETY: one of our objects.
    let barrier = unsafe { *BarrierObject::data(std) };
    let value = if barrier.is_null() {
        // Upstream shares the null resource; reading it back gives an
        // unconstructed Barrier.
        None
    } else {
        // SAFETY: the object's live reference.
        unsafe { Arc::increment_strong_count(barrier) };
        // SAFETY: the reference just added.
        Some(unsafe { Arc::from_raw(barrier) })
    };
    Some(Item {
        kind: IS_BARRIER,
        value: Value::Barrier(value),
    })
}

/// `php_swoole_thread_barrier_create`: an object over `barrier`.
/// # Safety
/// In a request; `out` is writable and holds nothing.
pub(crate) unsafe fn object(out: *mut zval, barrier: Option<&Arc<Barrier>>) {
    // SAFETY: after MINIT; the class's own entry.
    let std = unsafe { BarrierObject::create(&BARRIER, BARRIER.entry()) };
    if let Some(barrier) = barrier {
        // SAFETY: the object just created.
        unsafe { *BarrierObject::data(std) = Arc::into_raw(Arc::clone(barrier)) };
    }
    // SAFETY: per the caller.
    unsafe { set_object(out, std) };
}

/// `__construct(int $count)`: the constructed check comes before the
/// arguments, as upstream's does.
fn construct(call: &Call) {
    // SAFETY: the methods are non-static and the class final.
    let slot = unsafe { BarrierObject::data(call.this()) };
    if !slot.is_null() {
        call.throw_constructed_twice();
        return;
    }
    let Ok(count) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        return;
    };
    if count < 2 {
        call.throw(
            swoole_rs_phpswoole::exception(),
            c"The parameter $count must be greater than 1",
            INVALID_PARAMS,
        );
        return;
    }
    let count = usize::try_from(count).unwrap_or(usize::MAX);
    *slot = Arc::into_raw(Arc::new(Barrier::new(count)));
}

fn wait(call: &Call) {
    // SAFETY: as in `construct`.
    let barrier = unsafe { *BarrierObject::data(call.this()) };
    if barrier.is_null() {
        // Swoole's fatal error, which exits; this frame owns nothing.
        host::unconstructed(true);
        return;
    }
    // SAFETY: the object holds a reference for as long as it lives.
    let barrier = unsafe { &*barrier };
    barrier.wait();
}

handlers! {
    zim_swoole_thread_barrier___construct => construct;
    zim_swoole_thread_barrier_wait => wait;
}
