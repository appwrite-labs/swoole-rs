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
    context::{self, Ctx},
    header,
    host::{self, CO_SOCKET, HTTP2, NO_PAYLOAD},
    response::RESPONSE,
    state::State,
};
use std::{ffi::c_int, io::Write};
use swoole_rs_zend::{
    ZStr,
    bailout::{self, Bailout},
};

/// `SW_HTTP_MAX_APPEND_DATA`: a larger body is sent on its own, uncopied.
const MAX_APPEND_DATA: usize = 16384;

/// The HTTP/2 server's operations.
const HTTP2_END: c_int = 0;
const HTTP2_WRITE: c_int = 1;
const HTTP2_SEND_FILE: c_int = 2;
pub(crate) const HTTP2_PING: c_int = 3;

pub(crate) use context::{close, send};

/// The HTTP/2 server's `op`.
pub(crate) fn http2(
    ctx: Ctx,
    op: c_int,
    data: Option<&ZStr>,
    offset: i64,
    length: usize,
) -> Result<bool, Bailout> {
    let body = of(data);
    let server = crate::http2::server();
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
fn file_type(mut ctx: Ctx, file: &ZStr) {
    let response = ctx.state().response.zobject.get();
    let Some(props) = response.and_then(|response| response.props(Some(&RESPONSE))) else {
        return;
    };
    props.edit_array(c"header", |header| {
        let key = b"Content-Type";
        if header
            .array()
            .is_some_and(|header| header.find(key).is_some())
        {
            return;
        }
        let mime = swoole_rs_phpswoole::mime_type(file.bytes());
        header.insert_bytes(key, Some(&mime));
    });
}

/// Send a header without borrowing context state across the IO.
fn send_header(ctx: Ctx, length: usize) -> Result<bool, Bailout> {
    header::header(ctx, &[], length, |head| send(ctx, head))
}

/// The bytes of an optional string argument.
fn of(data: Option<&ZStr>) -> &[u8] {
    data.map_or(&[], ZStr::bytes)
}

/// `Context::write`: a chunk of a chunked body, the header first.
pub(crate) fn write(mut ctx: Ctx, data: Option<&ZStr>) -> Result<bool, Bailout> {
    if ctx.flags() & HTTP2 != 0 {
        return http2(ctx, HTTP2_WRITE, data, 0, 0);
    }
    if ctx.state().send_header == 0 {
        ctx.state().send_chunked = 1;
        if !send_header(ctx, 0)? {
            let state = ctx.state();
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
    let mut out = ctx.state().write_buffer.take().unwrap_or_default();
    out.clear();
    out.reserve(data.len() + 2 * size_of::<usize>() + 4);
    write!(out, "{:x}\r\n", data.len()).unwrap_or_default();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\r\n");
    let sent = send(ctx, &out);
    ctx.state().write_buffer = Some(out);
    Ok(sent)
}

/// `Context::send_trailer`, after the last chunk: false if it could not be
/// sent (the context is then ended and closed).
fn send_trailer(mut ctx: Ctx) -> Result<bool, Bailout> {
    if header::trailer(ctx, |trailer| send(ctx, trailer))? {
        return Ok(true);
    }
    ctx.state().end = 1;
    close(ctx);
    Ok(false)
}

/// `Context::end`: the rest of the response, then close unless kept alive.
pub(crate) fn end(mut ctx: Ctx, data: Option<&ZStr>) -> Result<bool, Bailout> {
    let flags = ctx.flags();
    if flags & HTTP2 != 0 {
        return http2(ctx, HTTP2_END, data, 0, 0);
    }
    if ctx.state().send_chunked != 0 {
        if !of(data).is_empty() && !write(ctx, data)? {
            return Ok(false);
        }
        if ctx.state().send_trailer != 0 {
            if !send(ctx, b"0\r\n") {
                return Ok(false);
            }
            send_trailer(ctx)?;
            ctx.state().send_trailer = 0;
            if ctx.state().end != 0 {
                return Ok(false);
            }
        } else if !send(ctx, b"0\r\n\r\n") {
            return Ok(false);
        }
        ctx.state().send_chunked = 0;
    } else if !body(ctx, data)? {
        return Ok(false);
    }
    if ctx.state().upgrade != 0 && flags & CO_SOCKET == 0 {
        crate::websocket::handshaked(ctx);
    }
    if ctx.state().keepalive == 0 {
        close(ctx);
    }
    ctx.state().end = 1;
    Ok(true)
}

/// The header and a whole body: one send, or two for a large body.
fn body(mut ctx: Ctx, data: Option<&ZStr>) -> Result<bool, Bailout> {
    if ctx.state().upgrade != 0 {
        crate::websocket::accept(ctx);
    }
    let raw = of(data);
    let (large, sent) = header::header(ctx, raw, raw.len(), |out| {
        let body = sent_body(ctx.state(), raw);
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
        ctx.state().end = 1;
        close(ctx);
        return Ok(false);
    }
    if !sent {
        ctx.state().send_header = 0;
        return Ok(false);
    }
    // The compressed body leaves the state: the send may yield to a coroutine
    // that reaches the same context.
    let state = ctx.state();
    let compressed = (!raw.is_empty() && state.content_compressed != 0)
        .then(|| state.compressed.take().unwrap_or_default());
    let body = compressed.as_deref().map_or(raw, Vec::as_slice);
    if send(ctx, body) {
        return Ok(true);
    }
    ctx.state().end = 1;
    close(ctx);
    Ok(false)
}

/// The body as sent: the compressed bytes when the header compressed it.
fn sent_body<'a>(state: &'a State, raw: &'a [u8]) -> &'a [u8] {
    if raw.is_empty() || state.content_compressed == 0 {
        return raw;
    }
    header::compressed(state)
}

/// `Context::send_file`: the header, then `length` bytes of `file` from
/// `offset`.
pub(crate) fn send_file(
    mut ctx: Ctx,
    file: &ZStr,
    offset: i64,
    length: usize,
) -> Result<bool, Bailout> {
    if ctx.flags() & HTTP2 != 0 {
        return http2(ctx, HTTP2_SEND_FILE, Some(file), offset, length);
    }
    file_type(ctx, file);
    if ctx.state().send_header == 0 {
        ctx.state().accept_compression = 0;
        if !send_header(ctx, length)? {
            ctx.state().send_header = 0;
            return Ok(false);
        }
    }
    if length > 0 && !context::sendfile(ctx, file, offset, length) {
        close(ctx);
        return Ok(false);
    }
    ctx.state().end = 1;
    if ctx.state().keepalive == 0 {
        close(ctx);
    }
    Ok(true)
}
