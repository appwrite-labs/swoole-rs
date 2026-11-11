//! Both classes' objects: `struct { swoole::http::Context *ctx; zend_object std; }`.
use crate::{
    host::Context,
    parse::{self, Property},
    state::{Slot, State},
};
use std::{
    borrow::Cow,
    collections::HashSet,
    ffi::{CStr, CString, c_int},
    ptr::NonNull,
    rc::Rc,
    sync::atomic::{AtomicU32, Ordering::Relaxed},
};
use swoole_rs_phpruntime::co;
use swoole_rs_server::{server_methods, view};
use swoole_rs_zend::string;
use swoole_rs_zend::{
    Object, Owned, Value,
    bailout::{self, Bailout},
    sys::{
        ZEND_ACC_NOT_SERIALIZABLE, swr_zend_efree, swr_zend_http_property_offset,
        swr_zend_zval_release, zend_class_entry, zend_object, zend_object_handlers, zend_string,
        zval,
    },
    unset_property_deny,
    value::set_long,
};

/// Either class's object: its context, none once released or before one is set.
pub(crate) type Holder = Object<Option<Ctx>>;

/// A live `swoole::http::Context`: the only way to reach its state. Only
/// [`Ctx::of`], whose callers promise the context lives as long as the handle
/// or a copy is used, and [`alloc`] make one; an object holds one until it
/// lets go of it. What is read and done over it is then safe, until [`free`]
/// ends it.
///
/// The handle is `Copy`, and a send can yield to a coroutine that reaches the
/// same context through its objects, so a [`Ctx::state`] borrow ends before
/// any host call, and before the handle (or a copy) is passed on.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct Ctx(NonNull<Context>);

impl Ctx {
    /// The handle of a context.
    /// # Safety
    /// `ctx` is a live context, which starts with its `ContextState`, for as
    /// long as the handle or a copy is used.
    pub(crate) const unsafe fn of(ctx: NonNull<Context>) -> Self {
        Self(ctx)
    }

    /// The context the handle names.
    pub(crate) fn raw(self) -> *mut Context {
        self.0.as_ptr()
    }

    /// The context's `ContextState`, its first base, for as long as the
    /// handle is borrowed.
    pub(crate) fn state(&mut self) -> &mut State {
        // SAFETY: `of`'s contract: a live context, which starts with its
        // `ContextState`; no other reference to it outlives a host call.
        unsafe { self.0.cast::<State>().as_mut() }
    }

    /// The context's state bits.
    pub(crate) fn flags(mut self) -> u32 {
        let state = self.state();
        u32::from(state.end)
            | u32::from(state.detached) << 1
            | u32::from(state.completed) << 2
            | u32::from(state.http2) << 3
            | u32::from(state.send_chunked) << 4
            | u32::from(co_socket(state)) << 5
            | u32::from(state.upgrade) << 6
            | u32::from(state.websocket) << 7
    }
}

/// The context of an object of either class (none once released or before
/// one is set).
/// # Safety
/// `object` is a live `Swoole\Http\Request` or `Swoole\Http\Response`.
#[must_use]
pub(crate) unsafe fn context_of(object: *const zend_object) -> Option<Ctx> {
    // SAFETY: per the caller; only the handle is copied out.
    unsafe { *Holder::data(object.cast_mut()) }
}

/// The method's `$this` context.
pub(crate) fn this(call: &swoole_rs_zend::Call) -> Option<Ctx> {
    // SAFETY: the classes' methods are non-static, so `$this` is one of ours.
    unsafe { context_of(call.this()) }
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
pub(crate) fn new_object(slot: &mut Slot, kind: Kind, ctx: Ctx) -> Option<*mut zend_object> {
    let class = match kind {
        Kind::Request => &crate::request::REQUEST,
        Kind::Response => &crate::response::RESPONSE,
    };
    // The context keeps the object's reference.
    slot.value = class.create_object().into_zval();
    slot.ptr = &raw mut slot.value;
    let std = slot.object()?;
    // SAFETY: one of our objects, laid out as a `Holder`.
    unsafe { *Holder::data(std) = Some(ctx) };
    Some(std)
}

/// A new object in `slot`, pointing at `ctx`, with its `fd` property (the
/// slot at `offset`) set.
fn object(slot: &mut Slot, kind: Kind, ctx: Ctx, offset: u32, fd: i64) {
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
pub(crate) fn context_new(fd: i64) -> Ctx {
    let mut ctx = alloc();
    // The objects' copies, which nothing reads until the context is filled.
    let held = ctx;
    let state = ctx.state();
    object(
        &mut state.request.zobject,
        Kind::Request,
        held,
        parse::fd_offset(),
        fd,
    );
    object(
        &mut state.response.zobject,
        Kind::Response,
        held,
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
pub(crate) fn alloc() -> Ctx {
    // Not `Box::new_zeroed`: glibc's `calloc` skips the thread cache and cost
    // 500 instructions a request where `malloc` and a memset cost about 100
    // (#2129). `black_box` keeps LLVM from folding them back into `calloc`.
    let state = std::hint::black_box(NonNull::from(Box::leak(Box::<State>::new_uninit())));
    // SAFETY: a fresh, writable `State`; all zeroes is a valid one (null
    // pointers, `None`s and `IS_UNDEF` zvals, as upstream's value-initialisation
    // left it), which `free` drops as a `Box<State>`.
    unsafe { state.write_bytes(0, 1) };
    // SAFETY: the context just made, live until `free`.
    unsafe { Ctx::of(state.cast()) }
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

/// The server a context not bound to a coroutine socket answers through,
/// which outlives the borrow of its state: the worker's server serves every
/// request it binds.
pub(crate) fn server(state: &State) -> view::Server<'static> {
    // SAFETY: a server-bound context's `private_data` is the worker's live server.
    unsafe { view::Server::from_raw(state.private_data) }
}

/// `Context::is_co_socket`: bound to a coroutine socket rather than a server.
pub(crate) fn co_socket(state: &State) -> bool {
    !state.zsocket.is_null()
}

/// `ctx->send`: the server's send to the session, or the coroutine socket's.
pub(crate) fn send(mut ctx: Ctx, data: &[u8]) -> bool {
    let state = ctx.state();
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
pub(crate) fn sendfile(mut ctx: Ctx, file: *mut zend_string, offset: i64, length: usize) -> bool {
    let state = ctx.state();
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
pub(crate) fn close(mut ctx: Ctx) -> bool {
    let state = ctx.state();
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
/// `worker_max_concurrency` (false: not now; the queue keeps the handle,
/// whose context its objects keep live until the queued request runs).
pub(crate) fn before_request(mut ctx: Ctx) -> bool {
    let state = ctx.state();
    if state.before_request == 0 {
        return true;
    }
    state.before_request = 0;
    state.after_response = 1;
    crate::server::admit(ctx)
}

/// `onAfterResponse`: the server uncounts the request and starts a queued one.
pub(crate) fn after_response(mut ctx: Ctx) {
    let state = ctx.state();
    if state.after_response != 0 {
        state.after_response = 0;
        crate::server::release_request();
    }
}

/// `Context::is_available`: the response can still reach its peer.
pub(crate) fn available(mut ctx: Ctx) -> bool {
    let state = ctx.state();
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
/// A property's `__destruct` can run PHP code, so each is released inside
/// `zend_try`; both are, so the context is freed, and a bailout is deferred.
/// # Errors
/// A destructor bailed out, or a bailout is pending.
pub(crate) fn release_objects(mut ctx: Ctx) -> Result<(), Bailout> {
    let request = ctx.state().request.zobject.ptr;
    // SAFETY: the context's live request object, released once; the shim's
    // setjmp sits between this frame and its destructors.
    bailout::defer(unsafe { bailout::trap(swr_zend_zval_release, request.cast()) });
    let response = ctx.state().response.zobject.ptr;
    // SAFETY: as above, for the response.
    bailout::defer(unsafe { bailout::trap(swr_zend_zval_release, response.cast()) });
    bailout::outcome(())
}

/// `Context::free`: release the context once neither object nor an HTTP/2
/// stream holds it.
/// # Safety
/// `ctx` is a live context.
pub(crate) unsafe fn free(mut ctx: Ctx) {
    let state = ctx.state();
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
    drop(unsafe { Box::from_raw(ctx.raw().cast::<State>()) });
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
