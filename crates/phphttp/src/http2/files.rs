//! The HTTP/2 server's file responses: the static handler's (upstream's
//! `http2_server_is_static_file` and `http2_server_send_range_file`) and
//! `Response::sendfile`'s (`http2_server_send_file`), over crates/server's
//! `StaticHandler`.
use super::server::{
    Value, begin_file, finish_file, of, respond, respond_with_length, send_body, set,
};
use crate::http2::{Ctx, Field};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::offload::async_job;
use swoole_rs_http::PreconditionStatus;
use swoole_rs_http::status::{BAD_REQUEST, NOT_FOUND, PARTIAL_CONTENT, RANGE_NOT_SATISFIABLE};
use swoole_rs_server::static_handler::{StaticHandler, page_404};
use swoole_rs_server::static_select::Handler as _;
use swoole_rs_server::view;
use swoole_rs_zend::bailout;

/// Read the entire advertised range before any of its bytes are sent.
fn read_range(fd: libc::c_int, offset: i64, buf: &mut [u8], pooled: bool) -> bool {
    let length = buf.len();
    let mut used = 0;
    let mut read = || {
        while used < length {
            let Some(at) = offset.checked_add_unsigned(used as u64) else {
                break;
            };
            match swoole_rs_corebase::sys::pread(fd, buf.split_at_mut(used).1, at) {
                Some(0) => break,
                Some(n) => used += n,
                None if std::io::Error::last_os_error().kind()
                    == std::io::ErrorKind::Interrupted => {}
                None => break,
            }
        }
    };
    if pooled {
        if !async_job(&mut read) {
            return false;
        }
    } else {
        read();
    }
    used == length
}

/// The text up to its first NUL, as a C string reads it.
fn c_text(text: &[u8]) -> &[u8] {
    text.split(|&byte| byte == 0).next().unwrap_or_default()
}

/// A string entry of one of the request's arrays (`$server`, `$header`).
fn entry(ctx: Ctx, array: Value, key: &[u8]) -> Option<Vec<u8>> {
    Some(ctx.entry(array, key)?.bytes()?.to_vec())
}

fn body(ctx: Ctx, data: &[u8], end_stream: bool) -> bool {
    of(ctx).is_some_and(|(session, stream)| send_body(ctx, &session, &stream, data, end_stream))
}

/// `http2_server_is_static_file`: answer the request from the document root
/// when the static handler serves it (true). `server` is the live server.
/// # Errors
/// Serving it left a bailout pending; it stays pending for the caller.
pub(crate) fn static_file(server: view::Server<'_>, ctx: Ctx) -> Result<bool, bailout::Bailout> {
    bailout::outcome(serve(server, ctx))
}

fn serve(server: view::Server<'_>, ctx: Ctx) -> bool {
    let Some(method) = entry(ctx, Value::Server, b"request_method") else {
        return false;
    };
    let head = method.eq_ignore_ascii_case(b"HEAD");
    if !head && !method.eq_ignore_ascii_case(b"GET") {
        return false;
    }
    let Some(uri) = ctx.request_target() else {
        return false;
    };
    // Borrowed where it was made: moving the handler out of the `Option` copies it.
    let mut made = StaticHandler::new(server, &uri);
    let Some(handler) = made.as_mut() else {
        return false;
    };
    if !handler.try_serve() {
        return false;
    }
    let status = handler.status();
    let redirect = handler.directory_redirect();
    if status == 200 && (!handler.try_serve_index_file() || !handler.open_file()) {
        return false;
    }
    ctx.unhook();
    set(ctx, Field::AcceptCompression, 0);
    if matches!(status, NOT_FOUND | BAD_REQUEST) {
        set(ctx, Field::Status, status.cast_unsigned());
        let body = if status == NOT_FOUND {
            page_404()
        } else {
            Vec::new()
        };
        respond_with_length(ctx, if head { &[] } else { &body }, body.len());
        return true;
    }
    if let Some(location) = redirect {
        ctx.set_header(b"Location", &location);
        set(ctx, Field::Status, 301);
        respond(ctx, &[]);
        return true;
    }
    ctx.set_header(b"Last-Modified", &handler.date_last_modified());
    ctx.set_header(b"ETag", handler.etag().as_bytes());
    let header = |key: &[u8]| entry(ctx, Value::Header, key).unwrap_or_default();
    let condition = handler.preconditions(
        &header(b"if-match"),
        &header(b"if-unmodified-since"),
        &header(b"if-none-match"),
        &header(b"if-modified-since"),
    );
    if condition != PreconditionStatus::Ok {
        set(ctx, Field::Status, condition as u32);
        if condition != PreconditionStatus::NotModified {
            ctx.set_header(b"Content-Length", b"0");
        }
        respond(ctx, &[]);
        return true;
    }
    if handler.lists_directory() {
        let listing = handler.index_page();
        respond_with_length(ctx, if head { &[] } else { &listing }, listing.len());
        return true;
    }
    let (range, if_range) = (
        entry(ctx, Value::Header, b"range"),
        entry(ctx, Value::Header, b"if-range"),
    );
    handler.parse_range(
        range.as_deref().map(c_text),
        if_range.as_deref().map(c_text),
    );
    let status = handler.status();
    set(
        ctx,
        Field::Status,
        u32::try_from(status).unwrap_or_default(),
    );
    if status == RANGE_NOT_SATISFIABLE {
        ctx.set_header(
            b"Content-Range",
            format!("bytes */{}", handler.filesize()).as_bytes(),
        );
        ctx.set_header(b"Content-Length", b"0");
        respond(ctx, &[]);
        return true;
    }
    let tasks = handler.tasks();
    if let [task] = tasks.as_slice() {
        if status == PARTIAL_CONTENT {
            let range = content_range(task.offset, task.length, handler.filesize());
            ctx.set_header(b"Content-Range", range.as_bytes());
        } else {
            ctx.set_header(b"Accept-Ranges", b"bytes");
        }
    }
    if head {
        ctx.set_header(b"Content-Type", &handler.content_type());
        respond_with_length(ctx, &[], handler.content_length());
        return true;
    }
    send_ranges(ctx, handler);
    true
}

/// The single range's `Content-Range`, as upstream's stringstream writes it
/// (`length + offset - 1` in `size_t`, as in v6.3).
fn content_range(offset: i64, length: usize, size: usize) -> String {
    let last = (length as u64)
        .wrapping_add(offset.c_cast::<u64>())
        .wrapping_sub(1);
    format!("bytes {offset}-{last}/{size}")
}

/// `http2_server_send_range_file`: the file's ranges, the stream (and so the
/// context) held while a coroutine reads it.
fn send_ranges(ctx: Ctx, handler: &mut StaticHandler<'_>) -> bool {
    let _held = of(ctx);
    let deadline = ctx.send_deadline();
    let Some(trailer) = begin_file(ctx, &handler.content_type(), handler.content_length()) else {
        return false;
    };
    let end_stream = !trailer;
    let tasks = handler.tasks();
    let multipart = tasks.len() > 1;
    let mut error = false;
    for task in &tasks {
        if multipart && !body(ctx, c_text(&task.part_header), false) {
            error = true;
            break;
        }
        if task.length > 0 {
            let pooled = !multipart && swoole_rs_phpcoroutines::in_coroutine();
            let mut buf = vec![0; task.length];
            if !handler
                .file()
                .is_some_and(|f| read_range(f.descriptor(), task.offset, &mut buf, pooled))
                || !body(ctx, &buf, !multipart && end_stream)
            {
                error = true;
                break;
            }
        }
    }
    if multipart && !error {
        error = !body(ctx, &handler.end_part(), end_stream);
    }
    finish_file(ctx, trailer, error, deadline);
    true
}

/// `swoole_http2_server_send_file`: `Response::sendfile` over HTTP/2, the
/// stream held while a coroutine reads the file.
pub(crate) fn send_file(ctx: Ctx, path: &[u8], offset: i64, length: usize) -> bool {
    let Some((session, stream)) = of(ctx) else {
        return false;
    };
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(std::ffi::OsStr::from_bytes(path))
    else {
        return false;
    };
    let deadline = ctx.send_deadline();
    let Some(trailer) = begin_file(ctx, &swoole_rs_framing::mime::get(path), length) else {
        return false;
    };
    let mut buf =
        vec![0; length.min(swoole_rs_corebase::constants::SW_SENDFILE_CHUNK_SIZE as usize)];
    let pooled = swoole_rs_phpcoroutines::in_coroutine();
    let mut sent = 0;
    while sent < length {
        let n = buf.len().min(length - sent);
        let chunk = buf.split_at_mut(n).0;
        let Some(at) = offset.checked_add_unsigned(sent as u64) else {
            break;
        };
        if !read_range(file.as_raw_fd(), at, chunk, pooled)
            || !send_body(
                ctx,
                &session,
                &stream,
                chunk,
                !trailer && n == length - sent,
            )
        {
            break;
        }
        sent += n;
    }
    finish_file(ctx, trailer, sent != length, deadline)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_is_written_as_upstreams_stringstream_writes_it() {
        assert_eq!(content_range(0, 100, 1000), "bytes 0-99/1000");
        assert_eq!(content_range(900, 100, 1000), "bytes 900-999/1000");
        assert_eq!(content_range(0, 0, 1), format!("bytes 0-{}/1", u64::MAX));
    }
}
