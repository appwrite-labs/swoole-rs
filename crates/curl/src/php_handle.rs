//! PHP entry points from both bundled `thirdparty/php*/curl/interface.cc` copies.
//! The argument zval keeps its `php_curl` object alive. Native accessors supply
//! the version-specific field offsets for callbacks, streams and option storage.
use crate::Easy;
use std::{ffi::c_int, marker::PhantomData, ptr::NonNull};
use swoole_rs_corebase::sys::Text;
use swoole_rs_zend::sys::{zend_object, zend_object_handlers, zend_result, zval};
use swoole_rs_zend::{Call, ObjectRef, sys::PHP_VERSION_ID};

#[repr(transparent)]
#[derive(Clone, Copy)]
pub(super) struct PhpHandle<'a>(NonNull<std::ffi::c_void>, PhantomData<ObjectRef<'a>>);

/// A libcurl allocation transferred as a POSIX iovec; consumed by `swoole_curl_free_bytes`.
#[repr(transparent)]
struct CurlBytes(std::io::IoSlice<'static>);

/// Zend owns the request-local GC scratch buffer throughout one `get_gc` call.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub(super) struct GcBuffer(NonNull<std::ffi::c_void>);

unsafe extern "C" {
    safe static swoole_curl_object_size: usize;
    safe static swoole_curl_object_offset: usize;
    // Written once at module startup, before Zend can construct an object.
    safe static swoole_coroutine_curl_handle_handlers: zend_object_handlers;
    pub(super) safe fn swr_zend_object_new(
        size: usize,
        offset: usize,
        ce: swoole_rs_zend::ClassEntry<'_>,
        handlers: &zend_object_handlers,
    ) -> *mut zend_object;
    safe fn zend_std_cast_object_tostring(
        object: ObjectRef<'_>,
        result: &mut std::mem::MaybeUninit<zval>,
        kind: u32,
    ) -> zend_result;
    safe fn swoole_curl_prepare_free(handle: PhpHandle<'_>) -> bool;
    safe fn swoole_curl_has_clone_storage(handle: PhpHandle<'_>) -> bool;
    safe fn swoole_curl_release_clone(handle: PhpHandle<'_>) -> bool;
    safe fn swoole_curl_free_lists(handle: PhpHandle<'_>);
    safe fn swoole_curl_free_callbacks(handle: PhpHandle<'_>);
    safe fn swoole_curl_release_values(handle: PhpHandle<'_>);
    pub(super) safe fn zend_object_std_dtor(object: ObjectRef<'_>);
    safe fn curl_easy_cleanup(easy: crate::Retired<'_>);
    pub(super) safe fn zend_get_gc_buffer_create() -> GcBuffer;
    safe fn swoole_curl_trace_callbacks(gc: GcBuffer, handle: PhpHandle<'_>);
    safe fn swoole_curl_trace_values(gc: GcBuffer, handle: PhpHandle<'_>);
    pub(super) safe fn swoole_curl_publish_roots(
        gc: GcBuffer,
        table: &mut std::mem::MaybeUninit<*mut swoole_rs_zend::sys::zval>,
        count: &mut std::mem::MaybeUninit<c_int>,
    );
    pub(super) safe fn zend_std_get_properties(object: ObjectRef<'_>) -> *mut std::ffi::c_void;
    // The native accessor checks the object's class before recovering php_curl.
    pub(super) safe fn swoole_curl_object_handle(object: ObjectRef<'_>) -> Option<PhpHandle<'_>>;
    safe fn swoole_curl_new_handle(slot: &mut swoole_rs_zend::Owned) -> PhpHandle<'_>;
    safe fn swoole_curl_bind_easy(handle: PhpHandle<'_>, easy: Easy<'_>);
    pub(super) safe fn swoole_curl_easy(handle: PhpHandle<'_>) -> Easy<'_>;
    safe fn swoole_curl_easy_upkeep(handle: PhpHandle<'_>) -> c_int;
    safe fn swoole_curl_take_private(handle: PhpHandle<'_>, previous: &mut swoole_rs_zend::Owned);
    safe fn swoole_curl_reset_handlers(handle: PhpHandle<'_>);
    safe fn curl_easy_pause(easy: Easy<'_>, mask: c_int) -> c_int;
    safe fn swoole_curl_in_callback(handle: PhpHandle<'_>) -> bool;
    // libcurl returns an allocated pointer, with the binary length as a separate
    // output for unescape. The native accessor packages them as one owned span.
    safe fn swoole_curl_url_bytes(
        handle: PhpHandle<'_>,
        input: &swoole_rs_zend::ZStr,
        decode: bool,
        result: &mut CurlBytes,
    ) -> bool;
    safe fn swoole_curl_free_bytes(buffer: CurlBytes);

    safe fn swoole_curl_new_clone(slot: &mut swoole_rs_zend::Owned) -> PhpHandle<'_>;
    safe fn zend_clear_exception();
    safe fn swoole_curl_alloc_handlers(handle: PhpHandle<'_>);
    safe fn swoole_curl_alloc_lists(handle: PhpHandle<'_>);
    safe fn swoole_curl_share_lists(handle: PhpHandle<'_>, source: PhpHandle<'_>);
    pub(super) safe fn swoole_curl_set_easy(handle: PhpHandle<'_>, easy: Easy<'_>);
    safe fn curl_easy_duphandle(source: Easy<'_>) -> *mut swoole_rs_curl_sys::CURL;
    safe fn swoole_curl_error_number(handle: PhpHandle<'_>) -> c_int;
    safe fn swoole_curl_error_buffer(handle: PhpHandle<'_>) -> Text<'_>;
    safe fn curl_easy_strerror(code: c_int) -> Text<'static>;
}

swoole_rs_zend::handlers! {
    zif_swoole_native_curl_copy_handle => copy_handle;
    zif_swoole_native_curl_init => init;
    zif_swoole_native_curl_escape => escape;
    zif_swoole_native_curl_unescape => unescape;
    zif_swoole_native_curl_pause => pause;
    zif_swoole_native_curl_upkeep => upkeep;
    zif_swoole_native_curl_reset => reset;
    zif_swoole_native_curl_close => close;
    zif_swoole_native_curl_errno => errno;
    zif_swoole_native_curl_error => error;
}

fn init(call: &Call<'_>) {
    if call.arity(0, 1).is_err() {
        return;
    }
    let Ok(url) = call.zstr_or_null(1) else {
        return;
    };
    let raw = swoole_rs_curl_sys::curl_easy_init();
    if raw.is_null() {
        swoole_rs_zend::bailout::defer(swoole_rs_zend::report(
            swoole_rs_zend::E_WARNING,
            c"Could not initialize a new cURL handle",
        ));
        call.set_bool(false);
        return;
    }
    let easy = Easy(raw, PhantomData);
    let mut object = swoole_rs_zend::Owned::default();
    let handle = swoole_curl_new_handle(&mut object);
    swoole_curl_alloc_handlers(handle);
    swoole_curl_alloc_lists(handle);
    swoole_curl_bind_easy(handle, easy);
    crate::setopt::defaults(handle);
    crate::swr_v1_curl_create_handle(easy);
    if url.is_none_or(|url| crate::setopt::set_url(call, handle, url)) {
        call.set_owned(object);
    } else {
        drop(object);
        call.set_bool(false);
    }
}

pub(super) fn argument<'a>(call: &'a Call<'_>, count: u32) -> Option<PhpHandle<'a>> {
    call.arity(count, count).ok()?;
    object_argument(call, 1)
}

/// The first argument's handle, usable as `swoole_curl_get_handle` requires for a read, or
/// none with false returned. The caller has checked the argument count.
pub(super) fn handle_argument<'a>(call: &'a Call<'_>) -> Option<PhpHandle<'a>> {
    let handle = object_argument(call, 1)?;
    if checked(handle, PHP_VERSION_ID >= 80400) {
        Some(handle)
    } else {
        call.set_bool(false);
        None
    }
}

pub(super) fn object_argument<'a>(call: &'a Call<'_>, index: u32) -> Option<PhpHandle<'a>> {
    let arg = call.value(index)?.referred();
    let Some(handle) = arg
        .object_ref()
        .and_then(|object| swoole_curl_object_handle(object))
    else {
        call.wrong_class(index, c"CurlHandle", arg);
        return None;
    };
    Some(handle)
}

pub(super) fn checked(handle: PhpHandle<'_>, exclusive: bool) -> bool {
    super::php::check_handle(swoole_curl_easy(handle), exclusive, true)
}

fn error_handle<'a>(call: &'a Call<'_>) -> Option<PhpHandle<'a>> {
    call.arity(1, 1).ok()?;
    handle_argument(call)
}

fn close(call: &Call<'_>) {
    let Some(handle) = argument(call, 1) else {
        return;
    };
    if checked(handle, true) {
        in_callback(call, handle, b"close");
    }
}

fn in_callback(call: &Call<'_>, handle: PhpHandle<'_>, operation: &[u8]) -> bool {
    if !swoole_curl_in_callback(handle) {
        return false;
    }
    let name = call.function().name().map_or(&[][..], |name| name.bytes());
    let message = [
        name,
        b"(): Attempt to ",
        operation,
        b" cURL handle from a callback",
    ]
    .concat();
    call.throw_error(&std::ffi::CString::new(message).unwrap_or_default());
    true
}

fn reset(call: &Call<'_>) {
    let Some(handle) = argument(call, 1) else {
        return;
    };
    if !checked(handle, true) {
        call.set_bool(false);
        return;
    }
    if in_callback(call, handle, b"reset") {
        return;
    }
    crate::swoole_curl_easy_reset(crate::Idle(swoole_curl_easy(handle)));
    let mut previous = swoole_rs_zend::Owned::default();
    if PHP_VERSION_ID >= 80400 {
        swoole_curl_take_private(handle, &mut previous);
    }
    swoole_rs_zend::bailout::defer(swoole_rs_zend::contain_with(|| {
        swoole_curl_reset_handlers(handle);
        crate::setopt::defaults(handle);
    }));
    drop(previous);
}

fn pause(call: &Call<'_>) {
    let Some(handle) = argument(call, 2) else {
        return;
    };
    let Ok(mask) = call.long(2, 0) else { return };
    if !checked(handle, true) {
        call.set_bool(false);
        return;
    }
    // libcurl can synchronously deliver buffered data to a PHP callback.
    let easy = swoole_curl_easy(handle);
    let result = swoole_rs_zend::contain_with(|| {
        call.set_long(i64::from(curl_easy_pause(
            easy,
            swoole_rs_corebase::CCast::c_cast(mask),
        )));
    });
    swoole_rs_zend::bailout::defer(result);
}

fn upkeep(call: &Call<'_>) {
    if let Some(handle) = argument(call, 1) {
        call.set_bool(checked(handle, true) && swoole_curl_easy_upkeep(handle) == 0);
    }
}

fn escape(call: &Call<'_>) {
    convert_url(call, false);
}

fn unescape(call: &Call<'_>) {
    convert_url(call, true);
}

fn convert_url(call: &Call<'_>, decode: bool) {
    let Some(handle) = argument(call, 2) else {
        return;
    };
    let Ok(Some(text)) = call.zstr(2) else { return };
    if !checked(handle, true) {
        call.set_bool(false);
        return;
    }
    if c_int::try_from(text.bytes().len()).is_err() {
        call.set_bool(false);
        return;
    }
    let mut output = CurlBytes(std::io::IoSlice::new(&[]));
    if swoole_curl_url_bytes(handle, text, decode, &mut output) {
        call.set_bytes(&output.0);
        swoole_curl_free_bytes(output);
    } else {
        call.set_bool(false);
    }
}

fn errno(call: &Call<'_>) {
    if let Some(handle) = error_handle(call) {
        call.set_long(i64::from(swoole_curl_error_number(handle)));
    }
}

fn error(call: &Call<'_>) {
    let Some(handle) = error_handle(call) else {
        return;
    };
    let code = swoole_curl_error_number(handle);
    if code == 0 {
        call.set_empty_string();
        return;
    }
    let buffer = swoole_curl_error_buffer(handle).get();
    let message = if buffer.is_empty() {
        curl_easy_strerror(code).get()
    } else {
        buffer
    };
    call.set_bytes(message.to_bytes());
}

fn init_copy(copy: PhpHandle<'_>, source: PhpHandle<'_>, easy: Easy<'_>) {
    swoole_curl_alloc_handlers(copy);
    swoole_curl_share_lists(copy, source);
    swoole_curl_set_easy(copy, easy);
    super::callbacks::copy_handlers(copy, source);
    *super::setopt::swoole_curl_private(copy) = super::setopt::swoole_curl_private(source).copy();
}

pub(super) fn pushed_object(
    easy: Easy<'_>,
    parent: PhpHandle<'_>,
) -> Option<swoole_rs_zend::Owned> {
    let owner = swoole_curl_easy(parent).handle()?;
    let mut object = swoole_rs_zend::Owned::default();
    let child = swoole_curl_new_handle(&mut object);
    init_copy(child, parent, easy);
    crate::Handle::create(easy, owner.multi_ref());
    Some(object)
}

fn clone_object(source: PhpHandle<'_>) -> swoole_rs_zend::Owned {
    let mut object = swoole_rs_zend::Owned::default();
    let clone = swoole_curl_new_clone(&mut object);
    let raw = curl_easy_duphandle(swoole_curl_easy(source));
    let copied = !raw.is_null() && {
        let easy = Easy(raw, PhantomData);
        init_copy(clone, source, easy);
        crate::swr_v1_curl_create_handle(easy);
        let mut ok = false;
        let result = swoole_rs_zend::contain_with(|| {
            ok = super::postfields::copy(clone, source);
        });
        if result.is_err() {
            swoole_rs_zend::bailout::defer(result);
            return object;
        }
        ok
    };
    if !copied {
        swoole_rs_zend::bailout::defer(swoole_rs_zend::throw_trapped(
            None,
            c"Failed to clone CurlHandle",
            0,
        ));
    }
    object
}

fn copy_handle(call: &Call<'_>) {
    let Some(source) = argument(call, 1) else {
        return;
    };
    if !checked(source, true) {
        call.set_bool(false);
        return;
    }
    let object = clone_object(source);
    if PHP_VERSION_ID >= 80500 && !swoole_rs_zend::host::exception().is_null() {
        drop(object);
        swoole_rs_zend::bailout::defer(swoole_rs_zend::contain_with(|| zend_clear_exception()));
        swoole_rs_zend::bailout::defer(swoole_rs_zend::report(
            swoole_rs_zend::E_WARNING,
            c"Cannot duplicate cURL handle",
        ));
        call.set_bool(false);
    } else {
        call.set_owned(object);
    }
}

swoole_rs_zend::exports! {
/// Allocate the version-specific `php_curl` storage and initialize the embedded Zend object.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_curl_create_object(ce: swoole_rs_zend::ClassEntry<'_>) -> *mut zend_object {
    swr_zend_object_new(swoole_curl_object_size, swoole_curl_object_offset, ce, &swoole_coroutine_curl_handle_handlers)
}

/// An integer cast exposes the Zend object ID for both easy and multi handles.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_curl_cast_object(object: ObjectRef<'_>, result: &mut std::mem::MaybeUninit<zval>, kind: u32) -> zend_result {
    if kind == swoole_rs_zend::sys::IS_LONG {
        result.write(swoole_rs_zend::Owned::long(i64::from(object.handle())).into_zval());
        0
    } else {
        zend_std_cast_object_tostring(object, result, kind)
    }
}

/// Enumerate edges held outside Zend's standard object properties.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_curl_get_gc(object: ObjectRef<'_>, table: &mut std::mem::MaybeUninit<*mut swoole_rs_zend::sys::zval>, count: &mut std::mem::MaybeUninit<c_int>) -> *mut std::ffi::c_void {
    let gc = zend_get_gc_buffer_create();
    if let Some(handle) = swoole_curl_object_handle(object) {
        swoole_curl_trace_values(gc, handle);
        swoole_curl_trace_callbacks(gc, handle);
    }
    swoole_curl_publish_roots(gc, table, count);
    if PHP_VERSION_ID >= 80500 {
        std::ptr::null_mut()
    } else {
        zend_std_get_properties(object)
    }
}

/// Transfer the initial reference of the clone to Zend's object handler caller.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_curl_clone_obj(object: ObjectRef<'_>) -> *mut swoole_rs_zend::sys::zend_object {
    let Some(source) = swoole_curl_object_handle(object) else { return std::ptr::null_mut() };
    let clone = clone_object(source);
    let result = clone.object().unwrap_or(std::ptr::null_mut());
    let _transferred = clone.into_zval();
    result
}

/// Zend invokes this handler on direct construction of CurlHandle.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_curl_get_constructor(_object: ObjectRef<'_>) -> *mut swoole_rs_zend::sys::zend_function {
    swoole_rs_zend::throw_error(c"Cannot directly construct CurlHandle, use curl_init() instead");
    std::ptr::null_mut()
}
}

swoole_rs_zend::exports! {
    preserve_pending:
/// Zend has finished using this object; no transfer can still own its easy handle.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_curl_free_obj(object: ObjectRef<'_>) {
    let result = swoole_rs_zend::contain_with(|| {
        if let Some(handle) = swoole_curl_object_handle(object) {
            let easy = swoole_curl_easy(handle);
            if !easy.0.is_null() {
                if !swoole_curl_prepare_free(handle) {
                    return;
                }
                if let Some(owner) = easy.handle()
                    && let Some(multi) = owner.multi()
                {
                    multi.remove_handle(owner);
                }
            }
            // A denied push returns its easy handle to libcurl but keeps its PHP storage
            // until the last reference to the PHP object is released.
            if swoole_curl_has_clone_storage(handle) {
                if swoole_curl_release_clone(handle) {
                    swoole_curl_free_lists(handle);
                }
                if !easy.0.is_null() {
                    crate::swr_v1_curl_destroy_handle(crate::Retired(easy));
                    curl_easy_cleanup(crate::Retired(easy));
                }
                swoole_curl_free_callbacks(handle);
                swoole_curl_release_values(handle);
            }
        }
        zend_object_std_dtor(object);
    });
    swoole_rs_zend::bailout::defer(result);
}
}
