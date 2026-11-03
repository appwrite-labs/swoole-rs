//! Both classes' objects: `struct { swoole::http::Context *ctx; zend_object std; }`.
use crate::{
    host::Context,
    parse::{self, Property},
    send,
    state::{Slot, State},
};
use std::{
    ffi::c_void,
    sync::atomic::{AtomicU32, Ordering::Relaxed},
};
use swoole_rs_zend::{
    Class, Object, init_object,
    sys::{
        IS_NULL, IS_STRING, ZEND_ACC_NOT_SERIALIZABLE, swr_zend_efree,
        swr_zend_http_property_offset, swr_zend_string_release, zend_class_entry, zend_object,
        zend_object_handlers, zval, zval_ptr_dtor,
    },
    type_of, unset_property_deny, value,
    value::{obj_of, set_long},
};

pub(crate) type Holder = Object<*mut Context>;

/// The context of an object of either class (null once released or before
/// one is set).
/// # Safety
/// `object` is a live `Swoole\Http\Request` or `Swoole\Http\Response`.
#[must_use]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_context(object: *const zend_object) -> *mut Context {
    // SAFETY: per the caller; only the pointer is copied out.
    unsafe { *Holder::data(object.cast_mut()) }
}

/// Point an object of either class at `context`.
/// # Safety
/// As `swr_v1_phphttp_context`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_set_context(
    object: *mut zend_object,
    context: *mut Context,
) {
    // SAFETY: per the caller.
    unsafe { *Holder::data(object) = context };
}

/// The method's `$this` context.
pub(crate) fn this(call: &swoole_rs_zend::Call) -> *mut Context {
    // SAFETY: the classes' methods are non-static, so `$this` is one of ours.
    unsafe { swr_v1_phphttp_context(call.this()) }
}

/// `SW_SET_CLASS_NOT_SERIALIZABLE`, clone and unset denied, custom object.
pub(crate) fn configure(
    entry: &mut zend_class_entry,
    handlers: &mut zend_object_handlers,
    create: unsafe extern "C" fn(*mut zend_class_entry) -> *mut zend_object,
    free: unsafe extern "C" fn(*mut zend_object),
) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    Holder::install(entry, handlers, create, free);
}

/// The body of both `create_object` handlers.
/// # Safety
/// As `Object::create`.
pub(crate) unsafe fn create(class: &Class, ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: per the caller; a null context is valid zeroed data.
    unsafe { Holder::create(class, ce) }
}

/// `Swoole\Http\Response`'s declared `fd` slot (MINIT).
static RESPONSE_FD: AtomicU32 = AtomicU32::new(0);

/// MINIT, once `Swoole\Http\Response` is registered.
pub(crate) fn response_minit(ce: *mut zend_class_entry) {
    // SAFETY: the registered class, which declares `fd`.
    RESPONSE_FD.store(
        unsafe { swr_zend_http_property_offset(ce, c"fd".as_ptr(), 2) },
        Relaxed,
    );
}

/// A new object of `ce` in `slot`, pointing at `ctx`, with its `fd` property
/// (the slot at `offset`) set.
fn object(slot: &mut Slot, ce: *mut zend_class_entry, ctx: *mut Context, offset: u32, fd: i64) {
    // SAFETY: the context's own zval and a registered class.
    unsafe { init_object(&raw mut slot.value, ce) };
    slot.ptr = &raw mut slot.value;
    // SAFETY: the object just created.
    let std = unsafe { obj_of(slot.ptr) };
    // SAFETY: one of our objects.
    unsafe { swr_v1_phphttp_set_context(std, ctx) };
    // SAFETY: the declared property's slot, `OBJ_PROP(std, offset)`.
    let property = unsafe { std.byte_add(offset as usize) }.cast::<zval>();
    // SAFETY: a fresh object's property, holding nothing to release.
    unsafe { set_long(property, fd) };
}

/// `swoole_http_context_new` after `new Context()`: the request and response
/// objects, both pointing at `ctx` and holding `fd`, and the request's
/// `server` and `header` arrays.
/// # Safety
/// `ctx` is a new, value-initialised context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_context_objects(ctx: *mut Context, fd: i64) {
    let state = send::state(ctx);
    object(
        &mut state.request.zobject,
        crate::request::entry(),
        ctx,
        parse::fd_offset(),
        fd,
    );
    object(
        &mut state.response.zobject,
        crate::response::entry(),
        ctx,
        RESPONSE_FD.load(Relaxed),
        fd,
    );
    parse::property(state, Property::Server);
    parse::property(state, Property::Header);
    state.fd = fd;
}

unsafe extern "C" {
    /// `php_swoole_socket_is_closed`.
    fn swr_http_host_socket_closed(socket: *const zval) -> bool;
    /// The server still holds the session's connection, neither closed nor
    /// closed by the peer.
    fn swr_http_host_connection_open(server: *mut c_void, fd: i64) -> bool;
    /// Free what C++ owns (the write and HTTP/2 data buffers) and the context.
    fn swr_http_host_context_delete(ctx: *mut Context);
}

/// `Context::is_co_socket`: bound to a coroutine socket rather than a server.
pub(crate) fn co_socket(state: &State) -> bool {
    // SAFETY: the context's own zval.
    unsafe { type_of(&raw const state.zsocket) != IS_NULL }
}

/// `Context::is_available`: the response can still reach its peer.
pub(crate) fn available(ctx: *mut Context) -> bool {
    let state = send::state(ctx);
    if state.response.zobject.ptr.is_null() {
        return false;
    }
    if co_socket(state) {
        // SAFETY: the context's live socket zval.
        return !unsafe { swr_http_host_socket_closed(&raw const state.zsocket) };
    }
    // SAFETY: the server the context is bound to.
    unsafe { swr_http_host_connection_open(state.private_data, state.fd) }
}

/// `Context::free`: release the context once neither object nor an HTTP/2
/// stream holds it.
/// # Safety
/// `ctx` is a live context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_context_free(ctx: *mut Context) {
    let state = send::state(ctx);
    if !state.request.zobject.ptr.is_null()
        || !state.response.zobject.ptr.is_null()
        || state.stream_id > 0
    {
        return;
    }
    if !state.request.path.is_null() {
        // SAFETY: the path the parser copied into request memory.
        unsafe { swr_zend_efree(state.request.path.cast()) };
    }
    // SAFETY: the context's own zval.
    if unsafe { type_of(&raw const state.request.zdata) } == IS_STRING {
        // SAFETY: a string zval.
        let data = unsafe { value::str_of(&raw const state.request.zdata) };
        // SAFETY: the received data the context holds.
        unsafe { swr_zend_string_release(data) };
    }
    if !state.response.reason.is_null() {
        // SAFETY: the reason `status()` copied into request memory.
        unsafe { swr_zend_efree(state.response.reason.cast()) };
    }
    // SAFETY: a live context.
    unsafe { parse::swr_v1_phphttp_parse_free(ctx) };
    let state = send::state(ctx);
    if co_socket(state) {
        // SAFETY: the context's reference to its socket.
        unsafe { zval_ptr_dtor(&raw mut state.zsocket) };
    }
    // SAFETY: nothing references the context any more.
    unsafe { swr_http_host_context_delete(ctx) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn std_follows_the_context_pointer_as_in_upstream() {
        // `struct HttpRequestObject { HttpContext *ctx; zend_object std; }`
        assert_eq!(
            Holder::OFFSET,
            size_of::<*mut Context>(),
            "std right after ctx"
        );
    }
}
