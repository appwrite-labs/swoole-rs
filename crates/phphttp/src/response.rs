//! `Swoole\Http\Response`.
use crate::cookie::{header_line, object_header_line};
use crate::{
    context::{self, Ctx, Holder},
    host::{
        self, CO_SOCKET, DETACHED, END, HEADER_KEY_SIZE, HTTP_CONTEXT_UNAVAILABLE, HTTP2, Property,
        SEND_CHUNKED, SERVER_IS_NOT_REGULAR_FILE, SESSION_CLOSED, UPGRADE, WEBSOCKET,
    },
    send, websocket,
};
use std::{
    ffi::{CString, c_int, c_void},
    sync::atomic::Ordering::Relaxed,
};
use swoole_rs_corebase::{CCast, HookType, swoole_call_hook, swoole_isset_hook};
use swoole_rs_httpbindings::{canonical_case, swr_v1_httpbindings_file_range};
use swoole_rs_phpstubs::http_response;
use swoole_rs_phpswoole::MinitArgs;
use swoole_rs_zend::bailout::Bailout;
use swoole_rs_zend::{
    Call, Custom, CustomClass, Invalid, Owned, Value, ZStr, ZString, handlers, push_string,
    set_key,
    sys::{IS_NULL, IS_OBJECT, IS_STRING, ZEND_ACC_PUBLIC, zend_long},
};
use swoole_rs_zend::{method_table, sys as z};

pub(crate) static RESPONSE: CustomClass<Data> = CustomClass::new();

/// `WebSocket::OPCODE_TEXT` and `WebSocket::CLOSE_NORMAL` (RFC 6455).
const OPCODE_TEXT: i64 = 1;
const CLOSE_NORMAL: i64 = 1000;
/// `SW_ERROR_WEBSOCKET_PACK_FAILED`.
const WEBSOCKET_PACK_FAILED: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_WEBSOCKET_PACK_FAILED as c_int;
/// `SW_HTTP_INTERNAL_SERVER_ERROR`.
const INTERNAL_SERVER_ERROR: c_int = 500;

/// A response object's context.
pub(crate) struct Data(pub(crate) Holder);

impl Custom for Data {
    fn class() -> &'static CustomClass<Self> {
        &RESPONSE
    }

    fn new() -> Self {
        Self(Holder::new(None))
    }
}

/// The object goes: what the user code it runs raised is raised once it is
/// freed.
impl Drop for Data {
    fn drop(&mut self) {
        if self.0.get().is_some_and(release) {
            Bailout.defer();
        }
    }
}

/// The method's `$this` context.
fn this(call: &Call) -> Option<Ctx> {
    call.custom::<Data>().and_then(|data| data.0.get())
}

/// The response object is going: finish what the handler left unanswered
/// (500 when no status was set) and release the context. True if user code
/// bailed out on the way.
fn release(mut ctx: Ctx) -> bool {
    context::after_response(ctx);
    let mut bailed = false;
    let state = ctx.state();
    if !state.end
        && (state.send_chunked || !state.send_header)
        && !state.detached
        // SAFETY: reads a thread global.
        && !swoole_rs_reactor::event_api::swr_v1_event_reactor().is_null()
    {
        if state.response.status == 0 {
            state.response.status = INTERNAL_SERVER_ERROR;
        }
        if context::available(ctx) {
            bailed = send::end(ctx, None).is_err();
        }
    }
    ctx.state().response.zobject.clear();
    // SAFETY: a live context, which the response no longer holds.
    unsafe { context::free(ctx) };
    bailed
}

method_table! {
    static METHODS = [
        c"initHeader" => zim_swoole_http_response_initHeader, &http_response::arginfo_class_Swoole_Http_Response_initHeader, z::ZEND_ACC_PUBLIC;
        c"isWritable" => zim_swoole_http_response_isWritable, &http_response::arginfo_class_Swoole_Http_Response_isWritable, z::ZEND_ACC_PUBLIC;
        c"cookie" => zim_swoole_http_response_cookie, &http_response::arginfo_class_Swoole_Http_Response_cookie, z::ZEND_ACC_PUBLIC;
        c"setCookie" => zim_swoole_http_response_cookie, &http_response::arginfo_class_Swoole_Http_Response_cookie, z::ZEND_ACC_PUBLIC;
        c"rawcookie" => zim_swoole_http_response_rawcookie, &http_response::arginfo_class_Swoole_Http_Response_cookie, z::ZEND_ACC_PUBLIC;
        c"setRawCookie" => zim_swoole_http_response_rawcookie, &http_response::arginfo_class_Swoole_Http_Response_cookie, z::ZEND_ACC_PUBLIC;
        c"status" => zim_swoole_http_response_status, &http_response::arginfo_class_Swoole_Http_Response_status, z::ZEND_ACC_PUBLIC;
        c"setStatusCode" => zim_swoole_http_response_status, &http_response::arginfo_class_Swoole_Http_Response_status, z::ZEND_ACC_PUBLIC;
        c"header" => zim_swoole_http_response_header, &http_response::arginfo_class_Swoole_Http_Response_header, z::ZEND_ACC_PUBLIC;
        c"setHeader" => zim_swoole_http_response_header, &http_response::arginfo_class_Swoole_Http_Response_header, z::ZEND_ACC_PUBLIC;
        c"trailer" => zim_swoole_http_response_trailer, &http_response::arginfo_class_Swoole_Http_Response_trailer, z::ZEND_ACC_PUBLIC;
        c"ping" => zim_swoole_http_response_ping, &http_response::arginfo_class_Swoole_Http_Response_ping, z::ZEND_ACC_PUBLIC;
        c"goaway" => zim_swoole_http_response_goaway, &http_response::arginfo_class_Swoole_Http_Response_goaway, z::ZEND_ACC_PUBLIC;
        c"write" => zim_swoole_http_response_write, &http_response::arginfo_class_Swoole_Http_Response_write, z::ZEND_ACC_PUBLIC;
        c"end" => zim_swoole_http_response_end, &http_response::arginfo_class_Swoole_Http_Response_end, z::ZEND_ACC_PUBLIC;
        c"sendfile" => zim_swoole_http_response_sendfile, &http_response::arginfo_class_Swoole_Http_Response_sendfile, z::ZEND_ACC_PUBLIC;
        c"redirect" => zim_swoole_http_response_redirect, &http_response::arginfo_class_Swoole_Http_Response_redirect, z::ZEND_ACC_PUBLIC;
        c"detach" => zim_swoole_http_response_detach, &http_response::arginfo_class_Swoole_Http_Response_detach, z::ZEND_ACC_PUBLIC;
        c"create" => zim_swoole_http_response_create, &http_response::arginfo_class_Swoole_Http_Response_create, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"upgrade" => zim_swoole_http_response_upgrade, &http_response::arginfo_class_Swoole_Http_Response_upgrade, z::ZEND_ACC_PUBLIC;
        c"push" => zim_swoole_http_response_push, &http_response::arginfo_class_Swoole_Http_Response_push, z::ZEND_ACC_PUBLIC;
        c"recv" => zim_swoole_http_response_recv, &http_response::arginfo_class_Swoole_Http_Response_recv, z::ZEND_ACC_PUBLIC;
        c"close" => zim_swoole_http_response_close, &http_response::arginfo_class_Swoole_Http_Response_close, z::ZEND_ACC_PUBLIC;
        c"disconnect" => zim_swoole_http_response_disconnect, &http_response::arginfo_class_Swoole_Http_Response_disconnect, z::ZEND_ACC_PUBLIC;
    ];
}

/// MINIT: register the class as `php_swoole_http_response_minit` did, after
/// `Swoole\Http\Request`'s; the parser learns the codecs built in (`codecs`:
/// 1 zlib, 8 `SW_HAVE_COMPRESSION`).
pub fn response_minit(args: &MinitArgs) {
    let codecs = args.codecs | (u32::from(args.compression) << 3);
    let minit = args.minit;
    minit.register_object::<Data>(c"Swoole\\Http\\Response", METHODS.table(), 0);
    minit.property_long(&RESPONSE, c"fd", 0, ZEND_ACC_PUBLIC);
    for name in [c"socket", c"header", c"cookie", c"trailer"] {
        minit.property_null(&RESPONSE, name, ZEND_ACC_PUBLIC);
    }
    crate::header::minit(&RESPONSE);
    context::response_minit();
    crate::parse::minit(minit, codecs);
}

/// `http_response_get_and_check_context`: a context that can still respond.
fn checked(call: &Call) -> Option<Ctx> {
    let ctx = this(call).filter(|ctx| ctx.flags() & (END | DETACHED) == 0);
    if ctx.is_none() {
        host::set_last_error(HTTP_CONTEXT_UNAVAILABLE);
    }
    ctx
}

/// `php_swoole_http_response_get_context`, setting `SW_ERROR_SESSION_CLOSED`
/// when there is none.
fn open(call: &Call) -> Option<Ctx> {
    let ctx = this(call);
    if ctx.is_none() {
        host::set_last_error(SESSION_CLOSED);
    }
    ctx
}

/// The coroutine WebSocket checks `push`, `recv` and `disconnect` share.
fn websocket(call: &Call) -> Option<Ctx> {
    let ctx = open(call)?;
    let flags = ctx.flags();
    if flags & CO_SOCKET == 0 || flags & UPGRADE == 0 {
        host::fatal(&format!("fd[{}] is not a websocket conncetion", ctx.fd()));
        return None;
    }
    Some(ctx)
}

/// `ZEND_PARSE_PARAMETERS_END_EX(RETURN_FALSE)` or a failed check: false.
fn or_false(call: &Call, done: Option<()>) {
    if done.is_none() {
        call.set_bool(false);
    }
}

fn property(mut ctx: Ctx, which: Property) -> *mut swoole_rs_zend::sys::zval {
    crate::header::response_property(ctx.state(), which)
}

fn init_header(call: &Call) {
    or_false(
        call,
        checked(call).map(|ctx| {
            for which in [Property::Header, Property::Cookie, Property::Trailer] {
                property(ctx, which);
            }
            call.set_bool(true);
        }),
    );
}

fn is_writable(call: &Call) {
    call.set_bool(this(call).is_some_and(|ctx| ctx.flags() & (END | DETACHED) == 0));
}

/// A send's result as the method's return value; a bailout is raised last.
fn sent(call: &Call, result: &Result<bool, Bailout>) {
    match result {
        Ok(done) => call.set_bool(*done),
        Err(_) => host::raise(true),
    }
}

fn write(call: &Call) {
    let run = || -> Option<Result<bool, Bailout>> {
        let data = call.arity(1, 1).and_then(|()| call.zstr(1)).ok()??;
        let mut ctx = checked(call)?;
        // Compressing a chunked body would compress the chunks, not the stream.
        ctx.state().accept_compression = false;
        Some(send::write(ctx, Some(data)))
    };
    match run() {
        Some(result) => sent(call, &result),
        None => call.set_bool(false),
    }
}

/// `http_response_after`: the server's and the hooks' after-response step.
fn after(ctx: Ctx) -> Result<(), Bailout> {
    unsafe extern "C" fn hooks(ctx: *mut c_void) {
        // SAFETY: the hooks take the context, as upstream passed it. The
        // frame owns nothing a bailout out of a hook would skip.
        unsafe { swoole_call_hook(HookType::AFTER_RESPONSE, ctx) };
    }
    context::after_response(ctx);
    if !swoole_isset_hook(HookType::AFTER_RESPONSE) {
        return Ok(());
    }
    // SAFETY: a live context; the trap's setjmp sits in C, below `hooks`.
    unsafe { swoole_rs_zend::bailout::trap(hooks, ctx.raw()) }
}

fn end(call: &Call) {
    let run = || -> Option<Result<bool, Bailout>> {
        let ctx = checked(call)?;
        let data = call.arity(0, 1).and_then(|()| call.zstr_or_null(1)).ok()?;
        Some(after(ctx).and_then(|()| send::end(ctx, data)))
    };
    match run() {
        Some(result) => sent(call, &result),
        None => call.set_bool(false),
    }
}

/// `stat(2)` of a file name, as `php_swoole_sys_error` reports its failure.
fn stat(file: &ZStr) -> Result<libc::stat, ()> {
    let path = file.c_str();
    if let Some(stat) = swoole_rs_corebase::sys::stat(path) {
        return Ok(stat);
    }
    let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    let reason = swoole_rs_corebase::sys::strerror(error);
    let mut message = b"stat(".to_vec();
    message.extend_from_slice(path.to_bytes());
    message.extend_from_slice(b") failed, Error: ");
    message.extend_from_slice(reason.as_bytes());
    message.extend_from_slice(format!("[{error}]").as_bytes());
    host::report(error, false, message);
    Err(())
}

fn send_file(call: &Call) {
    let run = || -> Option<Result<bool, Bailout>> {
        let ctx = checked(call)?;
        if ctx.flags() & SEND_CHUNKED != 0 {
            host::fatal("can't use sendfile when HTTP chunk is enabled");
            return None;
        }
        let parsed = call
            .arity(1, 3)
            .and_then(|()| Ok((call.zstr(1)?, call.long(2, 0)?, call.long(3, 0)?)));
        let (Some(file), offset, requested) = parsed.ok()? else {
            return None;
        };
        let name = file.bytes();
        if name.is_empty() {
            host::notice("file name is empty");
            return None;
        }
        let stat = stat(file).ok()?;
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
            let mut message = b"parameter $file[".to_vec();
            message.extend_from_slice(name);
            message.extend_from_slice(b"] given is not a regular file");
            host::report(SERVER_IS_NOT_REGULAR_FILE, false, message);
            return None;
        }
        let mut length = 0;
        match swr_v1_httpbindings_file_range(stat.st_size, offset, requested, Some(&mut length)) {
            0 => {}
            1 | 3 => {
                host::notice(&format!(
                    "parameter $offset[{offset}] exceeds the file size"
                ));
                return None;
            }
            2 => {
                host::notice(&format!(
                    "parameter $length[{requested}] exceeds the file size"
                ));
                return None;
            }
            _ => return None,
        }
        let length = usize::try_from(length).unwrap_or_default();
        Some(after(ctx).and_then(|()| send::send_file(ctx, file, offset, length)))
    };
    match run() {
        Some(result) => sent(call, &result),
        None => call.set_bool(false),
    }
}

/// `http_response_set_cookie`: `cookie` encodes the value, `rawcookie` not.
fn set_cookie(call: &Call, encode: bool) {
    // Zend parses in order and stops at the first bad argument.
    let parsed = (|| -> Result<_, Invalid> {
        call.arity(1, 10)?;
        let name = call.value(1);
        let value = call.zstr(2)?;
        let expires = call.long(3, 0)?;
        let path = call.zstr(4)?;
        let domain = call.zstr(5)?;
        let secure = call.bool(6, false)?;
        let http_only = call.bool(7, false)?;
        let same_site = call.zstr(8)?;
        let priority = call.zstr(9)?;
        let partitioned = call.bool(10, false)?;
        Ok(name.map(|name| {
            let flags = [secure, http_only, partitioned];
            (
                name,
                [value, path, domain, same_site, priority],
                expires,
                flags,
            )
        }))
    })();
    let Ok(Some((
        name,
        [value, path, domain, same_site, priority],
        expires,
        [secure, http_only, partitioned],
    ))) = parsed
    else {
        return call.set_bool(false);
    };
    let kind = name.kind();
    if kind != IS_STRING && kind != IS_OBJECT {
        host::notice("The first argument must be a string or an cookie object");
        return call.set_bool(false);
    }
    // The Set-Cookie line (crates/httpbindings), or null after a warning. Upstream builds
    // it before it uses the context, so a bad name warns on any response.
    let cookie = if kind == IS_STRING {
        // A string argument, so `Z_PARAM_STR` reads it as it is.
        let name = call.zstr(1).ok().flatten();
        header_line(
            encode,
            [name, value, path, domain, same_site, priority],
            expires,
            (secure, http_only, partitioned),
        )
    } else {
        object_header_line(name)
    };
    let Some(cookie) = cookie else {
        return call.set_bool(false);
    };
    let Some(ctx) = checked(call) else {
        return call.set_bool(false);
    };
    // SAFETY: the cookie property is a live array; it takes over the string.
    unsafe { push_string(property(ctx, Property::Cookie), cookie.into_raw()) };
    call.set_bool(true);
}

fn cookie(call: &Call) {
    set_cookie(call, true);
}

fn raw_cookie(call: &Call) {
    set_cookie(call, false);
}

fn status(call: &Call) {
    let run = || -> Option<()> {
        let parsed = call
            .arity(1, 2)
            .and_then(|()| Ok((call.long(1, 0)?, call.string_opt(2)?)));
        let (code, reason) = parsed.ok()?;
        let mut ctx = checked(call)?;
        let reason = reason.unwrap_or_default();
        let response = &mut ctx.state().response;
        let code = code.c_cast::<i32>();
        response.status = code;
        response.reason = (!reason.is_empty()).then(|| {
            let text = reason.split(|&byte| byte == 0).next().unwrap_or_default();
            Box::new(CString::new(text).unwrap_or_default())
        });
        call.set_bool(true);
        Some(())
    };
    or_false(call, run());
}

fn disconnect(call: &Call) {
    let run = || -> Option<()> {
        let ctx = websocket(call)?;
        let parsed = call
            .arity(0, 2)
            .and_then(|()| Ok((call.long(1, CLOSE_NORMAL)?, call.zstr(2)?)));
        let (code, reason) = parsed.ok()?;
        call.set_bool(websocket::response_disconnect(ctx, code, reason));
        Some(())
    };
    or_false(call, run());
}

fn header(call: &Call) {
    let run = || -> Option<()> {
        let parsed = call
            .arity(2, 3)
            .and_then(|()| Ok((call.string_opt(1)?, call.value(2), call.bool(3, true)?)));
        let (Some(key), Some(value), format) = parsed.ok()? else {
            return None;
        };
        let ctx = checked(call)?;
        call.set_bool(set_header(ctx, key, value, format));
        Some(())
    };
    or_false(call, run());
}

fn trailer(call: &Call) {
    let run = || -> Option<()> {
        let parsed = call
            .arity(2, 2)
            .and_then(|()| Ok((call.string_opt(1)?, call.string_or_null(2)?)));
        let (Some(key), value) = parsed.ok()? else {
            return None;
        };
        let mut ctx = checked(call)?;
        if key.len() > HEADER_KEY_SIZE - 1 {
            host::notice("trailer key is too long");
            return None;
        }
        let key = key.to_ascii_lowercase();
        // SAFETY: the trailer property is a live array.
        unsafe { set_key(property(ctx, Property::Trailer), &key, value) };
        ctx.state().send_trailer = true;
        call.set_bool(true);
        Some(())
    };
    or_false(call, run());
}

fn ping(call: &Call) {
    let run = || -> Option<()> {
        let ctx = this(call)?;
        let data = call.arity(0, 1).and_then(|()| call.zstr(1)).ok()?;
        let flags = ctx.flags();
        if flags & (HTTP2 | WEBSOCKET) == 0 {
            host::fatal("only supports websocket or http2 client");
            return None;
        }
        if flags & HTTP2 != 0 {
            match send::http2(ctx, send::Http2::Ping) {
                Ok(done) => call.set_bool(done),
                Err(bailout) => bailout.defer(),
            }
            return Some(());
        }
        let payload = data.map_or(&[][..], ZStr::bytes);
        let Some(frame) = websocket::encode(payload, websocket::OPCODE_PING, websocket::FLAG_FIN)
        else {
            host::set_last_error(WEBSOCKET_PACK_FAILED);
            return None;
        };
        call.set_bool(send::send(ctx, frame.bytes()));
        Some(())
    };
    or_false(call, run());
}

fn goaway(call: &Call) {
    let run = || -> Option<()> {
        let ctx = checked(call)?;
        if ctx.flags() & HTTP2 == 0 {
            host::fatal(&format!("fd[{}] is not a HTTP2 conncetion", ctx.fd()));
            return None;
        }
        let parsed = call
            .arity(0, 2)
            .and_then(|()| Ok((call.long(1, 0)?, call.zstr_or_null(2)?)));
        let (code, debug) = parsed.ok()?;
        let debug = debug.map_or(&[][..], ZStr::bytes);
        let server = crate::http2::server();
        call.set_bool(server.is_some_and(|server| (server.goaway)(ctx, code, debug)));
        Some(())
    };
    or_false(call, run());
}

fn upgrade(call: &Call) {
    let mut bailed = false;
    let mut run = || -> Option<()> {
        let ctx = checked(call)?;
        if ctx.flags() & CO_SOCKET == 0 {
            host::fatal("async server dose not support protocol upgrade");
            return None;
        }
        let upgraded = websocket::upgrade(ctx);
        bailed = upgraded.is_err();
        call.set_bool(upgraded.unwrap_or(false));
        Some(())
    };
    let done = run();
    or_false(call, done);
    host::raise(bailed);
}

fn push(call: &Call) {
    let run = || -> Option<()> {
        let ctx = websocket(call)?;
        let parsed = call.arity(1, 3).and_then(|()| {
            Ok((
                call.value(1),
                call.long(2, OPCODE_TEXT)?,
                call.zval_or_null(3),
            ))
        });
        let (Some(data), opcode, flags) = parsed.ok()? else {
            return None;
        };
        call.set_bool(websocket::response_push(ctx, data, opcode, flags));
        Some(())
    };
    let done = run();
    or_false(call, done);
}

fn close(call: &Call) {
    let run = || -> Option<()> {
        let ctx = open(call)?;
        call.set_bool(context::close(ctx));
        Some(())
    };
    or_false(call, run());
}

fn recv(call: &Call) {
    let run = || -> Option<()> {
        let mut ctx = websocket(call)?;
        let timeout = call.arity(0, 1).and_then(|()| call.double(1, 0.0)).ok()?;
        let state = ctx.state();
        // `websocket` checked the context is bound to a coroutine socket.
        let (settings, socket) = (u32::from(state.websocket_settings), context::socket(state)?);
        let frame = websocket::recv(settings, &mut state.ws_buffer, socket, timeout);
        match frame.kind() {
            // `""`: the peer closed.
            IS_STRING => {
                context::close(ctx);
                call.set_owned(frame);
            }
            IS_NULL => call.set_bool(false),
            _ => call.set_owned(frame),
        }
        Some(())
    };
    or_false(call, run());
}

fn detach(call: &Call) {
    let run = || -> Option<()> {
        let mut ctx = checked(call)?;
        ctx.state().detached = true;
        call.set_bool(true);
        Some(())
    };
    or_false(call, run());
}

fn create_response(call: &Call) {
    let run = || -> Option<()> {
        let parsed = call
            .arity(1, 2)
            .and_then(|()| Ok((call.value(1), call.long(2, -1)?)));
        let (Some(object), fd) = parsed.ok()? else {
            return None;
        };
        respond_on(call, object, fd);
        Some(())
    };
    or_false(call, run());
}

/// `Context::set_header`: `$response->header[key] = value`, the key in
/// canonical case when `format` (lower case over HTTP/2).
pub(crate) fn set_header(ctx: Ctx, key: &[u8], value: &Value, format: bool) -> bool {
    if key.len() > HEADER_KEY_SIZE - 1 {
        host::notice("header key is too long");
        return false;
    }
    if crate::header::has_crlf(key) {
        return false;
    }
    let header = property(ctx, Property::Header);
    let mut buffer = [0; HEADER_KEY_SIZE];
    // Formatted in a stack copy, as upstream does in its thread buffer.
    let name = match buffer.get_mut(..key.len()) {
        Some(name) if format => {
            name.copy_from_slice(key);
            if ctx.flags() & HTTP2 != 0 {
                name.make_ascii_lowercase();
            } else {
                canonical_case(name);
            }
            &*name
        }
        _ => key,
    };
    // The array takes this copy's reference.
    let mut value = value.copy().into_zval();
    // SAFETY: the live header array and the name's bytes, which Zend copies.
    unsafe { z::add_assoc_zval_ex(header, name.as_ptr().cast(), name.len(), &raw mut value) };
    true
}

/// `Context::set_header(key, value, false)` of a string value. A warning's
/// bailout waits for the caller's last step.
pub(crate) fn set_header_bytes(ctx: Ctx, key: &[u8], value: &[u8]) {
    // This frame's reference; the header array takes its own.
    let value = Owned::string(ZString::new(value));
    set_header(ctx, key, &value, false);
}

/// `php_swoole_fatal_error(E_WARNING, ...)`, then `RETURN_FALSE`.
fn refuse(call: &Call, message: &str) {
    host::fatal(message);
    call.set_bool(false);
}

/// `Response::create($server|$socket|[$server|$socket, $request]|$fd, $fd)`:
/// a response on a server connection or a coroutine socket, sharing the
/// request's context when one is given.
fn respond_on(call: &Call, mut object: &Value, mut fd: zend_long) {
    let mut paired = false;
    let mut ctx = None;
    let mut server = None;
    let mut socket = None;
    if let Some(pair) = object.array() {
        let Some(request) = pair
            .index(1)
            .filter(|found| found.instance_of(&crate::request::REQUEST))
            .and_then(Value::custom::<crate::request::Data>)
        else {
            return refuse(call, "parameter $1.second must be instanceof Http\\Request");
        };
        let Some(first) = pair.index(0).filter(|first| first.object().is_some()) else {
            return refuse(
                call,
                "parameter $1 must be instanceof Server or Coroutine\\Socket",
            );
        };
        object = first;
        paired = true;
        ctx = request.0.get();
    }
    if object.object().is_some() {
        if object.instance_of(swoole_rs_phpserver::server_class_ref()) {
            let Some(checked) = websocket::server_of(object) else {
                return;
            };
            server = Some(checked);
            if !websocket::server_connection(checked, fd) {
                return refuse(
                    call,
                    &format!("parameter $2 ({fd}) must be valid connection session_id"),
                );
            }
        } else if object.instance_of(swoole_rs_phpclients::socket()) {
            // A socket object whose constructor never ran holds no socket, so
            // there is nothing to respond on: false, as without a target.
            if let Some(co) = swoole_rs_phpclients::get_socket(object) {
                socket = Some(object);
                fd = zend_long::from(co.handle().sock_fd());
            }
        } else {
            return refuse(
                call,
                "parameter $1 must be instanceof Server or Coroutine\\Socket",
            );
        }
    } else {
        fd = object.long();
        // SAFETY: the running server, which nothing deletes during this method.
        server = unsafe { swoole_rs_server::view::Server::current() };
    }
    if server.is_some_and(|server| !server.is_started()) {
        return refuse(call, "server is not running");
    }
    if server.is_none() && socket.is_none() {
        call.set_bool(false);
        return;
    }
    let mut ctx = ctx.unwrap_or_else(|| {
        let mut ctx = context::alloc();
        ctx.state().keepalive = true;
        if let Some(server) = server {
            context::init(ctx.state(), server);
        } else {
            crate::parse::init_request_parser(ctx);
            // `Context::init(zval *)`.
            let state = ctx.state();
            state.parse_cookie = true;
            state.parse_body = true;
            state.parse_files = true;
            if crate::parse::CAPABILITIES.load(Relaxed) & 8 != 0 {
                state.enable_compression = true;
                state.compression_level = 1;
            }
        }
        ctx
    });
    if let Some(server) = server {
        context::bind_server(ctx.state(), server);
    } else if let Some(socket) = socket {
        context::bind(ctx.state(), socket);
    }
    let held = ctx;
    let state = ctx.state();
    state.fd = fd;
    let slot = &mut state.response.zobject;
    let _made = context::new_object(slot, context::Kind::Response, held);
    // SAFETY: the object just made. The return slot takes the reference; the
    // context borrows it.
    unsafe { call.set_value(slot.copy_value()) };
    let Some(props) = slot.get().and_then(|value| value.props(Some(&RESPONSE))) else {
        return;
    };
    props.long(c"fd", fd);
    // The property takes its own reference.
    if let Some(socket) = socket {
        props.value(c"socket", socket);
    }
    if paired && let Some(request) = ctx.state().request.zobject.props() {
        request.long(c"fd", fd);
    }
}

fn redirect(call: &Call) {
    let mut bailed = false;
    let mut run = || -> Option<()> {
        let parsed = call
            .arity(1, 2)
            .map(|()| (call.value(1), call.zval_or_null(2)));
        let (Some(url), code) = parsed.ok()? else {
            return None;
        };
        let mut ctx = checked(call)?;
        let status = code.map_or(302, |code| code.long().c_cast::<c_int>());
        ctx.state().response.status = status;
        set_header(ctx, b"Location", url, false);
        if swoole_rs_zend::bailout::pending() {
            bailed = true;
            return Some(());
        }
        match send::end(ctx, None) {
            Ok(done) => call.set_bool(done),
            Err(_) => bailed = true,
        }
        Some(())
    };
    let done = run();
    or_false(call, done);
    host::raise(bailed);
}

handlers! {
    zim_swoole_http_response_initHeader => init_header;
    zim_swoole_http_response_isWritable => is_writable;
    zim_swoole_http_response_cookie => cookie;
    zim_swoole_http_response_rawcookie => raw_cookie;
    zim_swoole_http_response_status => status;
    zim_swoole_http_response_header => header;
    zim_swoole_http_response_trailer => trailer;
    zim_swoole_http_response_ping => ping;
    zim_swoole_http_response_goaway => goaway;
    zim_swoole_http_response_write => write;
    zim_swoole_http_response_end => end;
    zim_swoole_http_response_sendfile => send_file;
    zim_swoole_http_response_redirect => redirect;
    zim_swoole_http_response_detach => detach;
    zim_swoole_http_response_create => create_response;
    zim_swoole_http_response_upgrade => upgrade;
    zim_swoole_http_response_push => push;
    zim_swoole_http_response_recv => recv;
    zim_swoole_http_response_close => close;
    zim_swoole_http_response_disconnect => disconnect;
}
