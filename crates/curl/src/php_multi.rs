//! PHP's multi-handle functions over the coroutine cURL engine.
use std::{ffi::c_int, marker::PhantomData, ptr::NonNull};
use swoole_rs_zend::{Call, ObjectRef};

/// A multi object kept alive by its argument for this call.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct PhpMulti<'a>(NonNull<std::ffi::c_void>, PhantomData<ObjectRef<'a>>);

unsafe extern "C" {
    safe fn swoole_curl_multi_object(object: ObjectRef<'_>) -> Option<PhpMulti<'_>>;
    safe fn swoole_curl_multi_error(handle: PhpMulti<'_>) -> c_int;
}

fn argument<'a>(call: &'a Call<'_>) -> Option<PhpMulti<'a>> {
    let value = call.value(1)?.referred();
    let handle = value
        .object_ref()
        .and_then(|object| swoole_curl_multi_object(object));
    if handle.is_none() {
        call.wrong_class(1, c"CurlMultiHandle", value);
    }
    handle
}

swoole_rs_zend::handlers! {
    zif_swoole_native_curl_multi_errno => errno;
}

fn errno(call: &Call<'_>) {
    if call.arity(1, 1).is_ok()
        && let Some(handle) = argument(call)
    {
        call.set_long(i64::from(swoole_curl_multi_error(handle)));
    }
}
