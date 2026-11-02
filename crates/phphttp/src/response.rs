//! `Swoole\Http\Response`.
use crate::cookie::{swr_v1_httpbindings_cookie_line, swr_v1_httpbindings_cookie_object_line};
use crate::{
    context::{self, Holder},
    host::{
        self, CO_SOCKET, COOKIE, Context, DETACHED, END, HEADER, HEADER_KEY_SIZE,
        HTTP_CONTEXT_UNAVAILABLE, HTTP2, SEND_CHUNKED, SEND_TRAILER, SERVER_IS_NOT_REGULAR_FILE,
        SESSION_CLOSED, TRAILER, UPGRADE, WEBSOCKET, swr_http_host_close, swr_http_host_disconnect,
        swr_http_host_end, swr_http_host_goaway, swr_http_host_mark, swr_http_host_ping,
        swr_http_host_push, swr_http_host_recv, swr_http_host_redirect,
        swr_http_host_response_create, swr_http_host_response_free,
        swr_http_host_response_property, swr_http_host_send_file, swr_http_host_set_header,
        swr_http_host_status, swr_http_host_upgrade, swr_http_host_write,
    },
};
use std::{ffi::CStr, mem::MaybeUninit, ptr};
use swoole_rs_httpbindings::swr_v1_httpbindings_file_range;
use swoole_rs_zend::string::bytes;
use swoole_rs_zend::{
    Call, Class, Invalid, declare_long, declare_null, handlers, push_string, set_key,
    sys::{
        IS_OBJECT, IS_STRING, ZEND_ACC_PUBLIC, zend_class_entry, zend_function_entry, zend_object,
        zend_string,
    },
    type_of, value_of,
};

static RESPONSE: Class = Class::new();

/// `WebSocket::OPCODE_TEXT` and `WebSocket::CLOSE_NORMAL` (RFC 6455).
const OPCODE_TEXT: i64 = 1;
const CLOSE_NORMAL: i64 = 1000;

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class or a subclass of it.
    unsafe { context::create(&RESPONSE, ce) }
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees an object this class created.
    let ctx = unsafe { *Holder::data(std) };
    // SAFETY: the context the object holds; C++ ends and releases it.
    let bailed = !ctx.is_null() && unsafe { swr_http_host_response_free(ctx) };
    // SAFETY: as above.
    unsafe { Holder::dtor(std) };
    host::raise(bailed);
}

/// MINIT: register the class as `php_swoole_http_response_minit` did.
/// # Safety
/// Called once, at MINIT, with a static `PHP_FE_END`-terminated method table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_response_minit(
    methods: *const zend_function_entry,
) -> *mut zend_class_entry {
    // SAFETY: MINIT, with the caller's table.
    unsafe {
        RESPONSE.register(c"Swoole\\Http\\Response", methods, |entry, handlers| {
            context::configure(entry, handlers, create, free);
        });
    }
    let ce = RESPONSE.entry();
    // SAFETY: MINIT, with the class just registered.
    unsafe { declare_long(ce, c"fd", 0, ZEND_ACC_PUBLIC) };
    for name in [c"socket", c"header", c"cookie", c"trailer"] {
        // SAFETY: as above.
        unsafe { declare_null(ce, name) };
    }
    crate::header::minit(ce);
    ce
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
    // SAFETY: a live context whose response object is live.
    unsafe { swr_http_host_response_property(ctx, which) }
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

fn write(call: &Call) {
    let mut bailed = false;
    let mut run = || -> Option<()> {
        let data = call.arity(1, 1).and_then(|()| call.str(1)).ok()??;
        let ctx = checked(call)?;
        // SAFETY: a live context, the argument and this call's return slot.
        bailed = unsafe { swr_http_host_write(ctx, data, call.return_value()) };
        Some(())
    };
    let done = run();
    or_false(call, done);
    host::raise(bailed);
}

fn end(call: &Call) {
    let mut bailed = false;
    let mut run = || -> Option<()> {
        let ctx = checked(call)?;
        let data = call.arity(0, 1).and_then(|()| call.str_or_null(1)).ok()?;
        // SAFETY: a live context, the optional argument and the return slot.
        bailed = unsafe { swr_http_host_end(ctx, nullable(data), call.return_value()) };
        Some(())
    };
    let done = run();
    or_false(call, done);
    host::raise(bailed);
}

/// `stat(2)` of a file name, as `php_swoole_sys_error` reports its failure.
fn stat(file: *mut zend_string) -> Result<libc::stat, ()> {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: a zend_string's bytes are NUL-terminated.
    let path = unsafe {
        CStr::from_ptr(
            file.wrapping_byte_add(std::mem::offset_of!(zend_string, val))
                .cast(),
        )
    };
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
    let mut bailed = false;
    let mut run = || -> Option<()> {
        let ctx = checked(call)?;
        if host::flags(ctx) & SEND_CHUNKED != 0 {
            host::fatal("can't use sendfile when HTTP chunk is enabled");
            return None;
        }
        let parsed = call
            .arity(1, 3)
            .and_then(|()| Ok((call.str(1)?, call.long(2, 0)?, call.long(3, 0)?)));
        let (Some(file), offset, requested) = parsed.ok()? else {
            return None;
        };
        // SAFETY: a parsed string argument lives for the call.
        let name = unsafe { bytes(file) };
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
        // SAFETY: a live context and the checked file range.
        let sent = unsafe { swr_http_host_send_file(ctx, file, offset, length) };
        bailed = sent < 0;
        call.set_bool(sent > 0);
        Some(())
    };
    let done = run();
    or_false(call, done);
    host::raise(bailed);
}

/// `http_response_set_cookie`: `cookie` encodes the value, `rawcookie` not.
fn set_cookie(call: &Call, encode: bool) {
    // Zend parses in order and stops at the first bad argument.
    let parsed = (|| -> Result<_, Invalid> {
        call.arity(1, 10)?;
        let name = call.passed(1);
        let value = call.str(2)?;
        let expires = call.long(3, 0)?;
        let path = call.str(4)?;
        let domain = call.str(5)?;
        let secure = call.bool(6, false)?;
        let http_only = call.bool(7, false)?;
        let same_site = call.str(8)?;
        let priority = call.str(9)?;
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
    let Some(ctx) = checked(call) else {
        return call.set_bool(false);
    };
    // The Set-Cookie line (crates/httpbindings), or null after a warning.
    let cookie = if kind == IS_STRING {
        // SAFETY: a passed argument is a live zval.
        let name = unsafe { value_of(name) };
        // SAFETY: a string argument holds `str_`.
        let name = unsafe { name.str_ };
        // SAFETY: the parsed arguments of this call.
        unsafe {
            swr_v1_httpbindings_cookie_line(
                u8::from(encode),
                name,
                nullable(value),
                expires,
                nullable(path),
                nullable(domain),
                u8::from(secure),
                u8::from(http_only),
                nullable(same_site),
                nullable(priority),
                u8::from(partitioned),
            )
        }
    } else {
        // SAFETY: an object argument; a non-Cookie one is reported by httpbindings.
        unsafe { swr_v1_httpbindings_cookie_object_line(name) }
    };
    if cookie.is_null() {
        return call.set_bool(false);
    }
    // SAFETY: the cookie property is a live array; it takes over the string.
    unsafe { push_string(property(ctx, COOKIE), cookie) };
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
        // SAFETY: a live context and the reason's bytes, which C++ copies.
        unsafe { swr_http_host_status(ctx, code, reason.as_ptr().cast(), reason.len()) };
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
        // SAFETY: a live context, the optional reason and the return slot.
        unsafe { swr_http_host_disconnect(ctx, code, nullable(reason), call.return_value()) };
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
        // SAFETY: a live context and this call's arguments.
        let set =
            unsafe { swr_http_host_set_header(ctx, key.as_ptr().cast(), key.len(), value, format) };
        call.set_bool(set);
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
        // SAFETY: a live context.
        unsafe { swr_http_host_mark(ctx, SEND_TRAILER) };
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
        let data = call.arity(0, 1).and_then(|()| call.str(1)).ok()?;
        if host::flags(ctx) & (HTTP2 | WEBSOCKET) == 0 {
            host::fatal("only supports websocket or http2 client");
            return None;
        }
        // SAFETY: a live context, the optional payload and the return slot.
        unsafe { swr_http_host_ping(ctx, nullable(data), call.return_value()) };
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
        // SAFETY: a live HTTP/2 context and the optional debug data.
        call.set_bool(unsafe { swr_http_host_goaway(ctx, code, nullable(debug)) });
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
        // SAFETY: a live coroutine-socket context.
        let upgraded = unsafe { swr_http_host_upgrade(ctx) };
        bailed = upgraded < 0;
        call.set_bool(upgraded > 0);
        Some(())
    };
    let done = run();
    or_false(call, done);
    host::raise(bailed);
}

fn push(call: &Call) {
    let mut bailed = false;
    let mut run = || -> Option<()> {
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
        // SAFETY: a live context, this call's arguments and its return slot.
        bailed = unsafe {
            swr_http_host_push(
                ctx,
                data,
                opcode,
                flags.unwrap_or(ptr::null_mut()),
                call.return_value(),
            )
        };
        Some(())
    };
    let done = run();
    or_false(call, done);
    host::raise(bailed);
}

fn close(call: &Call) {
    let run = || -> Option<()> {
        let ctx = open(call)?;
        // SAFETY: a live context.
        call.set_bool(unsafe { swr_http_host_close(ctx) });
        Some(())
    };
    or_false(call, run());
}

fn recv(call: &Call) {
    let run = || -> Option<()> {
        let ctx = websocket(call)?;
        let timeout = call.arity(0, 1).and_then(|()| call.double(1, 0.0)).ok()?;
        // SAFETY: a live coroutine WebSocket context and the return slot.
        unsafe { swr_http_host_recv(ctx, timeout, call.return_value()) };
        Some(())
    };
    or_false(call, run());
}

fn detach(call: &Call) {
    let run = || -> Option<()> {
        let ctx = checked(call)?;
        // SAFETY: a live context.
        unsafe { swr_http_host_mark(ctx, DETACHED) };
        call.set_bool(true);
        Some(())
    };
    or_false(call, run());
}

fn create_response(call: &Call) {
    let run = || -> Option<()> {
        let parsed = call
            .arity(1, 2)
            .and_then(|()| Ok((call.passed(1), call.long(2, -1)?)));
        let (Some(object), fd) = parsed.ok()? else {
            return None;
        };
        // SAFETY: this call's argument and return slot.
        unsafe { swr_http_host_response_create(object, fd, call.return_value()) };
        Some(())
    };
    or_false(call, run());
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
        // SAFETY: a live context, this call's arguments and its return slot.
        bailed = unsafe {
            swr_http_host_redirect(
                ctx,
                url,
                code.unwrap_or(ptr::null_mut()),
                call.return_value(),
            )
        };
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
