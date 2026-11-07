//! Both classes' objects: `struct { swoole::http::Context *ctx; zend_object std; }`.
use crate::{
    host::Context,
    parse::{self, Property},
    send,
    state::{Slot, State},
};
use std::{
    borrow::Cow,
    collections::HashSet,
    ffi::{CStr, CString, c_char, c_int, c_void},
    ptr,
    rc::Rc,
    sync::atomic::{AtomicU32, Ordering::Relaxed},
};
use swoole_rs_phpruntime::co;
use swoole_rs_server::{server_methods, view};
use swoole_rs_zend::string;
use swoole_rs_zend::{
    Class, Object, bailout, init_object,
    sys::{
        IS_NULL, IS_STRING, ZEND_ACC_NOT_SERIALIZABLE, swr_zend_efree,
        swr_zend_http_property_offset, swr_zend_string_release, zend_class_entry, zend_object,
        zend_object_handlers, zend_string, zval, zval_ptr_dtor,
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

/// `swoole_http_context_new`: a new context with its request and response
/// objects, both pointing at it and holding `fd`, and the request's `server`
/// and `header` arrays.
#[cfg_attr(
    target_os = "macos",
    unsafe(export_name = "_Z23swoole_http_context_newx")
)]
#[cfg_attr(
    not(target_os = "macos"),
    unsafe(export_name = "_Z23swoole_http_context_newl")
)]
pub(crate) extern "C" fn context_new(fd: i64) -> *mut Context {
    let ctx = swr_v1_phphttp_context_new();
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
    ctx
}

unsafe extern "C" {
    /// crates/phpclients' `php_swoole_socket_is_closed`.
    #[link_name = "_Z27php_swoole_socket_is_closedPK12_zval_struct"]
    fn php_swoole_socket_is_closed(socket: *const zval) -> bool;
}

/// `new Context()`: a value-initialised context, which
/// `swr_v1_phphttp_context_free` releases.
#[must_use]
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phphttp_context_new() -> *mut Context {
    // SAFETY: all zeroes is a valid `State`: null pointers, `None`s and
    // `IS_UNDEF` zvals, as C++'s value-initialisation left it.
    let state: Box<State> = unsafe { Box::new_zeroed().assume_init() };
    Box::into_raw(state).cast()
}

/// `Context::init(Server *)`: the server's request settings, then bound to it.
/// # Safety
/// `ctx` is a live context and `server` the live server.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_context_init(ctx: *mut Context, server: *mut c_void) {
    // SAFETY: per the caller.
    let config = unsafe { swoole_rs_server::view::config(server) };
    let state = send::state(ctx);
    state.parse_cookie = u8::from(config.http_parse_cookie);
    state.parse_body = u8::from(config.http_parse_post);
    state.parse_files = u8::from(config.http_parse_files);
    if parse::CAPABILITIES.load(Relaxed) & 8 != 0 {
        state.enable_compression = u8::from(config.http_compression);
        state.compression_level = i8::from_ne_bytes([config.http_compression_level]);
        state.compression_min_length = config.compression_min_length;
    }
    // SAFETY: per the caller.
    unsafe { swr_v1_phphttp_context_bind(ctx, server, ptr::null_mut()) };
}

/// `Context::is_co_socket`: bound to a coroutine socket rather than a server.
pub(crate) fn co_socket(state: &State) -> bool {
    // SAFETY: the context's own zval.
    unsafe { type_of(&raw const state.zsocket) != IS_NULL }
}

/// `ctx->send`: the server's send to the session, or the coroutine socket's.
pub(crate) fn send(ctx: *mut Context, data: &[u8]) -> bool {
    send_raw(ctx, data.as_ptr().cast(), data.len())
}

/// `send` of `length` bytes at `data`, which may be the context's own buffer.
pub(crate) fn send_raw(ctx: *mut Context, data: *const c_char, length: usize) -> bool {
    let state = send::state(ctx);
    let (target, fd) = (state.private_data, state.fd);
    if co_socket(state) {
        // SAFETY: the context's live socket and `length` bytes; true once
        // every byte is sent.
        unsafe { co::send_all(target, data, length) }.cast_unsigned() == length
    } else {
        // SAFETY: the context's live server and `length` bytes.
        unsafe { server_send(target, fd, data, length) }
    }
}

/// `SW_ERROR_OUTPUT_SEND_YIELD`.
const OUTPUT_SEND_YIELD: c_int = 1010;

unsafe extern "C" {
    /// crates/phpserver's wait for the session's buffer to drain, then send:
    /// 0 false, 1 true, 2 bailed out.
    fn swr_v1_phpserver_send_yield(
        server: *mut c_void,
        session_id: i64,
        data: *const c_char,
        length: usize,
    ) -> c_int;
}

/// `Server::send` to `fd`, waiting in a coroutine while the socket's buffer
/// is full. A bailout while waiting is left for the handler to raise.
/// # Safety
/// `server` is the live server and `data` holds `length` bytes.
pub(crate) unsafe fn server_send(
    server: *mut c_void,
    fd: i64,
    data: *const c_char,
    length: usize,
) -> bool {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Server::send's uint32_t length"
    )]
    // SAFETY: per the caller.
    let sent = unsafe { server_methods::send(server, fd, data.cast(), length as u32) };
    if sent || swoole_rs_corebase::log::last_error() != OUTPUT_SEND_YIELD {
        return sent;
    }
    // SAFETY: per the caller.
    match unsafe { swr_v1_phpserver_send_yield(server, fd, data, length) } {
        1 => true,
        2 => {
            bailout::defer(Err(bailout::Bailout));
            false
        }
        _ => false,
    }
}

/// `ctx->sendfile`.
pub(crate) fn sendfile(
    ctx: *mut Context,
    file: *mut zend_string,
    offset: i64,
    length: usize,
) -> bool {
    let state = send::state(ctx);
    let (target, fd) = (state.private_data, state.fd);
    if co_socket(state) {
        // SAFETY: the context's live socket and a live, NUL-terminated file name.
        unsafe {
            co::sendfile(
                target,
                string::val(file).cast(),
                offset,
                i64::try_from(length).unwrap_or(i64::MAX),
            )
        }
    } else {
        // SAFETY: a live file name.
        let name = unsafe { swoole_rs_zend::string::bytes(file) };
        // SAFETY: the context's live server and the file name's bytes.
        unsafe {
            server_methods::sendfile(
                target,
                fd,
                name.as_ptr().cast(),
                u32::try_from(name.len()).unwrap_or(u32::MAX),
                offset,
                length,
            )
        }
    }
}

/// `ctx->close`.
pub(crate) fn close(ctx: *mut Context) -> bool {
    let state = send::state(ctx);
    let (target, fd) = (state.private_data, state.fd);
    if co_socket(state) {
        // SAFETY: the context's live socket.
        unsafe { co::close(target) }
    } else {
        // SAFETY: the context's live server.
        unsafe { server_methods::close(target, fd, false) }
    }
}

/// `ctx->send(ctx, data, length)` for C++.
/// # Safety
/// `ctx` is a live, bound context and `data` holds `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_send(
    ctx: *mut Context,
    data: *const c_char,
    length: usize,
) -> bool {
    send_raw(ctx, data, length)
}

/// `ctx->close(ctx)` for C++.
/// # Safety
/// `ctx` is a live, bound context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_close(ctx: *mut Context) -> bool {
    close(ctx)
}

/// `Context::bind(Server *)`, or `bind(zval *)` when `server` is null: the
/// context sends through the server, or holds the socket object and sends
/// through its coroutine socket.
/// # Safety
/// `ctx` is a live context; `server` is the live server or `socket` a live
/// `Swoole\Coroutine\Socket`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_context_bind(
    ctx: *mut Context,
    server: *mut c_void,
    socket: *mut zval,
) {
    let state = send::state(ctx);
    if server.is_null() {
        // SAFETY: per the caller; the context takes a reference.
        unsafe { copy_zval(&raw mut state.zsocket, socket) };
        // SAFETY: per the caller.
        state.private_data = unsafe { swoole_rs_phpclients::php_swoole_get_socket(socket) };
    } else {
        state.private_data = server;
        // SAFETY: the context's own zval; a server's context holds no socket.
        unsafe { set_null(&raw mut state.zsocket) };
    }
}

/// `ZVAL_COPY`.
/// # Safety
/// `target` is writable and `source` live.
unsafe fn copy_zval(target: *mut zval, source: *const zval) {
    // SAFETY: per the caller.
    unsafe { swoole_rs_zend::sys::swr_zval_copy(target, source) };
}

/// `ZVAL_NULL`.
/// # Safety
/// `target` is writable and holds nothing to release.
unsafe fn set_null(target: *mut zval) {
    // SAFETY: per the caller; the type carries no value.
    unsafe {
        (*target).u1 = swoole_rs_zend::sys::_zval_struct__bindgen_ty_1 { type_info: IS_NULL };
    }
}

/// `Context::copy`: an HTTP/2 stream's context takes its connection's
/// settings, transport and hooks.
/// # Safety
/// `ctx` and `from` are live contexts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_context_copy(ctx: *mut Context, from: *const Context) {
    // SAFETY: a live context, read only.
    let from = unsafe { &*from.cast::<State>() };
    let state = send::state(ctx);
    state.parse_cookie = from.parse_cookie;
    state.parse_body = from.parse_body;
    state.parse_files = from.parse_files;
    state.enable_compression = from.enable_compression;
    state.compression_level = from.compression_level;
    state.compression_min_length = from.compression_min_length;
    state.compression_types.clone_from(&from.compression_types);
    state.upload_tmp_dir.clone_from(&from.upload_tmp_dir);
    state.websocket_settings = from.websocket_settings;
    // SAFETY: the copy holds one more reference to the socket (if any).
    unsafe { copy_zval(&raw mut state.zsocket, &raw const from.zsocket) };
    state.private_data = from.private_data;
    state.before_request = from.before_request;
    state.after_response = from.after_response;
}

/// `upload_tmp_dir`: the coroutine server's or `Request::create`'s, else
/// the server's, else `/tmp`.
pub(crate) fn upload_tmp_dir(state: &State) -> Cow<'_, CStr> {
    if let Some(directory) = &state.upload_tmp_dir {
        return Cow::Borrowed(directory);
    }
    if co_socket(state) || state.private_data.is_null() {
        return Cow::Borrowed(c"/tmp");
    }
    // SAFETY: the context's live server.
    Cow::Owned(unsafe { view::upload_tmp_dir(state.private_data) })
}

/// A directory as `std::string::c_str()` reads it: up to its first NUL.
pub(crate) fn directory(bytes: &[u8]) -> Rc<CString> {
    let text = bytes.split(|&byte| byte == 0).next().unwrap_or_default();
    Rc::new(CString::new(text).unwrap_or_default())
}

/// A coroutine server's `http_compression_types`.
pub(crate) type CompressionTypes = Rc<HashSet<Box<[u8]>>>;

/// `compression_types` lists `kind`; `None` asks whether it lists any.
pub(crate) fn compression_type(state: &State, kind: Option<&[u8]>) -> bool {
    if let Some(types) = &state.compression_types {
        return kind.is_none_or(|kind| types.contains(kind));
    }
    if co_socket(state) || state.private_data.is_null() {
        return false;
    }
    // SAFETY: the context's live server.
    unsafe { view::compression_type(state.private_data, kind) }
}

/// `onBeforeRequest`: a server's request is counted, or queued past
/// `worker_max_concurrency` (false: not now).
/// # Safety
/// `ctx` is a live context.
pub(crate) unsafe fn before_request(ctx: *mut Context) -> bool {
    let state = send::state(ctx);
    if state.before_request == 0 {
        return true;
    }
    state.before_request = 0;
    state.after_response = 1;
    // SAFETY: per the caller.
    unsafe { crate::server::admit(ctx) }
}

/// `onAfterResponse`: the server uncounts the request and starts a queued one.
pub(crate) fn after_response(ctx: *mut Context) {
    let state = send::state(ctx);
    if state.after_response != 0 {
        state.after_response = 0;
        crate::server::release_request();
    }
}

/// `Context::is_available`: the response can still reach its peer.
pub(crate) fn available(ctx: *mut Context) -> bool {
    let state = send::state(ctx);
    if state.response.zobject.ptr.is_null() {
        return false;
    }
    if co_socket(state) {
        // SAFETY: the context's live socket zval.
        return !unsafe { php_swoole_socket_is_closed(&raw const state.zsocket) };
    }
    // SAFETY: the server the context is bound to.
    unsafe { view::by_session(state.private_data, state.fd) }
        .is_some_and(|conn| conn.closed.load(Relaxed) == 0 && conn.peer_closed == 0)
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
    // SAFETY: `swr_v1_phphttp_context_new`'s, which nothing references any more.
    drop(unsafe { Box::from_raw(ctx.cast::<State>()) });
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

    #[test]
    fn a_directory_ends_at_its_first_nul_as_c_str_reads_it() {
        assert_eq!(directory(b"/var/up\0loads").as_bytes(), b"/var/up");
        assert_eq!(directory(b"/tmp").as_bytes(), b"/tmp");
    }

    #[test]
    fn a_coroutine_servers_types_answer_without_the_server() {
        // A value-initialised context: no socket bound, no server.
        let mut state: State = unsafe { std::mem::zeroed() };
        assert!(!compression_type(&state, None));
        assert_eq!(*upload_tmp_dir(&state), *c"/tmp");
        state.compression_types = Some(Rc::new(HashSet::from([b"text/html".to_vec().into()])));
        assert!(compression_type(&state, None));
        assert!(compression_type(&state, Some(b"text/html")));
        assert!(!compression_type(&state, Some(b"text/plain")));
        state.compression_types = Some(Rc::default());
        assert!(compression_type(&state, None), "an empty list still lists");
        assert!(!compression_type(&state, Some(b"text/html")));
        state.upload_tmp_dir = Some(directory(b"/srv"));
        assert_eq!(*upload_tmp_dir(&state), *c"/srv");
    }
}
