//! PHP's multi-handle functions over the coroutine cURL engine.
use crate::php_handle::{
    GcBuffer, swoole_curl_publish_roots, swr_zend_object_new, zend_get_gc_buffer_create,
    zend_std_get_properties,
};
use crate::{Multi, Released};
use std::mem::MaybeUninit;
use std::rc::Rc;
use std::{ffi::c_int, marker::PhantomData, ptr::NonNull};
use swoole_rs_zend::sys::{PHP_VERSION_ID, zend_function, zend_object, zend_object_handlers, zval};
use swoole_rs_zend::{Call, ClassEntry, ObjectRef, bailout, contain_with};

/// A multi object kept alive by its argument for this call.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct PhpMulti<'a>(NonNull<std::ffi::c_void>, PhantomData<ObjectRef<'a>>);

unsafe extern "C" {
    safe static swoole_curl_multi_size: usize;
    safe static swoole_curl_multi_offset: usize;
    safe fn swoole_curl_multi_handlers(ce: ClassEntry<'_>) -> &'static zend_object_handlers;
    safe fn swoole_curl_multi_trace(gc: GcBuffer, handle: PhpMulti<'_>);
    safe static swoole_coroutine_curl_multi_handle_ce: Option<ClassEntry<'static>>;
    #[expect(
        improper_ctypes,
        reason = "C stores the opaque Rc pointer and returns it to Rust at destruction"
    )]
    safe fn swoole_curl_multi_attach(handle: PhpMulti<'_>, engine: Released);
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
    zif_swoole_native_curl_multi_init => init;
    zif_swoole_native_curl_multi_errno => errno;
}

fn init(call: &Call<'_>) {
    if call.arity(0, 0).is_err() {
        return;
    }
    let Some(class) = swoole_coroutine_curl_multi_handle_ce else {
        return;
    };
    let object = class.create_object();
    let Some(handle) = object
        .object_ref()
        .and_then(|object| swoole_curl_multi_object(object))
    else {
        return;
    };
    let Some(engine) = NonNull::new(Rc::into_raw(Multi::create()).cast_mut()) else {
        return;
    };
    bailout::defer(contain_with(|| {
        swoole_curl_multi_attach(handle, Released(engine));
    }));
    call.set_owned(object);
}

fn errno(call: &Call<'_>) {
    if call.arity(1, 1).is_ok()
        && let Some(handle) = argument(call)
    {
        call.set_long(i64::from(swoole_curl_multi_error(handle)));
    }
}

swoole_rs_zend::exports! {
/// Allocate the multi object with the handler table for this PHP version.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_curl_multi_create_object(ce: ClassEntry<'_>) -> *mut zend_object {
    swr_zend_object_new(swoole_curl_multi_size, swoole_curl_multi_offset, ce, swoole_curl_multi_handlers(ce))
}

#[unsafe(no_mangle)]
pub extern "C" fn swoole_curl_multi_get_constructor(_object: ObjectRef<'_>) -> *mut zend_function {
    swoole_rs_zend::throw_error(c"Cannot directly construct CurlMultiHandle, use curl_multi_init() instead");
    std::ptr::null_mut()
}

/// Report the push callback and each retained easy handle to Zend's collector.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_curl_multi_get_gc(object: ObjectRef<'_>, table: &mut MaybeUninit<*mut zval>, count: &mut MaybeUninit<c_int>) -> *mut std::ffi::c_void {
    let gc = zend_get_gc_buffer_create();
    if let Some(handle) = swoole_curl_multi_object(object) {
        swoole_curl_multi_trace(gc, handle);
    }
    swoole_curl_publish_roots(gc, table, count);
    if PHP_VERSION_ID >= 80500 {
        std::ptr::null_mut()
    } else {
        zend_std_get_properties(object)
    }
}
}
