//! `Context::end`, `write`, `send_trailer` and `send_file`: the HTTP/1
//! response over the server's or the coroutine socket's send, which C++ lends
//! (`swr_http_host_send*`, the context's write buffer). The HTTP/2 server's
//! own `end`, `write` and `send_file` stay the http2 module's.
//!
//! Host calls never run while Rust holds a reference into the context: C++
//! writes some of the same fields (`keepalive`), so each step re-borrows it.
use crate::{
    header,
    host::{
        self, CO_SOCKET, Context, HTTP2, NO_PAYLOAD, swr_http_host_buffer, swr_http_host_close,
        swr_http_host_compressed_body, swr_http_host_file_type, swr_http_host_flush,
        swr_http_host_http2, swr_http_host_send, swr_http_host_sendfile,
        swr_http_host_websocket_accept, swr_http_host_websocket_handshaked,
    },
    state::State,
};
use std::{
    ffi::{c_char, c_int},
    ptr,
};
use swoole_rs_zend::{
    bailout::Bailout,
    string::bytes,
    sys::{_zval_struct__bindgen_ty_1, IS_FALSE, IS_TRUE, zend_string, zval},
};

/// `SW_HTTP_MAX_APPEND_DATA`: a larger body is sent on its own, uncopied.
const MAX_APPEND_DATA: usize = 16384;

/// `SWR_PHPHTTP_HTTP2_*`.
const HTTP2_END: c_int = 0;
const HTTP2_WRITE: c_int = 1;
const HTTP2_SEND_FILE: c_int = 2;

/// The context's `ContextState`, its first base.
pub(crate) fn state<'a>(ctx: *mut Context) -> &'a mut State {
    // SAFETY: a live `swoole::http::Context`, which starts with its
    // `ContextState`; callers drop the borrow before any host call.
    unsafe { &mut *ctx.cast::<State>() }
}

fn send(ctx: *mut Context, data: &[u8]) -> bool {
    // SAFETY: a live context and `data.len()` bytes.
    unsafe { swr_http_host_send(ctx, data.as_ptr().cast(), data.len()) }
}

/// Clear the write buffer and copy `parts` into it (no send).
fn buffer(ctx: *mut Context, [head, body, tail]: [&[u8]; 3]) {
    // SAFETY: a live context and the three spans.
    unsafe {
        swr_http_host_buffer(
            ctx,
            head.as_ptr().cast(),
            head.len(),
            body.as_ptr().cast(),
            body.len(),
            tail.as_ptr().cast(),
            tail.len(),
        );
    }
}

fn flush(ctx: *mut Context) -> bool {
    // SAFETY: a live context.
    unsafe { swr_http_host_flush(ctx) }
}

fn close(ctx: *mut Context) {
    // SAFETY: a live context.
    unsafe { swr_http_host_close(ctx) };
}

/// The HTTP/2 server's `op`.
fn http2(
    ctx: *mut Context,
    op: c_int,
    data: *mut zend_string,
    offset: i64,
    length: usize,
) -> Result<bool, Bailout> {
    // SAFETY: a live HTTP/2 context; C++ traps a bailout.
    match unsafe { swr_http_host_http2(ctx, op, data, offset, length) } {
        -1 => Err(Bailout),
        done => Ok(done == 1),
    }
}

/// The header of a `length`-byte response body into the write buffer, with
/// `body` after it when given.
fn buffer_header(
    ctx: *mut Context,
    data: *const c_char,
    length: usize,
    body: Option<&[u8]>,
) -> Result<(), Bailout> {
    header::header(state(ctx), data, length, |_, head| {
        buffer(ctx, [head, body.unwrap_or_default(), b""]);
    })
}

/// The bytes of an optional string argument.
fn of<'a>(data: *mut zend_string) -> &'a [u8] {
    if data.is_null() {
        return &[];
    }
    // SAFETY: a live string argument.
    unsafe { bytes(data) }
}

/// `Context::write`: a chunk of a chunked body, the header first.
pub(crate) fn write(ctx: *mut Context, data: *mut zend_string) -> Result<bool, Bailout> {
    if host::flags(ctx) & HTTP2 != 0 {
        return http2(ctx, HTTP2_WRITE, data, 0, 0);
    }
    if state(ctx).send_header == 0 {
        state(ctx).send_chunked = 1;
        buffer_header(ctx, ptr::null(), 0, None)?;
        if !flush(ctx) {
            let state = state(ctx);
            state.send_chunked = 0;
            state.send_header = 0;
            return Ok(false);
        }
    }
    let data = of(data);
    if data.is_empty() {
        host::report(NO_PAYLOAD, false, "the data sent must not be empty");
        return Ok(false);
    }
    // `"%x\r\n" data "\r\n"` (swoole_dec2hex: lowercase).
    let size = format!("{:x}\r\n", data.len());
    buffer(ctx, [size.as_bytes(), data, b"\r\n"]);
    Ok(flush(ctx))
}

/// `Context::send_trailer`, after the last chunk: false if it could not be
/// sent (the context is then ended and closed).
fn send_trailer(ctx: *mut Context) -> Result<bool, Bailout> {
    header::trailer(state(ctx), |trailer| buffer(ctx, [trailer, b"", b""]))?;
    if flush(ctx) {
        return Ok(true);
    }
    state(ctx).end = 1;
    close(ctx);
    Ok(false)
}

/// `Context::end`: the rest of the response, then close unless kept alive.
pub(crate) fn end(ctx: *mut Context, data: *mut zend_string) -> Result<bool, Bailout> {
    let flags = host::flags(ctx);
    if flags & HTTP2 != 0 {
        return http2(ctx, HTTP2_END, data, 0, 0);
    }
    if state(ctx).send_chunked != 0 {
        if !of(data).is_empty() && !write(ctx, data)? {
            return Ok(false);
        }
        if state(ctx).send_trailer != 0 {
            if !send(ctx, b"0\r\n") {
                return Ok(false);
            }
            send_trailer(ctx)?;
            state(ctx).send_trailer = 0;
            if state(ctx).end != 0 {
                return Ok(false);
            }
        } else if !send(ctx, b"0\r\n\r\n") {
            return Ok(false);
        }
        state(ctx).send_chunked = 0;
    } else if !body(ctx, data)? {
        return Ok(false);
    }
    if state(ctx).upgrade != 0 && flags & CO_SOCKET == 0 {
        // SAFETY: a live server context.
        unsafe { swr_http_host_websocket_handshaked(ctx) };
    }
    if state(ctx).keepalive == 0 {
        close(ctx);
    }
    state(ctx).end = 1;
    Ok(true)
}

/// The header and a whole body: one send, or two for a large body.
fn body(ctx: *mut Context, data: *mut zend_string) -> Result<bool, Bailout> {
    if state(ctx).upgrade != 0 {
        // SAFETY: a live context; offers permessage-deflate when built with zlib.
        unsafe { swr_http_host_websocket_accept(ctx) };
    }
    let raw = of(data);
    let (pointer, length) = if data.is_null() {
        (ptr::null(), 0)
    } else {
        (raw.as_ptr().cast::<c_char>(), raw.len())
    };
    let mut large = false;
    header::header(state(ctx), pointer, length, |state, head| {
        let body = sent_body(ctx, state, raw);
        large = body.len() > MAX_APPEND_DATA;
        buffer(ctx, [head, if large { b"" } else { body }, b""]);
    })?;
    if !large {
        if flush(ctx) {
            return Ok(true);
        }
        state(ctx).end = 1;
        close(ctx);
        return Ok(false);
    }
    if !flush(ctx) {
        state(ctx).send_header = 0;
        return Ok(false);
    }
    let body = sent_body(ctx, state(ctx), raw);
    if send(ctx, body) {
        return Ok(true);
    }
    state(ctx).end = 1;
    close(ctx);
    Ok(false)
}

/// The body as sent: the compressed bytes when the header compressed it.
fn sent_body<'a>(ctx: *mut Context, state: &State, raw: &'a [u8]) -> &'a [u8] {
    if raw.is_empty() || state.content_compressed == 0 {
        return raw;
    }
    let mut length = 0;
    // SAFETY: a live context whose zlib buffer `build` just filled; it lives
    // until the next compression of this context.
    let data = unsafe { swr_http_host_compressed_body(ctx, &raw mut length) };
    // SAFETY: as above, `length` bytes.
    unsafe { std::slice::from_raw_parts(data.cast(), length) }
}

/// `Context::send_file`: the header, then `length` bytes of `file` from
/// `offset`.
pub(crate) fn send_file(
    ctx: *mut Context,
    file: *mut zend_string,
    offset: i64,
    length: usize,
) -> Result<bool, Bailout> {
    if host::flags(ctx) & HTTP2 != 0 {
        return http2(ctx, HTTP2_SEND_FILE, file, offset, length);
    }
    // SAFETY: a live context and file name.
    unsafe { swr_http_host_file_type(ctx, file) };
    if state(ctx).send_header == 0 {
        state(ctx).accept_compression = 0;
        buffer_header(ctx, ptr::null(), length, None)?;
        if !flush(ctx) {
            state(ctx).send_header = 0;
            return Ok(false);
        }
    }
    // SAFETY: a live context and the checked range of a regular file.
    if length > 0 && !unsafe { swr_http_host_sendfile(ctx, file, offset, length) } {
        close(ctx);
        return Ok(false);
    }
    state(ctx).end = 1;
    if state(ctx).keepalive == 0 {
        close(ctx);
    }
    Ok(true)
}

/// `Context::end` for C++ (`Response` freed unended, `redirect`, the
/// WebSocket handshake): its result in `return_value`; true if a bailout was
/// trapped, which the caller re-raises.
/// # Safety
/// `context` is a live context, `data` a live string or null, and
/// `return_value` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_end(
    context: *mut Context,
    data: *mut zend_string,
    return_value: *mut zval,
) -> bool {
    match end(context, data) {
        Ok(done) => {
            let type_info = if done { IS_TRUE } else { IS_FALSE };
            // SAFETY: per the caller, a writable zval; `ZVAL_BOOL` sets only its type.
            unsafe { (*return_value).u1 = _zval_struct__bindgen_ty_1 { type_info } };
            false
        }
        Err(_) => true,
    }
}
