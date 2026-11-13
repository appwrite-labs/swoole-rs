//! The executor introspection functions of `swoole_admin_server.cc`, which the
//! library's admin server calls: `swoole_get_vm_status()`,
//! `swoole_get_objects()` and `swoole_get_object_by_handle()`.
use std::{ops::Range, ptr::NonNull};
use swoole_rs_zend::{
    Call, Owned, OwnedArray, Php, handlers,
    sys::{
        swr_zend_object_addref, swr_zend_objects_get, swr_zend_objects_top,
        swr_zend_resource_count, zend_object,
    },
};

handlers! {
    zif_swoole_get_vm_status => vm_status;
    zif_swoole_get_objects => objects;
    zif_swoole_get_object_by_handle => object_by_handle;
}

/// `EG(objects_store).top`, on the thread running PHP.
fn objects_top(_php: Php<'_>) -> u32 {
    // SAFETY: the executor's object store, which this thread's PHP owns.
    unsafe { swr_zend_objects_top() }
}

/// An object the store holds, live while this call releases none. Only
/// [`live_objects`] makes one.
struct Live(NonNull<zend_object>);

impl Live {
    /// The object, with a new reference for the caller to hand on.
    fn retained(&self) -> Owned {
        // SAFETY: a live object.
        unsafe { swr_zend_object_addref(self.0.as_ptr()) };
        // SAFETY: the live object, with the reference just added.
        unsafe { Owned::object(self.0.as_ptr()) }
    }
}

/// The live objects of the store among `handles`, in handle order
/// (`objects_store_foreach`).
fn live_objects(php: Php<'_>, handles: Range<u32>) -> impl Iterator<Item = Live> {
    let top = objects_top(php);
    (handles.start..handles.end.min(top))
        // SAFETY: each handle is below the store's top.
        .filter_map(|handle| NonNull::new(unsafe { swr_zend_objects_get(handle) }))
        .map(Live)
}

fn vm_status(call: &Call) {
    call.set_array();
    // At most the store's u32 top.
    let count = u32::try_from(live_objects(call.php(), 0..u32::MAX).count()).unwrap_or(u32::MAX);
    call.add_assoc_long(c"object_num", i64::from(count));
    // SAFETY: the executor's resource list.
    call.add_assoc_long(
        c"resource_num",
        i64::from(unsafe { swr_zend_resource_count() }),
    );
}

fn objects(call: &Call) {
    if objects_top(call.php()) <= 1 {
        call.set_bool(false);
        return;
    }
    let mut array = OwnedArray::default();
    for object in live_objects(call.php(), 0..u32::MAX) {
        array.push(object.retained());
    }
    call.set_owned(array.into_owned());
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
    let top = objects_top(call.php());
    // `handle + 1` is at most the store's top.
    let object = in_store(handle, top)
        .and_then(|handle| live_objects(call.php(), handle..handle + 1).next());
    let Some(object) = object else {
        call.set_bool(false);
        return;
    };
    call.set_owned(object.retained());
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
