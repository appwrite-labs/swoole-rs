//! Both classes' objects: `struct { swoole::http::Context *ctx; zend_object std; }`.
use crate::{
    host::Context,
    parse::{self, Property},
    state::{Slot, State},
};
use std::{
    borrow::Cow,
    cell::UnsafeCell,
    collections::HashSet,
    ffi::{CStr, CString, c_int},
    mem::MaybeUninit,
    ptr,
    rc::Rc,
    sync::atomic::Ordering::Relaxed,
};
use swoole_rs_corebase::{HookType, swoole_call_hook};
use swoole_rs_phpruntime::co;
use swoole_rs_server::{server_methods, view};
use swoole_rs_zend::{
    Call, Declared, Owned, Value, ZStr,
    bailout::{self, Bailout},
    host::Then,
    sys::{swr_zend_http_release_objects, zend_object, zval},
};

/// What a `Swoole\Http\Request` or `Swoole\Http\Response` object embeds
/// (`struct { swoole::http::Context *ctx; zend_object std; }`): its hold on
/// its context, set once when the context makes the object and let go when
/// the object is freed, so a method's `$this` lends it for the call.
pub(crate) type Holder = std::cell::OnceCell<Hold>;

/// A hold on a `swoole::http::Context`: its request and response objects, an
/// HTTP/2 stream or connection, a queued request and the code working on it
/// each have one, and the context goes with the last. Only [`alloc`] makes the
/// first, of a fresh context.
#[derive(Clone)]
pub struct Hold(Rc<UnsafeCell<State>>);

impl Hold {
    /// The context, for as long as this hold is borrowed.
    #[must_use]
    pub fn ctx(&self) -> Ctx<'_> {
        Ctx(self)
    }
}

/// A `swoole::http::Context` a [`Hold`] keeps live: the only way to reach its
/// state, safe to read and change for as long as the hold is borrowed.
///
/// The handle is `Copy`, and a send can yield to a coroutine that reaches the
/// same context through its objects, so a [`Ctx::state`] borrow ends before
/// any host call, and before the handle (or a copy) is passed on.
#[derive(Clone, Copy)]
pub struct Ctx<'a>(&'a Hold);

impl PartialEq for Ctx<'_> {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0.0, &other.0.0)
    }
}

impl Eq for Ctx<'_> {}

impl Ctx<'_> {
    /// The context the handle names.
    pub(crate) fn raw(self) -> *mut Context {
        self.0.0.get().cast()
    }

    /// Another hold on the context, to keep past this handle's borrow.
    #[must_use]
    pub fn hold(self) -> Hold {
        self.0.clone()
    }

    /// The context's `ContextState`, its first base, for as long as the
    /// handle is borrowed.
    pub(crate) fn state(&mut self) -> &mut State {
        // SAFETY: the hold the handle borrows keeps the state live; no other
        // reference to it outlives a host call.
        unsafe { self.0.0.as_mut_unchecked() }
    }

    /// The `kind` hook of Swoole's global hooks, which take the context as
    /// upstream passes them its `HttpContext *`.
    #[inline]
    pub(crate) fn call_hook(self, kind: Hook) {
        let kind = match kind {
            Hook::BeforeRequest => HookType::BEFORE_REQUEST,
            Hook::AfterResponse => HookType::AFTER_RESPONSE,
        };
        // SAFETY: the hooks of both kinds take a live context.
        unsafe { swoole_call_hook(kind, self.raw()) };
    }

    /// The context's state bits.
    pub(crate) fn flags(mut self) -> u32 {
        let state = self.state();
        u32::from(state.end)
            | (u32::from(state.detached) << 1)
            | (u32::from(state.completed) << 2)
            | (u32::from(state.http2) << 3)
            | (u32::from(state.send_chunked) << 4)
            | (u32::from(co_socket(state)) << 5)
            | (u32::from(state.upgrade) << 6)
            | (u32::from(state.websocket) << 7)
    }
}

/// The global hooks that take an HTTP context.
#[derive(Clone, Copy)]
pub(crate) enum Hook {
    BeforeRequest,
    AfterResponse,
}

/// `Swoole\Http\Response`'s declared `fd` slot (MINIT).
static RESPONSE_FD: Declared = Declared::new();

/// MINIT, once `Swoole\Http\Response` is registered.
pub(crate) fn response_minit() {
    RESPONSE_FD.resolve(&crate::response::RESPONSE, c"fd");
}

/// The class of a context's object.
#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Request,
    Response,
}

/// A new `Swoole\Http\Request` or `Response` in the context's `slot`,
/// holding `hold`.
pub(crate) fn new_object(slot: &mut Slot, kind: Kind, hold: Hold) -> Option<*mut zend_object> {
    // The context keeps the object's reference.
    slot.put(kind.class().create_object());
    point(slot.get()?, kind, hold)
}

/// A new `Swoole\Http\Request` or `Response` as `call`'s return value,
/// holding `hold`, whose `slot` borrows it, as upstream's `RETVAL_OBJ` and
/// `ctx->request.zobject` do.
pub(crate) fn lend_object(call: &Call, slot: &mut Slot, kind: Kind, hold: Hold) {
    call.set_owned(kind.class().create_object());
    // SAFETY: the return value holds the object, and the object (`point`) the
    // context and so the slot: the object's free handler clears the slot
    // before the object goes.
    unsafe { slot.keep(call.ret()) };
    if let Some(object) = slot.get() {
        point(object, kind, hold);
    }
}

/// Give `object`, a new `kind` object, `hold`.
fn point(object: &Value, kind: Kind, hold: Hold) -> Option<*mut zend_object> {
    match kind {
        Kind::Request => object.custom::<crate::request::Data>()?.0.set(hold),
        Kind::Response => object.custom::<crate::response::Data>()?.0.set(hold),
    }
    .ok()?;
    object.object()
}

impl Kind {
    fn class(self) -> &'static swoole_rs_zend::Class {
        match self {
            Self::Request => &crate::request::REQUEST,
            Self::Response => &crate::response::RESPONSE,
        }
    }
}

/// A new object in `slot`, holding `hold`, with its declared `fd` property set.
fn object(slot: &mut Slot, kind: Kind, hold: Hold, property: &Declared, fd: i64) {
    if new_object(slot, kind, hold).is_some()
        && let Some(object) = slot.get()
    {
        property.init(object, Owned::long(fd));
    }
}

/// `swoole_http_context_new`: a new context with its request and response
/// objects, both holding it and `fd`, and the request's `server` and `header`
/// arrays.
pub(crate) fn context_new(fd: i64) -> Hold {
    let hold = alloc();
    let mut ctx = hold.ctx();
    let state = ctx.state();
    object(
        &mut state.request.zobject,
        Kind::Request,
        hold.clone(),
        &parse::REQUEST_FD,
        fd,
    );
    object(
        &mut state.response.zobject,
        Kind::Response,
        hold.clone(),
        &RESPONSE_FD,
        fd,
    );
    parse::property(&mut state.request, Property::Server);
    parse::property(&mut state.request, Property::Header);
    state.fd = fd;
    hold
}

/// `new Context()`: a value-initialised context, which goes with its last
/// hold.
#[must_use]
pub(crate) fn alloc() -> Hold {
    // Not `Rc::new_zeroed`: glibc's `calloc` skips the thread cache and cost
    // 500 instructions a request where `malloc` and a memset cost about 100
    // (#2129). `black_box` keeps LLVM from folding them back into `calloc`.
    let mut fresh = std::hint::black_box(Rc::<UnsafeCell<State>>::new_uninit());
    if let Some(state) = Rc::get_mut(&mut fresh) {
        *state = MaybeUninit::zeroed();
    }
    // SAFETY: a fresh `Rc` is unique, so `get_mut` zeroed it, and all zeroes
    // is a valid `State` (null pointers, `None`s and `IS_UNDEF` zvals, as
    // upstream's value-initialisation left it, and a null socket handle, which
    // `socket` reads only once `bind` set it).
    Hold(unsafe { fresh.assume_init() })
}

/// `Context::init(Server *)`: the server's request settings, then bound to it.
pub(crate) fn init(state: &mut State, server: view::Server<'_>) {
    let config = server.config();
    state.parse_cookie = config.http_parse_cookie.get();
    state.parse_body = config.http_parse_post.get();
    state.parse_files = config.http_parse_files.get();
    if parse::CAPABILITIES.load(Relaxed) & 8 != 0 {
        state.enable_compression = config.http_compression.get();
        state.compression_level = i8::from_ne_bytes([config.http_compression_level.get()]);
        state.compression_min_length = config.compression_min_length.get();
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

/// The coroutine socket the context is bound to, which outlives the borrow
/// of its state (the context holds the socket's object), or None for a
/// server's context.
pub(crate) fn socket(state: &State) -> Option<co::Co> {
    co_socket(state).then_some(state.socket)
}

/// `ctx->send`: the server's send to the session, or the coroutine socket's.
pub(crate) fn send(mut ctx: Ctx, data: &[u8]) -> bool {
    let state = ctx.state();
    match socket(state) {
        // True once every byte is sent.
        Some(socket) => socket.send_all(data) == Some(data.len()),
        None => server_send(server(state), state.fd, data),
    }
}

/// `SW_ERROR_OUTPUT_SEND_YIELD`.
const OUTPUT_SEND_YIELD: c_int = swoole_rs_error::ErrorCode::SW_ERROR_OUTPUT_SEND_YIELD as c_int;

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

/// [`server_send`] of `head` then `body`: one write that copies neither where the
/// server writes straight to the socket, else the two joined.
pub(crate) fn server_send_parts(
    server: view::Server<'_>,
    fd: i64,
    head: &[u8],
    body: &[u8],
) -> bool {
    if body.is_empty() {
        return server_send(server, fd, head);
    }
    server_methods::send_parts(server, fd, head, body)
        .unwrap_or_else(|| server_send(server, fd, &[head, body].concat()))
}

/// `ctx->sendfile`.
pub(crate) fn sendfile(mut ctx: Ctx, file: &ZStr, offset: i64, length: usize) -> bool {
    let state = ctx.state();
    if let Some(socket) = socket(state) {
        socket.sendfile(file.c_str(), offset, length)
    } else {
        server_methods::sendfile(server(state), state.fd, file.bytes(), offset, length)
    }
}

/// `ctx->close`.
pub(crate) fn close(mut ctx: Ctx) -> bool {
    let state = ctx.state();
    match socket(state) {
        Some(socket) => socket.close(),
        None => server_methods::close(server(state), state.fd, false),
    }
}

/// `Context::bind(Server *)`: the context sends through the server.
pub(crate) fn bind_server(state: &mut State, server: view::Server<'_>) {
    state.private_data = server.raw();
    // A server's context holds no socket.
    state.zsocket = Owned::null();
}

/// `Context::bind(zval *)`: the context holds the socket object and sends
/// through its coroutine socket; nothing for a value that holds none.
pub(crate) fn bind(state: &mut State, socket: &Value) {
    let Some(co) = swoole_rs_phpclients::get_socket(socket) else {
        return;
    };
    state.socket = co.handle();
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
    state.socket = from.socket;
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
    if !state.before_request {
        return true;
    }
    state.before_request = false;
    state.after_response = true;
    // Only `init` sets `before_request`, after binding the server (or a copy, with it).
    crate::server::admit(server(state), ctx)
}

/// `onAfterResponse`: the server uncounts the request and starts a queued one.
pub(crate) fn after_response(mut ctx: Ctx) {
    let state = ctx.state();
    if state.after_response {
        state.after_response = false;
        // Set only by `before_request`, on a server-bound context.
        crate::server::release_request(server(state));
    }
}

/// `Context::is_available`: the response can still reach its peer.
pub(crate) fn available(mut ctx: Ctx) -> bool {
    let state = ctx.state();
    if state.response.zobject.get().is_none() {
        return false;
    }
    if co_socket(state) {
        return !swoole_rs_phpclients::is_socket_closed(&state.zsocket);
    }
    crate::context::server(state)
        .by_session(state.fd)
        .is_some_and(|conn| conn.closed().load(Relaxed) == 0 && !conn.peer_closed())
}

unsafe extern "C" {
    /// `swr_zend_http_release_objects` inside `zend_try` (nonzero for a
    /// bailout): the record's objects, which only [`Release::new`] reads from
    /// a live context's slots, released in turn as its count says.
    #[link_name = "swr_zend_http_try_release_objects"]
    safe fn try_release_objects(release: &mut Release) -> c_int;
}

/// The context's own references to its request and response objects,
/// released once it is handled (`swr_zend_http_release`, which the C body
/// `swr_zend_http_release_objects` releases). A property's `__destruct` can
/// run PHP code, so both are released inside one `zend_try`: the handler's,
/// when its coroutine call lends it ([`Release::then`]), or
/// [`Release::finish`]'s. The response is released even after the request's
/// bailout, so the context is freed, and the bailout is deferred first, so the
/// response's free hooks call no PHP code.
#[repr(C)]
pub(crate) struct Release {
    request: *mut zval,
    response: *mut zval,
    /// How many of the two releases have begun.
    begun: u8,
}

impl Release {
    /// Both slots are read now: the request's free handler frees the context
    /// only once the response's has cleared its slot.
    pub(crate) fn new(mut ctx: Ctx) -> Self {
        let state = ctx.state();
        Self {
            request: state.request.zobject.as_ptr(),
            response: state.response.zobject.as_ptr(),
            begun: 0,
        }
    }

    /// Whether a release has begun.
    pub(crate) const fn begun(&self) -> bool {
        self.begun != 0
    }

    /// The releases, as a coroutine call's [`Then`], for as long as `self`
    /// is not moved.
    pub(crate) fn then(&mut self) -> Then {
        // SAFETY: the C body over this record's live objects, which reads and
        // writes only the record; the caller keeps `self` in place through the
        // call it is given to.
        unsafe {
            Then::new(
                Some(swr_zend_http_release_objects),
                ptr::from_mut(self).cast(),
            )
        }
    }

    /// What has not been released, inside `zend_try`, with a bailout of the
    /// handler's trap already deferred.
    /// # Errors
    /// A destructor bailed out, or a bailout is pending.
    pub(crate) fn finish(mut self) -> Result<(), Bailout> {
        // Twice at most: a bailout in the request's release leaves the response's.
        while self.begun < 2 {
            bailout::defer(bailout::trapped(try_release_objects(&mut self)));
        }
        bailout::outcome(())
    }
}

/// [`Release`] outside a handler call.
/// # Errors
/// A destructor bailed out, or a bailout is pending.
pub(crate) fn release_objects(ctx: Ctx) -> Result<(), Bailout> {
    Release::new(ctx).finish()
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
