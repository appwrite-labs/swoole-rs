//! `Context::end`, `write`, `send_trailer` and `send_file`: the HTTP/1
//! response over the server's or the coroutine socket's send
//! (`context::send`). Chunked writes reuse their context's buffer; whole
//! responses lease the header buffer across IO. The HTTP/2 server's
//! own `end`, `write` and `send_file` stay the http2 module's.
//!
//! Host calls never run while Rust holds a reference into the context: a send
//! may yield to coroutines that reach the same context through its objects, so
//! each step re-borrows it.
use crate::{
    context, header,
    host::{self, CO_SOCKET, Context, HTTP2, NO_PAYLOAD},
    state::State,
};
use std::{
    ffi::{c_char, c_int},
    io::Write,
    ptr,
};
use swoole_rs_zend::{
    Props,
    bailout::{self, Bailout},
    string::bytes,
    sys::zend_string,
};

/// `SW_HTTP_MAX_APPEND_DATA`: a larger body is sent on its own, uncopied.
const MAX_APPEND_DATA: usize = 16384;

/// The HTTP/2 server's operations.
const HTTP2_END: c_int = 0;
const HTTP2_WRITE: c_int = 1;
const HTTP2_SEND_FILE: c_int = 2;
pub(crate) const HTTP2_PING: c_int = 3;

/// The context's `ContextState`, its first base.
pub(crate) fn state<'a>(ctx: *mut Context) -> &'a mut State {
    // SAFETY: a live `swoole::http::Context`, which starts with its
    // `ContextState`; callers drop the borrow before any host call.
    unsafe { &mut *ctx.cast::<State>() }
}

pub(crate) use context::{close, send};

/// The HTTP/2 server's `op`.
pub(crate) fn http2(
    ctx: *mut Context,
    op: c_int,
    data: *mut zend_string,
    offset: i64,
    length: usize,
) -> Result<bool, Bailout> {
    let body = of(data);
    let (server, ctx) = (crate::http2::server(), crate::http2::Ctx::of(ctx));
    let done = server.is_some_and(|server| match op {
        HTTP2_END => (server.respond)(ctx, body),
        HTTP2_WRITE => (server.write)(ctx, body),
        HTTP2_PING => (server.ping)(ctx),
        _ => (server.send_file)(ctx, body, offset, length),
    });
    if bailout::pending() {
        return Err(Bailout);
    }
    Ok(done)
}

/// The file's `Content-Type` header, unless the response has one.
fn file_type(ctx: *mut Context, file: *mut zend_string) {
    let Some(object) = state(ctx).response.zobject.object() else {
        return;
    };
    // SAFETY: the context's live response, under its registered class.
    let props = unsafe { Props::new(crate::response::entry(), object) };
    // SAFETY: nothing else borrows `header`, and no PHP code runs, until the
    // type is added.
    let header = unsafe { props.array(c"header") };
    let key = b"Content-Type";
    if header
        .array()
        .is_some_and(|header| header.find(key).is_some())
    {
        return;
    }
    // SAFETY: a live file name.
    let path = unsafe { bytes(file) };
    let mime = swoole_rs_phpswoole::mime_type(path);
    header.insert_bytes(key, Some(mime.to_bytes()));
}

/// Send a header without borrowing context state across the IO.
fn send_header(ctx: *mut Context, length: usize) -> Result<bool, Bailout> {
    header::header(ctx, ptr::null(), length, |head| send(ctx, head))
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
        if !send_header(ctx, 0)? {
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
    let mut out = state(ctx).write_buffer.take().unwrap_or_default();
    out.clear();
    out.reserve(data.len() + 2 * size_of::<usize>() + 4);
    write!(out, "{:x}\r\n", data.len()).unwrap_or_default();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\r\n");
    let sent = send(ctx, &out);
    state(ctx).write_buffer = Some(out);
    Ok(sent)
}

/// `Context::send_trailer`, after the last chunk: false if it could not be
/// sent (the context is then ended and closed).
fn send_trailer(ctx: *mut Context) -> Result<bool, Bailout> {
    if header::trailer(ctx, |trailer| send(ctx, trailer))? {
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
        crate::websocket::handshaked(ctx);
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
        crate::websocket::accept(ctx);
    }
    let raw = of(data);
    let (pointer, length) = if data.is_null() {
        (ptr::null(), 0)
    } else {
        (raw.as_ptr().cast::<c_char>(), raw.len())
    };
    let (large, sent) = header::header(ctx, pointer, length, |out| {
        let body = sent_body(state(ctx), raw);
        let large = body.len() > MAX_APPEND_DATA;
        if !large {
            out.extend_from_slice(body);
        }
        (large, send(ctx, out))
    })?;
    if !large {
        if sent {
            return Ok(true);
        }
        state(ctx).end = 1;
        close(ctx);
        return Ok(false);
    }
    if !sent {
        state(ctx).send_header = 0;
        return Ok(false);
    }
    let body = sent_body(state(ctx), raw);
    if send(ctx, body) {
        return Ok(true);
    }
    state(ctx).end = 1;
    close(ctx);
    Ok(false)
}

/// The body as sent: the compressed bytes when the header compressed it.
fn sent_body<'a>(state: &State, raw: &'a [u8]) -> &'a [u8] {
    if raw.is_empty() || state.content_compressed == 0 {
        return raw;
    }
    let compressed = header::compressed(state);
    // SAFETY: the body `header` just compressed; it lives until the next
    // compression of this context.
    unsafe { std::slice::from_raw_parts(compressed.as_ptr(), compressed.len()) }
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
    file_type(ctx, file);
    if state(ctx).send_header == 0 {
        state(ctx).accept_compression = 0;
        if !send_header(ctx, length)? {
            state(ctx).send_header = 0;
            return Ok(false);
        }
    }
    // SAFETY: a live context and the checked range of a regular file.
    if length > 0 && !context::sendfile(ctx, file, offset, length) {
        close(ctx);
        return Ok(false);
    }
    state(ctx).end = 1;
    if state(ctx).keepalive == 0 {
        close(ctx);
    }
    Ok(true)
}
