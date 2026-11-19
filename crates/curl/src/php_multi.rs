//! PHP's multi-handle functions over the coroutine cURL engine.
use crate::php_handle::{
    GcBuffer, swoole_curl_object_handle, swoole_curl_publish_roots, swr_zend_object_new,
    zend_get_gc_buffer_create, zend_std_get_properties,
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
    #[expect(
        improper_ctypes,
        reason = "the object retains the opaque Rust engine for the call"
    )]
    safe fn swoole_curl_multi_engine(handle: PhpMulti<'_>) -> Option<&Multi>;
    safe fn swoole_curl_multi_find_easy(
        handle: PhpMulti<'_>,
        easy: NonNull<swoole_rs_curl_sys::CURL>,
    ) -> Option<&swoole_rs_zend::Value>;
    safe fn swoole_curl_multi_membership(
        handle: PhpMulti<'_>,
        easy: crate::php_handle::PhpHandle<'_>,
        error: c_int,
        add: bool,
    );
    safe fn swoole_curl_multi_empty(handle: PhpMulti<'_>) -> bool;
    safe fn swoole_curl_multi_verify(handle: PhpMulti<'_>);
    safe fn swoole_curl_assign_long(slot: &swoole_rs_zend::Value, number: i64);
    safe fn swoole_curl_multi_set_error(handle: PhpMulti<'_>, code: c_int);
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
    zif_swoole_native_curl_multi_info_read => info_read;
    zif_swoole_native_curl_multi_add_handle => add;
    zif_swoole_native_curl_multi_remove_handle => remove;
    zif_swoole_native_curl_multi_exec => exec;
    zif_swoole_native_curl_multi_select => select;
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

fn wrong_operation(message: &std::ffi::CStr) {
    bailout::defer(swoole_rs_phpswoole::swoole_fatal_error(
        swoole_rs_error::ErrorCode::SW_ERROR_WRONG_OPERATION as c_int,
        message,
    ));
}

fn engine(handle: PhpMulti<'_>) -> Option<&Multi> {
    let multi = swoole_curl_multi_engine(handle);
    if multi.is_none() {
        wrong_operation(c"The given object is not a valid coroutine CurlMultiHandle object");
    }
    multi
}

fn current(multi: &Multi) -> Option<*mut std::ffi::c_void> {
    match crate::host::bound_co(multi, &mut [0; 128]) {
        Ok(co) => Some(co),
        Err((code, message)) => {
            bailout::defer(swoole_rs_phpswoole::swoole_fatal_error(code, message));
            None
        }
    }
}

fn add(call: &Call<'_>) {
    membership(call, true);
}

fn remove(call: &Call<'_>) {
    membership(call, false);
}

fn membership(call: &Call<'_>, add: bool) {
    use crate::php_handle::{checked, object_argument, swoole_curl_easy};
    use swoole_rs_corebase::CellExt;
    use swoole_rs_curl_sys::{CURLM_BAD_EASY_HANDLE, CURLM_BAD_HANDLE};

    if call.arity(2, 2).is_err() {
        return;
    }
    let Some(handle) = argument(call) else { return };
    let Some(easy) = object_argument(call, 2) else {
        return;
    };
    if add && !checked(easy, true) {
        call.set_bool(false);
        return;
    }
    let Some(multi) = engine(handle) else { return };
    let result = if add {
        if !crate::callbacks::prepare_transfer(easy) {
            return;
        }
        swoole_curl_easy(easy)
            .handle()
            .map_or(CURLM_BAD_EASY_HANDLE, |h| multi.add_handle(h))
    } else {
        // A handle outside a multi returns CURLM_BAD_HANDLE, including a second removal.
        swoole_curl_easy(easy)
            .handle()
            .filter(|h| h.multi.with_taken(|owner| owner.is_some()))
            .map_or(CURLM_BAD_HANDLE, |h| multi.remove_handle(h))
    };
    bailout::defer(contain_with(|| {
        swoole_curl_multi_membership(handle, easy, result, add);
    }));
    call.set_long(i64::from(result));
}

fn exec(call: &Call<'_>) {
    if call.arity(2, 2).is_err() {
        return;
    }
    let Some(handle) = argument(call) else { return };
    let Some(running) = call.value(2) else { return };
    let Some(multi) = engine(handle) else { return };
    if let Err(bailout) = contain_with(|| swoole_curl_multi_verify(handle)) {
        bailout.defer();
        return;
    }
    if current(multi).is_none() {
        return;
    }
    let result = multi.perform();
    if let Err(bailout) =
        contain_with(|| swoole_curl_assign_long(running, i64::from(multi.running.get())))
    {
        bailout.defer();
        return;
    }
    swoole_curl_multi_set_error(handle, result);
    call.set_long(i64::from(result));
}

fn select(call: &Call<'_>) {
    if call.arity(1, 2).is_err() {
        return;
    }
    let Some(handle) = argument(call) else { return };
    let Ok(timeout) = call.double(2, 1.0) else {
        return;
    };
    let Some(multi) = engine(handle) else { return };
    if PHP_VERSION_ID >= 80200 && !(0.0..=f64::from(c_int::MAX) / 1000.0).contains(&timeout) {
        if PHP_VERSION_ID >= 80500 {
            call.argument_value_error(2, c"must be between 0 and 2147483.647000");
        } else if PHP_VERSION_ID >= 80400 {
            call.argument_value_error(2, c"must be between 0 and 2147484");
        } else {
            wrong_operation(c"timeout must be between 0 and 2147484");
        }
        return;
    }
    if swoole_curl_multi_empty(handle) {
        call.set_long(0);
        return;
    }
    if let Some(co) = current(multi) {
        call.set_long(multi.select(co, timeout));
    }
}

fn info_read(call: &Call<'_>) {
    if call.arity(1, 2).is_err() {
        return;
    }
    let Some(handle) = argument(call) else { return };
    let Some(multi) = engine(handle) else { return };
    let mut pending = 0;
    let Some((kind, easy, result)) = multi.next_message(&mut pending) else {
        call.set_bool(false);
        return;
    };
    // Assigning to the queue-count reference may run a PHP destructor.
    let completed = NonNull::new(easy)
        .and_then(|easy| swoole_curl_multi_find_easy(handle, easy))
        .map(swoole_rs_zend::Value::copy);
    if let Some(slot) = call.value(2)
        && let Err(bailout) = contain_with(|| swoole_curl_assign_long(slot, i64::from(pending)))
    {
        bailout.defer();
        return;
    }
    let mut message = swoole_rs_zend::OwnedArray::default();
    message.insert_long(b"msg", i64::from(kind));
    message.insert_long(b"result", i64::from(result));
    if let Some(completed) = completed {
        if let Some(handle) = completed
            .object_ref()
            .and_then(|obj| swoole_curl_object_handle(obj))
        {
            crate::setopt::swoole_curl_save_error(handle, result);
        }
        message.insert(b"handle", completed);
    }
    call.set_owned(message.into_owned());
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
