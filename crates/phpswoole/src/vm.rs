//! The executor introspection functions of `swoole_admin_server.cc`, which the
//! library's admin server calls: `swoole_get_vm_status()`,
//! `swoole_get_objects()` and `swoole_get_object_by_handle()`.
use std::ops::Range;
use swoole_rs_zend::{Call, OwnedArray, Php, handlers};

handlers! {
    zif_swoole_get_vm_status => vm_status;
    zif_swoole_get_objects => objects;
    zif_swoole_get_object_by_handle => object_by_handle;
}

/// The handles among `handles` at which the store holds a live object, in
/// order (`objects_store_foreach`).
fn live_handles(php: Php<'_>, handles: Range<u32>) -> impl Iterator<Item = u32> {
    let top = php.objects_top();
    (handles.start..handles.end.min(top)).filter(move |&handle| php.object_live(handle))
}

fn vm_status(call: &Call) {
    let php = call.php();
    call.set_array();
    // At most the store's u32 top.
    let count = u32::try_from(live_handles(php, 0..u32::MAX).count()).unwrap_or(u32::MAX);
    call.add_assoc_long(c"object_num", i64::from(count));
    call.add_assoc_long(c"resource_num", i64::from(php.resource_count()));
}

fn objects(call: &Call) {
    let php = call.php();
    if php.objects_top() <= 1 {
        call.set_bool(false);
        return;
    }
    let mut array = OwnedArray::default();
    // Each object with a reference of its own, which the array takes.
    for object in live_handles(php, 0..u32::MAX).filter_map(|handle| php.object_at(handle)) {
        array.push(object);
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
    let php = call.php();
    let object = in_store(handle, php.objects_top()).and_then(|handle| php.object_at(handle));
    let Some(object) = object else {
        call.set_bool(false);
        return;
    };
    call.set_owned(object);
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
