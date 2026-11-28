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

/// Borrowed libcurl push headers, valid during the offer.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct PushHeaders<'a>(NonNull<std::ffi::c_void>, PhantomData<&'a std::ffi::CStr>);

/// libcurl's `CURL_PUSH_OK` and `CURL_PUSH_DENY` decisions.
#[repr(i32)]
#[derive(Clone, Copy)]
enum Push {
    Accept = 0,
    Deny = 1,
}

unsafe extern "C" {
    safe fn curl_pushheader_bynum(
        headers: PushHeaders<'_>,
        index: usize,
    ) -> Option<swoole_rs_corebase::sys::Text<'_>>;
    safe fn swoole_curl_call_push(
        handle: PhpMulti<'_>,
        args: swoole_rs_zend::Zvals<'_>,
        result: &mut swoole_rs_zend::Owned,
    ) -> bool;
    safe fn swoole_curl_multi_keep(handle: PhpMulti<'_>, child: crate::php_handle::PhpHandle<'_>);
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
    safe fn swoole_curl_multi_take_easy(
        handle: PhpMulti<'_>,
        handles: &mut swoole_rs_zend::Owned,
        destroying: bool,
    );
    safe fn swoole_curl_multi_detach_native(
        handle: PhpMulti<'_>,
        easy: crate::php_handle::PhpHandle<'_>,
    );
    safe fn swoole_curl_multi_is_in_coroutine(handle: PhpMulti<'_>) -> bool;
    safe fn swoole_curl_multi_release_engine(handle: PhpMulti<'_>);
    safe fn swoole_curl_multi_release_push(handle: PhpMulti<'_>);
    safe fn swoole_curl_multi_set_number(
        multi: Option<swoole_rs_curl_sys::LiveMulti<'_>>,
        option: c_int,
        value: i64,
    ) -> c_int;
    safe fn swoole_curl_multi_set_push(
        handle: PhpMulti<'_>,
        multi: Option<swoole_rs_curl_sys::LiveMulti<'_>>,
        value: &swoole_rs_zend::Value,
    ) -> bool;
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
    zif_swoole_native_curl_multi_close => close;
    zif_swoole_native_curl_multi_init => init;
    zif_swoole_native_curl_multi_info_read => info_read;
    zif_swoole_native_curl_multi_add_handle => add;
    zif_swoole_native_curl_multi_remove_handle => remove;
    zif_swoole_native_curl_multi_exec => exec;
    zif_swoole_native_curl_multi_setopt => setopt;
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

fn close(call: &Call<'_>) {
    if call.arity(1, 1).is_err() {
        return;
    }
    let Some(handle) = argument(call) else { return };
    let mut handles = swoole_rs_zend::Owned::default();
    bailout::defer(contain_with(|| detach(handle, &mut handles, false)));
}

fn remove_easy(multi: &Multi, easy: crate::Easy<'_>) -> swoole_rs_curl_sys::CURLMcode {
    if let Some(transfer) = crate::owner(easy)
        && transfer
            .multi()
            .as_deref()
            .is_some_and(|owner| std::ptr::eq(owner, multi))
    {
        return multi.remove_handle(&transfer);
    }
    crate::curl_multi_remove_handle(multi.live(), easy)
}

// Keep the easy objects alive until every transfer is detached, including while
// destruction releases the multi engine. Dropping a handle can reenter PHP.
fn detach(handle: PhpMulti<'_>, handles: &mut swoole_rs_zend::Owned, destroying: bool) {
    swoole_curl_multi_take_easy(handle, handles, destroying);
    let Some(table) = handles.array() else { return };
    let multi = swoole_curl_multi_engine(handle);
    for (_, value) in table.entries() {
        let Some(easy) = value
            .object_ref()
            .and_then(|object| swoole_curl_object_handle(object))
        else {
            continue;
        };
        let cp = crate::php_handle::swoole_curl_easy(easy);
        if destroying && multi.is_some() && !crate::php::check_handle(cp, true, false) {
            continue;
        }
        crate::callbacks::verify(easy, false);
        if let Some(multi) = multi {
            remove_easy(multi, cp);
        } else {
            swoole_curl_multi_detach_native(handle, easy);
        }
    }
}

fn wrong_operation(message: &std::ffi::CStr) {
    bailout::defer(swoole_rs_phpswoole::swoole_fatal_error(
        swoole_rs_error::ErrorCode::SW_ERROR_WRONG_OPERATION as c_int,
        message,
    ));
}

fn engine(handle: PhpMulti<'_>) -> Option<&Multi> {
    let multi =
        swoole_curl_multi_engine(handle).filter(|_| swoole_curl_multi_is_in_coroutine(handle));
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
    use swoole_rs_curl_sys::CURLM_BAD_EASY_HANDLE;

    if call.arity(2, 2).is_err() {
        return;
    }
    let Some(handle) = argument(call) else { return };
    let Some(easy) = object_argument(call, 2) else {
        return;
    };
    if add && !checked(easy, true) {
        swoole_curl_multi_set_error(handle, CURLM_BAD_EASY_HANDLE);
        call.set_long(i64::from(CURLM_BAD_EASY_HANDLE));
        return;
    }
    let Some(multi) = engine(handle) else { return };
    let result = if add {
        if !crate::callbacks::prepare_transfer(easy) {
            return;
        }
        crate::owner(swoole_curl_easy(easy)).map_or(CURLM_BAD_EASY_HANDLE, |h| multi.add_handle(&h))
    } else {
        remove_easy(multi, swoole_curl_easy(easy))
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

fn setopt(call: &Call<'_>) {
    use crate::setopt::{libcurl, warn};
    use swoole_rs_curl_sys::{
        CURLM_OK, CURLM_UNKNOWN_OPTION, CURLMOPT_CHUNK_LENGTH_PENALTY_SIZE,
        CURLMOPT_CONTENT_LENGTH_PENALTY_SIZE, CURLMOPT_MAX_CONCURRENT_STREAMS,
        CURLMOPT_MAX_HOST_CONNECTIONS, CURLMOPT_MAX_PIPELINE_LENGTH,
        CURLMOPT_MAX_TOTAL_CONNECTIONS, CURLMOPT_MAXCONNECTS, CURLMOPT_PIPELINING,
        CURLMOPT_PUSHFUNCTION,
    };

    if call.arity(3, 3).is_err() {
        return;
    }
    let Some(handle) = argument(call) else { return };
    let Ok(option) = call.long(2, 0) else { return };
    let Some(value) = call.value(3) else { return };
    let Some(multi) = engine(handle) else { return };
    // PHP casts the zend_long to libcurl's 32-bit CURLMoption before dispatch.
    let option = c_int::try_from(option & i64::from(u32::MAX)).unwrap_or(-1);
    let result = match option {
        CURLMOPT_PUSHFUNCTION => {
            bailout::defer(contain_with(|| {
                call.set_bool(swoole_curl_multi_set_push(
                    handle,
                    multi.live(),
                    value.referred(),
                ));
            }));
            return;
        }
        CURLMOPT_PIPELINING
        | CURLMOPT_MAXCONNECTS
        | CURLMOPT_MAX_HOST_CONNECTIONS
        | CURLMOPT_MAX_PIPELINE_LENGTH
        | CURLMOPT_CONTENT_LENGTH_PENALTY_SIZE
        | CURLMOPT_CHUNK_LENGTH_PENALTY_SIZE
        | CURLMOPT_MAX_TOTAL_CONNECTIONS
        | CURLMOPT_MAX_CONCURRENT_STREAMS
            if option != CURLMOPT_MAX_CONCURRENT_STREAMS
                || (PHP_VERSION_ID >= 80200 && libcurl(7, 67, 0)) =>
        {
            let Some(number) = value.referred().applied_long() else {
                return;
            };
            if option == CURLMOPT_PIPELINING && number & 1 != 0 {
                let (level, message) = if libcurl(7, 62, 0) {
                    (
                        swoole_rs_zend::E_WARNING,
                        c"CURLPIPE_HTTP1 is no longer supported",
                    )
                } else {
                    (
                        swoole_rs_zend::sys::E_DEPRECATED.cast_signed(),
                        c"CURLPIPE_HTTP1 is deprecated",
                    )
                };
                if !warn(level, message) {
                    return;
                }
            }
            swoole_curl_multi_set_number(multi.live(), option, number)
        }
        _ => {
            call.argument_value_error(2, c"is not a valid cURL multi option");
            CURLM_UNKNOWN_OPTION
        }
    };
    swoole_curl_multi_set_error(handle, result);
    call.set_bool(result == CURLM_OK);
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
/// Deliver libcurl's offered transfer to PHP. A denied easy handle remains libcurl's.
#[unsafe(no_mangle)]
extern "C" fn swoole_curl_multi_on_push(
    parent: NonNull<swoole_rs_curl_sys::CURL>,
    easy: crate::Easy<'_>,
    count: usize,
    headers: PushHeaders<'_>,
    multi: PhpMulti<'_>,
) -> Push {
    let Some(parent) = swoole_curl_multi_find_easy(multi, parent).map(swoole_rs_zend::Value::copy)
    else {
        return Push::Deny;
    };
    let Some(source) = parent
        .object_ref()
        .and_then(|obj| swoole_curl_object_handle(obj))
    else {
        return Push::Deny;
    };
    let mut lines = swoole_rs_zend::OwnedArray::default();
    for i in 0..count {
        let Some(line) = curl_pushheader_bynum(headers, i) else {
            return Push::Deny;
        };
        lines.push_bytes(line.get().to_bytes());
    }
    let Some(child) = crate::php_handle::pushed_object(easy, source) else {
        return Push::Deny;
    };
    let args = [parent, child, lines.into_owned()];
    let Some(handle) = args[1]
        .object_ref()
        .and_then(|obj| swoole_curl_object_handle(obj))
    else {
        return Push::Deny;
    };
    let mut result = swoole_rs_zend::Owned::default();
    let mut called = false;
    bailout::defer(contain_with(|| {
        called = swoole_curl_call_push(multi, swoole_rs_zend::Zvals::owned(&args), &mut result);
    }));
    if !called && !bailout::pending() {
        crate::setopt::warn(
            swoole_rs_zend::E_WARNING,
            c"Cannot call the CURLMOPT_PUSHFUNCTION",
        );
    }
    let accept = called
        && !bailout::pending()
        && result.kind() != swoole_rs_zend::sys::IS_UNDEF
        && result
            .applied_long()
            .is_some_and(|code| code != Push::Deny as i64);
    // The return value can run a PHP destructor before the offer is accepted.
    drop(result);
    if accept && !bailout::pending() {
        swoole_curl_multi_keep(multi, handle);
        return Push::Accept;
    }
    crate::swr_v1_curl_destroy_handle(crate::Retired(easy));
    crate::php_handle::swoole_curl_set_easy(handle, crate::Easy(std::ptr::null_mut(), PhantomData));
    Push::Deny
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

swoole_rs_zend::exports! {
    preserve_pending:
/// Zend is releasing the multi object, including its last references to the easy handles.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_curl_multi_free_obj(object: ObjectRef<'_>) {
    if let Some(handle) = swoole_curl_multi_object(object) {
        let mut handles = swoole_rs_zend::Owned::default();
        bailout::defer(contain_with(|| detach(handle, &mut handles, true)));
        bailout::defer(contain_with(|| swoole_curl_multi_release_engine(handle)));
        drop(handles);
        bailout::defer(contain_with(|| swoole_curl_multi_release_push(handle)));
    }
    bailout::defer(contain_with(|| crate::php_handle::zend_object_std_dtor(object)));
}
}
