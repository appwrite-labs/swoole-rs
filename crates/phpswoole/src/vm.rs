//! The executor introspection functions of `swoole_admin_server.cc`, which the
//! library's admin server calls: `swoole_get_vm_status()`,
//! `swoole_get_objects()` and `swoole_get_object_by_handle()`.
use std::ptr::NonNull;
use swoole_rs_zend::{
    Call, handlers, set_object,
    sys::{
        add_next_index_object, swr_zend_object_addref, swr_zend_objects_get, swr_zend_objects_top,
        swr_zend_resource_count, zend_object,
    },
};

handlers! {
    zif_swoole_get_vm_status => vm_status;
    zif_swoole_get_objects => objects;
    zif_swoole_get_object_by_handle => object_by_handle;
}

/// The live objects of the store, in handle order (`objects_store_foreach`).
fn live_objects() -> impl Iterator<Item = NonNull<zend_object>> {
    // SAFETY: the executor's object store.
    let top = unsafe { swr_zend_objects_top() };
    // SAFETY: each handle is below the store's top.
    (0..top).filter_map(|handle| NonNull::new(unsafe { swr_zend_objects_get(handle) }))
}

fn vm_status(call: &Call) {
    call.set_array();
    // At most the store's u32 top.
    let count = u32::try_from(live_objects().count()).unwrap_or(u32::MAX);
    call.add_assoc_long(c"object_num", i64::from(count));
    // SAFETY: the executor's resource list.
    call.add_assoc_long(
        c"resource_num",
        i64::from(unsafe { swr_zend_resource_count() }),
    );
}

fn objects(call: &Call) {
    // SAFETY: the executor's object store.
    if unsafe { swr_zend_objects_top() } <= 1 {
        call.set_bool(false);
        return;
    }
    call.set_array();
    for object in live_objects() {
        // SAFETY: a live object; the array takes the new reference.
        unsafe { swr_zend_object_addref(object.as_ptr()) };
        // SAFETY: as above.
        unsafe { add_next_index_object(call.return_value(), object.as_ptr()) };
    }
}

/// `handle` as a slot of a store whose top is `top`; a negative handle names
/// no object.
fn in_store(handle: i64, top: u32) -> Option<u32> {
    u32::try_from(handle)
        .ok()
        .filter(|&handle| top > 1 && handle < top)
}

fn object_by_handle(call: &Call) {
    let Ok(handle) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return;
    };
    // SAFETY: the executor's object store.
    let top = unsafe { swr_zend_objects_top() };
    // SAFETY: the handle is below the store's top.
    let object = in_store(handle, top)
        .and_then(|handle| NonNull::new(unsafe { swr_zend_objects_get(handle) }));
    let Some(object) = object else {
        call.set_bool(false);
        return;
    };
    // SAFETY: a live object; the return value takes the new reference.
    unsafe { swr_zend_object_addref(object.as_ptr()) };
    // SAFETY: the return slot is writable.
    unsafe { set_object(call.return_value(), object.as_ptr()) };
}

#[cfg(test)]
mod tests {
    use super::in_store;

    #[test]
    fn handles_must_fit_the_store() {
        assert_eq!(in_store(1, 2), Some(1));
        assert_eq!(in_store(2, 2), None);
        assert_eq!(in_store(-1, 5), None);
        assert_eq!(in_store(0, 1), None);
    }
}
