//! `Swoole\Http\Response`.
use crate::cookie::{header_line, object_header_line};
use crate::{
    context::{self, Holder},
    host::{
        self, CO_SOCKET, COOKIE, Context, DETACHED, END, HEADER, HEADER_KEY_SIZE,
        HTTP_CONTEXT_UNAVAILABLE, HTTP2, SEND_CHUNKED, SERVER_IS_NOT_REGULAR_FILE, SESSION_CLOSED,
        TRAILER, UPGRADE, WEBSOCKET,
    },
    send, websocket,
};
use std::{
    ffi::{CStr, c_char, c_int, c_void},
    mem::MaybeUninit,
    ptr,
    sync::atomic::Ordering::Relaxed,
};
use swoole_rs_httpbindings::{canonical_case, swr_v1_httpbindings_file_range};
use swoole_rs_phpstubs::http_response;
use swoole_rs_zend::{
    Call, Class, Invalid, Owned, Props, Value, ZStr, ZString, declare_long, declare_property_null,
    handlers, init_object, push_string, set_key,
    sys::{
        IS_NULL, IS_OBJECT, IS_STRING, ZEND_ACC_PUBLIC, zend_class_entry, zend_long, zend_object,
        zend_string,
    },
    type_of,
    value::obj_of,
};
use swoole_rs_zend::{bailout::Bailout, string::bytes};
use swoole_rs_zend::{method_table, sys as z};

static RESPONSE: Class = Class::new();

/// `WebSocket::OPCODE_TEXT` and `WebSocket::CLOSE_NORMAL` (RFC 6455).
const OPCODE_TEXT: i64 = 1;
const CLOSE_NORMAL: i64 = 1000;
/// `SW_ERROR_WEBSOCKET_PACK_FAILED`.
const WEBSOCKET_PACK_FAILED: c_int = 8505;
/// `SW_HTTP_INTERNAL_SERVER_ERROR`.
const INTERNAL_SERVER_ERROR: c_int = 500;

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class or a subclass of it.
    unsafe { context::create(&RESPONSE, ce) }
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees an object this class created.
    let ctx = unsafe { *Holder::data(std) };
    let bailed = !ctx.is_null() && release(ctx);
    // SAFETY: as above.
    unsafe { Holder::dtor(std) };
    host::raise(bailed);
}

unsafe extern "C" {
    /// `sw_reactor()`.
    fn swr_v1_event_reactor() -> *mut c_void;
    /// The HTTP/2 server's (crates/phphttp2).
    fn swr_v1_phphttp2_server_goaway(
        ctx: *mut Context,
        code: i64,
        data: *const c_char,
        length: usize,
    ) -> bool;
}

/// `Swoole\Http\Response`, as C++ reads it; MINIT writes it.
#[unsafe(no_mangle)]
pub static mut swoole_http_response_ce: *mut zend_class_entry = ptr::null_mut();

/// The response object is going: finish what the handler left unanswered
/// (500 when no status was set) and release the context. True if user code
/// bailed out on the way.
fn release(ctx: *mut Context) -> bool {
    context::after_response(ctx);
    let mut bailed = false;
    let state = send::state(ctx);
    if state.end == 0
        && (state.send_chunked != 0 || state.send_header == 0)
        && state.detached == 0
        // SAFETY: reads a thread global.
        && !unsafe { swr_v1_event_reactor() }.is_null()
    {
        if state.response.status == 0 {
            state.response.status = INTERNAL_SERVER_ERROR;
        }
        if context::available(ctx) {
            bailed = send::end(ctx, ptr::null_mut()).is_err();
        }
    }
    send::state(ctx).response.zobject.ptr = ptr::null_mut();
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
/// # Safety
/// Called once, at MINIT.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_response_minit(codecs: u32) {
    // SAFETY: MINIT, with a static table.
    unsafe {
        RESPONSE.register(
            c"Swoole\\Http\\Response",
            METHODS.as_ptr(),
            |entry, handlers| {
                context::configure(entry, handlers, create, free);
            },
        );
    }
    let ce = RESPONSE.entry();
    // SAFETY: MINIT, with the class just registered.
    unsafe { declare_long(ce, c"fd", 0, ZEND_ACC_PUBLIC) };
    for name in [c"socket", c"header", c"cookie", c"trailer"] {
        // SAFETY: as above.
        unsafe { declare_property_null(ce, name, swoole_rs_zend::sys::ZEND_ACC_PUBLIC) };
    }
    crate::header::minit(ce);
    context::response_minit(ce);
    // SAFETY: MINIT, before any thread reads it.
    unsafe { swoole_http_response_ce = ce };
    crate::parse::minit(codecs);
}

/// The registered class entry.
pub(crate) fn entry() -> *mut zend_class_entry {
    RESPONSE.entry()
}

/// `http_response_get_and_check_context`: a context that can still respond.
fn checked(call: &Call) -> Option<*mut Context> {
    let ctx = context::this(call);
    if ctx.is_null() || host::flags(ctx) & (END | DETACHED) != 0 {
        host::set_last_error(HTTP_CONTEXT_UNAVAILABLE);
        return None;
    }
    Some(ctx)
}

/// `php_swoole_http_response_get_context`, setting `SW_ERROR_SESSION_CLOSED`
/// when there is none.
fn open(call: &Call) -> Option<*mut Context> {
    let ctx = context::this(call);
    if ctx.is_null() {
        host::set_last_error(SESSION_CLOSED);
        return None;
    }
    Some(ctx)
}

/// The coroutine WebSocket checks `push`, `recv` and `disconnect` share.
fn websocket(call: &Call) -> Option<*mut Context> {
    let ctx = open(call)?;
    let flags = host::flags(ctx);
    if flags & CO_SOCKET == 0 || flags & UPGRADE == 0 {
        host::fatal(&format!(
            "fd[{}] is not a websocket conncetion",
            host::fd(ctx)
        ));
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

fn nullable(string: Option<*mut zend_string>) -> *mut zend_string {
    string.unwrap_or(ptr::null_mut())
}

fn property(ctx: *mut Context, which: i32) -> *mut swoole_rs_zend::sys::zval {
    crate::header::response_property(send::state(ctx), which)
}

fn init_header(call: &Call) {
    or_false(
        call,
        checked(call).map(|ctx| {
            for which in [HEADER, COOKIE, TRAILER] {
                property(ctx, which);
            }
            call.set_bool(true);
        }),
    );
}

fn is_writable(call: &Call) {
    let ctx = context::this(call);
    call.set_bool(!ctx.is_null() && host::flags(ctx) & (END | DETACHED) == 0);
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
        let data = call.arity(1, 1).and_then(|()| call.str(1)).ok()??;
        let ctx = checked(call)?;
        // Compressing a chunked body would compress the chunks, not the stream.
        send::state(ctx).accept_compression = 0;
        Some(send::write(ctx, data))
    };
    match run() {
        Some(result) => sent(call, &result),
        None => call.set_bool(false),
    }
}

/// `PHP_SWOOLE_HOOK_AFTER_RESPONSE` (`SW_GLOBAL_HOOK_USER + 3`).
const HOOK_AFTER_RESPONSE: c_int = 27;

/// `http_response_after`: the server's and the hooks' after-response step.
fn after(ctx: *mut Context) -> Result<(), Bailout> {
    unsafe extern "C" fn hooks(ctx: *mut c_void) {
        // SAFETY: the hooks take the context, as upstream passed it. The
        // frame owns nothing a bailout out of a hook would skip.
        unsafe { swoole_rs_corebase::swoole_call_hook(HOOK_AFTER_RESPONSE, ctx) };
    }
    context::after_response(ctx);
    if !swoole_rs_corebase::swoole_isset_hook(HOOK_AFTER_RESPONSE) {
        return Ok(());
    }
    // SAFETY: a live context; the trap's setjmp sits in C, below `hooks`.
    unsafe { swoole_rs_zend::bailout::trap(hooks, ctx) }
}

fn end(call: &Call) {
    let run = || -> Option<Result<bool, Bailout>> {
        let ctx = checked(call)?;
        let data = call.arity(0, 1).and_then(|()| call.str_or_null(1)).ok()?;
        Some(after(ctx).and_then(|()| send::end(ctx, nullable(data))))
    };
    match run() {
        Some(result) => sent(call, &result),
        None => call.set_bool(false),
    }
}

/// `stat(2)` of a file name, as `php_swoole_sys_error` reports its failure.
fn stat(file: &ZStr) -> Result<libc::stat, ()> {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    let path = file.c_str();
    // SAFETY: a NUL-terminated path and a writable buffer.
    if unsafe { libc::stat(path.as_ptr(), stat.as_mut_ptr()) } == 0 {
        // SAFETY: stat succeeded, so it filled the buffer.
        return Ok(unsafe { stat.assume_init() });
    }
    let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    // SAFETY: strerror accepts any error number.
    let reason = unsafe { libc::strerror(error) };
    // SAFETY: strerror returns a NUL-terminated message.
    let reason = unsafe { CStr::from_ptr(reason) };
    let mut message = b"stat(".to_vec();
    message.extend_from_slice(path.to_bytes());
    message.extend_from_slice(b") failed, Error: ");
    message.extend_from_slice(reason.to_bytes());
    message.extend_from_slice(format!("[{error}]").as_bytes());
    host::report(error, false, message);
    Err(())
}

fn send_file(call: &Call) {
    let run = || -> Option<Result<bool, Bailout>> {
        let ctx = checked(call)?;
        if host::flags(ctx) & SEND_CHUNKED != 0 {
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
        // SAFETY: a writable local.
        match unsafe {
            swr_v1_httpbindings_file_range(stat.st_size, offset, requested, &raw mut length)
        } {
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
        Some(after(ctx).and_then(|()| send::send_file(ctx, file.as_ptr(), offset, length)))
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
        let name = call.passed(1);
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
    // SAFETY: a passed argument is a live zval.
    let kind = unsafe { type_of(name) };
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
        // SAFETY: an object argument; a non-Cookie one is reported by httpbindings.
        unsafe { object_header_line(name) }
    };
    let Some(cookie) = cookie else {
        return call.set_bool(false);
    };
    let Some(ctx) = checked(call) else {
        return call.set_bool(false);
    };
    // SAFETY: the cookie property is a live array; it takes over the string.
    unsafe { push_string(property(ctx, COOKIE), cookie.into_raw()) };
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
        let ctx = checked(call)?;
        let reason = reason.unwrap_or_default();
        let response = &mut send::state(ctx).response;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream assigns it to an int"
        )]
        let code = code as i32;
        response.status = code;
        response.reason = if reason.is_empty() {
            ptr::null_mut()
        } else {
            // SAFETY: the reason's bytes, copied into request memory.
            unsafe { z::swr_zend_http_estrndup(reason.as_ptr().cast(), reason.len()) }
        };
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
            .and_then(|()| Ok((call.long(1, CLOSE_NORMAL)?, call.str(2)?)));
        let (code, reason) = parsed.ok()?;
        call.set_bool(websocket::response_disconnect(ctx, code, nullable(reason)));
        Some(())
    };
    or_false(call, run());
}

fn header(call: &Call) {
    let run = || -> Option<()> {
        let parsed = call
            .arity(2, 3)
            .and_then(|()| Ok((call.string_opt(1)?, call.passed(2), call.bool(3, true)?)));
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
        let ctx = checked(call)?;
        if key.len() > HEADER_KEY_SIZE - 1 {
            host::notice("trailer key is too long");
            return None;
        }
        let key = key.to_ascii_lowercase();
        // SAFETY: the trailer property is a live array.
        unsafe { set_key(property(ctx, TRAILER), &key, value) };
        send::state(ctx).send_trailer = 1;
        call.set_bool(true);
        Some(())
    };
    or_false(call, run());
}

fn ping(call: &Call) {
    let run = || -> Option<()> {
        let ctx = context::this(call);
        if ctx.is_null() {
            return None;
        }
        let data = call.arity(0, 1).and_then(|()| call.zstr(1)).ok()?;
        let flags = host::flags(ctx);
        if flags & (HTTP2 | WEBSOCKET) == 0 {
            host::fatal("only supports websocket or http2 client");
            return None;
        }
        if flags & HTTP2 != 0 {
            match send::http2(ctx, send::HTTP2_PING, ptr::null_mut(), 0, 0) {
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
        if host::flags(ctx) & HTTP2 == 0 {
            host::fatal(&format!("fd[{}] is not a HTTP2 conncetion", host::fd(ctx)));
            return None;
        }
        let parsed = call
            .arity(0, 2)
            .and_then(|()| Ok((call.long(1, 0)?, call.str_or_null(2)?)));
        let (code, debug) = parsed.ok()?;
        let debug = nullable(debug);
        let (data, length) = if debug.is_null() {
            (ptr::null(), 0)
        } else {
            // SAFETY: a live string argument.
            let debug = unsafe { bytes(debug) };
            (debug.as_ptr().cast(), debug.len())
        };
        // SAFETY: a live HTTP/2 context and the debug data's span.
        call.set_bool(unsafe { swr_v1_phphttp2_server_goaway(ctx, code, data, length) });
        Some(())
    };
    or_false(call, run());
}

fn upgrade(call: &Call) {
    let mut bailed = false;
    let mut run = || -> Option<()> {
        let ctx = checked(call)?;
        if host::flags(ctx) & CO_SOCKET == 0 {
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
                call.passed(1),
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
        let ctx = websocket(call)?;
        let timeout = call.arity(0, 1).and_then(|()| call.double(1, 0.0)).ok()?;
        let state = send::state(ctx);
        let return_value = call.return_value();
        let (settings, socket) = (u32::from(state.websocket_settings), state.private_data);
        websocket::recv(
            settings,
            &mut state.ws_buffer,
            socket,
            return_value,
            timeout,
        );
        // SAFETY: the frame, or the failure, `recv` left in the return slot.
        match unsafe { type_of(return_value) } {
            // `""`: the peer closed.
            IS_STRING => {
                context::close(ctx);
            }
            IS_NULL => call.set_bool(false),
            _ => {}
        }
        Some(())
    };
    or_false(call, run());
}

fn detach(call: &Call) {
    let run = || -> Option<()> {
        let ctx = checked(call)?;
        send::state(ctx).detached = 1;
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
pub(crate) fn set_header(ctx: *mut Context, key: &[u8], value: *mut z::zval, format: bool) -> bool {
    if key.len() > HEADER_KEY_SIZE - 1 {
        host::notice("header key is too long");
        return false;
    }
    if crate::header::has_crlf(key) {
        return false;
    }
    let header = property(ctx, HEADER);
    let mut buffer = [0; HEADER_KEY_SIZE];
    // Formatted in a stack copy, as upstream does in its thread buffer.
    let name = match buffer.get_mut(..key.len()) {
        Some(name) if format => {
            name.copy_from_slice(key);
            if host::flags(ctx) & HTTP2 != 0 {
                name.make_ascii_lowercase();
            } else {
                canonical_case(name);
            }
            &*name
        }
        _ => key,
    };
    // SAFETY: the value the array now also references.
    unsafe { z::swr_zval_try_addref(value) };
    // SAFETY: the live header array and the name's bytes, which Zend copies.
    unsafe { z::add_assoc_zval_ex(header, name.as_ptr().cast(), name.len(), value) };
    true
}

/// `Context::set_header(key, value, false)` of a string value. A warning's
/// bailout waits for the caller's last step.
pub(crate) fn set_header_bytes(ctx: *mut Context, key: &[u8], value: &[u8]) {
    // This frame's reference; the header array takes its own.
    let value = Owned::string(ZString::new(value));
    set_header(ctx, key, value.as_ptr(), false);
}

/// `php_swoole_fatal_error(E_WARNING, ...)`, then `RETURN_FALSE`.
fn refuse(call: &Call, message: &str) {
    host::fatal(message);
    call.set_bool(false);
}

/// An object of `ce` or a subclass.
fn instance_of(object: &Value, ce: *mut zend_class_entry) -> bool {
    // SAFETY: a live class and a registered one.
    object
        .class()
        .is_some_and(|class| unsafe { z::swr_zend_instanceof(class, ce) })
}

/// `Response::create($server|$socket|[$server|$socket, $request]|$fd, $fd)`:
/// a response on a server connection or a coroutine socket, sharing the
/// request's context when one is given.
fn respond_on(call: &Call, mut object: &Value, mut fd: zend_long) {
    let mut paired = false;
    let mut ctx: *mut Context = ptr::null_mut();
    let mut server: *mut c_void = ptr::null_mut();
    let mut socket = ptr::null_mut();
    if let Some(pair) = object.array() {
        let Some(request) = pair
            .index(1)
            .filter(|found| instance_of(found, crate::request::entry()))
            .and_then(Value::object)
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
        // SAFETY: a `Swoole\Http\Request`.
        ctx = unsafe { context::context_of(request) };
    }
    if object.object().is_some() {
        let server_ce = swoole_rs_phpserver::server_class_entry();
        let socket_ce = swoole_rs_phpclients::socket_class();
        if instance_of(object, server_ce) {
            let Some(checked) = websocket::server_of(object.as_ptr()) else {
                return;
            };
            server = checked;
            if !websocket::server_connection(server, fd) {
                return refuse(
                    call,
                    &format!("parameter $2 ({fd}) must be valid connection session_id"),
                );
            }
        } else if instance_of(object, socket_ce) {
            socket = object.as_ptr();
            // SAFETY: a `Swoole\Coroutine\Socket`.
            fd = zend_long::from(websocket::sock_fd(unsafe {
                swoole_rs_phpclients::get_socket(socket)
            }));
        } else {
            return refuse(
                call,
                "parameter $1 must be instanceof Server or Coroutine\\Socket",
            );
        }
    } else {
        fd = object.long();
        server = swoole_rs_corebase::swr_v1_core_server();
    }
    if !server.is_null() && !websocket::started(server) {
        return refuse(call, "server is not running");
    }
    if server.is_null() && socket.is_null() {
        call.set_bool(false);
        return;
    }
    let ctx = if ctx.is_null() {
        let ctx = context::alloc();
        send::state(ctx).keepalive = 1;
        if server.is_null() {
            crate::parse::init_request_parser(ctx);
            // `Context::init(zval *)`.
            let state = send::state(ctx);
            state.parse_cookie = 1;
            state.parse_body = 1;
            state.parse_files = 1;
            if crate::parse::CAPABILITIES.load(Relaxed) & 8 != 0 {
                state.enable_compression = 1;
                state.compression_level = 1;
            }
        } else {
            // SAFETY: the live server.
            unsafe { context::init(ctx, server) };
        }
        ctx
    } else {
        ctx
    };
    // SAFETY: a live context, and the server or the socket.
    unsafe { context::bind(ctx, server, socket) };
    let response = call.return_value();
    let ce = RESPONSE.entry();
    // SAFETY: the return slot and this registered class.
    unsafe { init_object(response, ce) };
    // SAFETY: the object just created.
    let std = unsafe { obj_of(response) };
    // SAFETY: a `Swoole\Http\Response`.
    unsafe { context::set_context(std, ctx) };
    let state = send::state(ctx);
    state.fd = fd;
    // SAFETY: the object zval just created.
    state.response.zobject.value = unsafe { *response };
    state.response.zobject.ptr = &raw mut state.response.zobject.value;
    // SAFETY: the Response just created, under its class.
    let props = unsafe { Props::new(ce, std) };
    props.long(c"fd", fd);
    // SAFETY: null or the caller's live socket; the property takes its own reference.
    if let Some(socket) = unsafe { Value::from_ptr(socket) } {
        props.value(c"socket", socket);
    }
    if paired && let Some(request) = send::state(ctx).request.zobject.props() {
        request.long(c"fd", fd);
    }
}

fn redirect(call: &Call) {
    let mut bailed = false;
    let mut run = || -> Option<()> {
        let parsed = call
            .arity(1, 2)
            .map(|()| (call.passed(1), call.zval_or_null(2)));
        let (Some(url), code) = parsed.ok()? else {
            return None;
        };
        let ctx = checked(call)?;
        #[expect(clippy::cast_possible_truncation, reason = "upstream's int status")]
        let status = code.map_or(302, |code| {
            // SAFETY: the argument.
            unsafe { z::swr_zval_get_long(code) as c_int }
        });
        send::state(ctx).response.status = status;
        set_header(ctx, b"Location", url, false);
        if swoole_rs_zend::bailout::pending() {
            bailed = true;
            return Some(());
        }
        match send::end(ctx, ptr::null_mut()) {
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
