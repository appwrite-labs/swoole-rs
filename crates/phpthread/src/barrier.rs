//! `Swoole\Thread\Barrier`: a reusable barrier every thread holding the object
//! (or a container value referring to it) waits on.
use crate::{
    host::{handlers, host},
    item::{IS_BARRIER, Item, Value},
};
use std::{
    ptr,
    sync::{Arc, Barrier},
};
use swoole_rs_zend::{
    Call, Class, Object, set_object,
    sys::{
        ZEND_ACC_FINAL, ZEND_ACC_NOT_SERIALIZABLE, zend_class_entry, zend_function_entry,
        zend_object, zval,
    },
    throw_constructed_twice, throw_exception, unset_property_deny, value,
};

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

/// `php_swoole_thread_barrier_minit`.
/// # Safety
/// MINIT, after `swr_v1_phpthread_minit`, with a static method table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpthread_barrier_minit(
    methods: *const zend_function_entry,
) -> *mut zend_class_entry {
    // SAFETY: MINIT, with the caller's table.
    unsafe {
        BARRIER.register(c"Swoole\\Thread\\Barrier", methods, |entry, handlers| {
            entry.ce_flags |= ZEND_ACC_FINAL | ZEND_ACC_NOT_SERIALIZABLE;
            handlers.clone_obj = None;
            handlers.unset_property = Some(unset_property_deny);
            BarrierObject::install(entry, handlers, create, free);
        });
    }
    BARRIER.entry()
}

/// A Barrier stored in a container, by reference; `None` for other objects.
/// # Safety
/// `value` is a live object zval.
pub(crate) unsafe fn shared(value: *mut zval) -> Option<Item> {
    // SAFETY: per the caller.
    let std = unsafe { value::obj_of(value) };
    // SAFETY: a live object.
    if unsafe { (*std).ce } != BARRIER.entry() {
        return None;
    }
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
        // SAFETY: in a handler, with `$this` live.
        unsafe { throw_constructed_twice(call.this()) };
        return;
    }
    let Ok(count) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        return;
    };
    if count < 2 {
        // SAFETY: in a handler, with Swoole's exception class.
        unsafe {
            throw_exception(
                host().exception,
                c"The parameter $count must be greater than 1",
                INVALID_PARAMS,
            );
        }
        return;
    }
    let count = usize::try_from(count).unwrap_or(usize::MAX);
    *slot = Arc::into_raw(Arc::new(Barrier::new(count)));
}

fn wait(call: &Call) {
    // SAFETY: as in `construct`.
    let barrier = unsafe { *BarrierObject::data(call.this()) };
    if barrier.is_null() {
        // SAFETY: Swoole's fatal error (which exits); this frame owns nothing.
        unsafe { (host().unconstructed)(true) };
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
