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
    ffi::{CStr, CString, c_int},
    rc::Rc,
    sync::atomic::{AtomicU32, Ordering::Relaxed},
};
use swoole_rs_phpruntime::co;
use swoole_rs_server::{server_methods, view};
use swoole_rs_zend::string;
use swoole_rs_zend::{
    Owned, Value, init_object,
    sys::{
        swr_zend_efree, swr_zend_http_property_offset, zend_class_entry, zend_object, zend_string,
        zval, zval_ptr_dtor,
    },
    value::set_long,
};

/// What a `Swoole\Http\Request` or `Swoole\Http\Response` object embeds
/// (`struct { swoole::http::Context *ctx; zend_object std; }`): its context,
/// null once released or before one is set.
pub(crate) type Holder = std::cell::Cell<*mut Context>;

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

/// The class of a context's object.
#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Request,
    Response,
}

/// A new `Swoole\Http\Request` or `Response` in the context's `slot`,
/// pointing at `ctx`.
pub(crate) fn new_object(
    slot: &mut Slot,
    kind: Kind,
    ctx: *mut Context,
) -> Option<*mut zend_object> {
    let ce = match kind {
        Kind::Request => crate::request::entry(),
        Kind::Response => crate::response::entry(),
    };
    // SAFETY: the context's own zval and a registered class.
    unsafe { init_object(&raw mut slot.value, ce) };
    slot.ptr = &raw mut slot.value;
    let object = slot.get()?;
    match kind {
        Kind::Request => object.custom::<crate::request::Data>()?.0.set(ctx),
        Kind::Response => object.custom::<crate::response::Data>()?.0.set(ctx),
    }
    slot.object()
}

/// A new object in `slot`, pointing at `ctx`, with its `fd` property (the
/// slot at `offset`) set.
fn object(slot: &mut Slot, kind: Kind, ctx: *mut Context, offset: u32, fd: i64) {
    let Some(std) = new_object(slot, kind, ctx) else {
        return;
    };
    // SAFETY: the declared property's slot, `OBJ_PROP(std, offset)`.
    let property = unsafe { std.byte_add(offset as usize) }.cast::<zval>();
    // SAFETY: a fresh object's property, holding nothing to release.
    unsafe { set_long(property, fd) };
}

/// `swoole_http_context_new`: a new context with its request and response
/// objects, both pointing at it and holding `fd`, and the request's `server`
/// and `header` arrays.
pub(crate) fn context_new(fd: i64) -> *mut Context {
    let ctx = alloc();
    let state = send::state(ctx);
    object(
        &mut state.request.zobject,
        Kind::Request,
        ctx,
        parse::fd_offset(),
        fd,
    );
    object(
        &mut state.response.zobject,
        Kind::Response,
        ctx,
        RESPONSE_FD.load(Relaxed),
        fd,
    );
    parse::property(state, Property::Server);
    parse::property(state, Property::Header);
    state.fd = fd;
    ctx
}

/// `new Context()`: a value-initialised context, which
/// `free` releases.
#[must_use]
pub(crate) fn alloc() -> *mut Context {
    // Not `Box::new_zeroed`: glibc's `calloc` skips the thread cache and cost
    // 500 instructions a request where `malloc` and a memset cost about 100
    // (#2129). `black_box` keeps LLVM from folding them back into `calloc`.
    let state = std::hint::black_box(Box::into_raw(Box::<State>::new_uninit()));
    // SAFETY: a fresh, writable `State`; all zeroes is a valid one (null
    // pointers, `None`s and `IS_UNDEF` zvals, as upstream's value-initialisation
    // left it), which `free` drops as a `Box<State>`.
    unsafe { state.write_bytes(0, 1) };
    state.cast()
}

/// `Context::init(Server *)`: the server's request settings, then bound to it.
pub(crate) fn init(state: &mut State, server: view::Server<'_>) {
    let config = server.config();
    state.parse_cookie = u8::from(config.http_parse_cookie);
    state.parse_body = u8::from(config.http_parse_post);
    state.parse_files = u8::from(config.http_parse_files);
    if parse::CAPABILITIES.load(Relaxed) & 8 != 0 {
        state.enable_compression = u8::from(config.http_compression);
        state.compression_level = i8::from_ne_bytes([config.http_compression_level]);
        state.compression_min_length = config.compression_min_length;
    }
    bind_server(state, server);
}

/// The server a context not bound to a coroutine socket answers through.
pub(crate) fn server(state: &State) -> view::Server<'_> {
    // SAFETY: a server-bound context's `private_data` is its live server.
    unsafe { view::Server::from_raw(state.private_data) }
}

/// `Context::is_co_socket`: bound to a coroutine socket rather than a server.
pub(crate) fn co_socket(state: &State) -> bool {
    !state.zsocket.is_null()
}

/// `ctx->send`: the server's send to the session, or the coroutine socket's.
pub(crate) fn send(ctx: *mut Context, data: &[u8]) -> bool {
    let state = send::state(ctx);
    let (target, fd) = (state.private_data, state.fd);
    if co_socket(state) {
        // SAFETY: the context's live socket; true once every byte is sent.
        unsafe { co::Co::of(target) }.send_all(data).cast_unsigned() == data.len()
    } else {
        server_send(server(state), fd, data)
    }
}

/// `SW_ERROR_OUTPUT_SEND_YIELD`.
const OUTPUT_SEND_YIELD: c_int = 1010;

/// `Server::send` to `fd`, waiting in a coroutine while the socket's buffer
/// is full. A bailout while waiting is left for the handler to raise.
pub(crate) fn server_send(server: view::Server<'_>, fd: i64, data: &[u8]) -> bool {
    let sent = server_methods::send(server, fd, data);
    if sent || swoole_rs_corebase::log::last_error() != OUTPUT_SEND_YIELD {
        return sent;
    }
    swoole_rs_phpserver::php_swoole_server_send_yield(server, fd, data).unwrap_or_else(|bailed| {
        bailed.defer();
        false
    })
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
        // SAFETY: the context's live socket.
        let socket = unsafe { co::Co::of(target) };
        // SAFETY: a live, NUL-terminated file name.
        let file = unsafe { std::ffi::CStr::from_ptr(string::val(file).cast()) };
        socket.sendfile(file, offset, length)
    } else {
        // SAFETY: a live file name.
        let name = unsafe { swoole_rs_zend::string::bytes(file) };
        server_methods::sendfile(server(state), fd, name, offset, length)
    }
}

/// `ctx->close`.
pub(crate) fn close(ctx: *mut Context) -> bool {
    let state = send::state(ctx);
    let (target, fd) = (state.private_data, state.fd);
    if co_socket(state) {
        // SAFETY: the context's live socket.
        unsafe { co::Co::of(target) }.close()
    } else {
        server_methods::close(server(state), fd, false)
    }
}

/// `Context::bind(Server *)`: the context sends through the server.
pub(crate) fn bind_server(state: &mut State, server: view::Server<'_>) {
    state.private_data = server.raw();
    // A server's context holds no socket.
    state.zsocket = Owned::null();
}

/// `Context::bind(zval *)`: the context holds the socket object and sends
/// through its coroutine socket.
/// # Safety
/// `socket` is a live `Swoole\Coroutine\Socket`.
pub(crate) unsafe fn bind(state: &mut State, socket: &Value) {
    // SAFETY: per the caller.
    state.private_data = unsafe { swoole_rs_phpclients::get_socket(socket.as_ptr()) };
    // The context takes a reference.
    state.zsocket = socket.copy();
}

/// `Context::copy`: an HTTP/2 stream's context takes another context's
/// settings, transport and hooks.
pub(crate) fn copy_settings(state: &mut State, from: &State) {
    state.parse_cookie = from.parse_cookie;
    state.parse_body = from.parse_body;
    state.parse_files = from.parse_files;
    state.enable_compression = from.enable_compression;
    state.compression_level = from.compression_level;
    state.compression_min_length = from.compression_min_length;
    state.compression_types.clone_from(&from.compression_types);
    state.upload_tmp_dir.clone_from(&from.upload_tmp_dir);
    state.websocket_settings = from.websocket_settings;
    state.zsocket = from.zsocket.copy();
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
    Cow::Owned(crate::context::server(state).upload_tmp_dir())
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
    crate::context::server(state).compression_type(kind)
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
        return !unsafe { swoole_rs_phpclients::is_socket_closed(state.zsocket.as_ptr()) };
    }
    crate::context::server(state)
        .by_session(state.fd)
        .is_some_and(|conn| conn.closed.load(Relaxed) == 0 && conn.peer_closed == 0)
}

/// The context's own references to its request and response objects,
/// released once it is handled. Their free handlers clear the slots, and the
/// second frees the context, so each pointer is read before its release.
pub(crate) fn release_objects(ctx: *mut Context) {
    let request = send::state(ctx).request.zobject.ptr;
    // SAFETY: the context's live request object, released once.
    unsafe { zval_ptr_dtor(request) };
    let response = send::state(ctx).response.zobject.ptr;
    // SAFETY: as above, for the response.
    unsafe { zval_ptr_dtor(response) };
}

/// `Context::free`: release the context once neither object nor an HTTP/2
/// stream holds it.
/// # Safety
/// `ctx` is a live context.
pub(crate) unsafe fn free(ctx: *mut Context) {
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
    if !state.response.reason.is_null() {
        // SAFETY: the reason `status()` copied into request memory.
        unsafe { swr_zend_efree(state.response.reason.cast()) };
    }
    parse::parse_free(state);
    // SAFETY: `alloc`'s, which nothing references any more; the drop
    // releases the request's data, the socket and any unfinished upload.
    drop(unsafe { Box::from_raw(ctx.cast::<State>()) });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn std_follows_the_context_pointer_as_in_upstream() {
        // `struct HttpRequestObject { HttpContext *ctx; zend_object std; }`
        assert_eq!(
            swoole_rs_zend::Object::<crate::request::Data>::OFFSET,
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
        // A value-initialised context: no socket bound, no server. Not
        // dropped: releasing its zvals needs Zend.
        let mut state = std::mem::ManuallyDrop::new(unsafe { std::mem::zeroed::<State>() });
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
