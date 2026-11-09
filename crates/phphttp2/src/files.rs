//! The HTTP/2 server's file responses: the static handler's (upstream's
//! `http2_server_is_static_file` and `http2_server_send_range_file`) and
//! `Response::sendfile`'s (`http2_server_send_file`), over crates/server's
//! `StaticHandler`.
use crate::server::{
    Value, begin_file, current, finish_file, of, respond, send_body, set, zval_of,
};
use libc::c_void;
use std::ffi::CString;
use swoole_rs_hooks::offload::async_job;
use swoole_rs_http2::{sw_rs_http2_file_length, sw_rs_http2_file_mode};
use swoole_rs_phphttp::http2::Field;
use swoole_rs_server::static_handler::{StaticHandler, page_404};
use swoole_rs_zend::{
    bailout, string,
    sys::{IS_STRING, zend_hash_str_find, zend_string, zval},
    type_of, value,
};

/// `sw_rs_http2_file_mode`'s modes.
const MODE_EMPTY: u32 = 0;
const MODE_MULTI: u32 = 1;
const MODE_SINGLE: u32 = 2;
/// `SW_HTTP_NOT_FOUND`, `SW_HTTP_NOT_MODIFIED` and `SW_HTTP_PARTIAL_CONTENT`.
const NOT_FOUND: i32 = 404;
const NOT_MODIFIED: u32 = 304;
const PARTIAL_CONTENT: i32 = 206;

/// A file opened read-only, closed on drop.
struct File(libc::c_int);

impl File {
    fn open(path: &[u8]) -> Option<Self> {
        let path = CString::new(path).ok()?;
        // SAFETY: a NUL-terminated path.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) };
        (fd >= 0).then_some(Self(fd))
    }

    /// One read of up to `buf.len()` bytes at `offset`, on the thread pool
    /// when `pooled`; `None` if it failed.
    fn read(&self, offset: i64, buf: &mut [u8], pooled: bool) -> Option<usize> {
        let mut read = Read {
            fd: self.0,
            offset,
            buf: buf.as_mut_ptr(),
            length: buf.len(),
            result: -1,
        };
        let context = (&raw mut read).cast();
        if pooled {
            // SAFETY: the read's context outlives the job, which the coroutine waits for.
            if !unsafe { async_job(read_job, context) } {
                return None;
            }
        } else {
            // SAFETY: as above, on this thread.
            unsafe { read_job(context) };
        }
        usize::try_from(read.result).ok()
    }
}

impl Drop for File {
    fn drop(&mut self) {
        // SAFETY: the descriptor `open` made, closed once.
        unsafe { libc::close(self.0) };
    }
}

/// One `pread` for [`File::read`].
struct Read {
    fd: libc::c_int,
    offset: i64,
    buf: *mut u8,
    length: usize,
    result: isize,
}

unsafe extern "C" fn read_job(context: *mut c_void) {
    // SAFETY: the `Read` `File::read` lends.
    let read = unsafe { &mut *context.cast::<Read>() };
    // SAFETY: the buffer's `length` writable bytes.
    read.result = unsafe { libc::pread(read.fd, read.buf.cast(), read.length, read.offset) };
}

/// The text up to its first NUL, as a C string reads it.
fn c_text(text: &[u8]) -> &[u8] {
    text.split(|&byte| byte == 0).next().unwrap_or_default()
}

fn set_header(ctx: *mut c_void, name: &[u8], value: &[u8]) {
    swoole_rs_phphttp::set_header_bytes(ctx, name, value);
}

/// A string entry of an array zval.
fn entry(array: *mut zval, key: &[u8]) -> Option<*mut zend_string> {
    // SAFETY: a live array.
    let table = unsafe { value::table(array) };
    // SAFETY: a live table and the key's bytes.
    let found = unsafe { zend_hash_str_find(table, key.as_ptr().cast(), key.len()) };
    // SAFETY: a found entry is live.
    (!found.is_null() && unsafe { type_of(found) } == IS_STRING).then(|| {
        // SAFETY: a string zval.
        unsafe { value::str_of(found) }
    })
}

fn body(ctx: *mut c_void, data: &[u8], end_stream: bool) -> bool {
    of(ctx).is_some_and(|(session, stream)| send_body(ctx, &session, &stream, data, end_stream))
}

/// `http2_server_is_static_file`: answer the request from the document root
/// when the static handler serves it (true).
/// # Safety
/// `server` is the live server and `ctx` a live HTTP/2 stream context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp2_server_static(
    server: *mut c_void,
    ctx: *mut c_void,
) -> bool {
    let served = serve(server, ctx);
    bailout::finish();
    served
}

fn serve(server: *mut c_void, ctx: *mut c_void) -> bool {
    let zserver = zval_of(ctx, Value::Server);
    let Some(uri) = entry(zserver, b"request_uri") else {
        return false;
    };
    // SAFETY: the request's live string.
    let uri = unsafe { string::bytes(uri) };
    // SAFETY: the live server.
    let Some(handler) = (unsafe { StaticHandler::new(server, uri) }) else {
        return false;
    };
    if !handler.try_serve() {
        return false;
    }
    if handler.status() == NOT_FOUND {
        set(ctx, Field::Status, 404);
        respond(ctx, &page_404());
        return true;
    }
    // if http_index_files is enabled, need to search the index file first.
    if !handler.try_serve_index_file() {
        return false;
    }
    // no index file in the directory: with http_autoindex, list the directory.
    if handler.lists_directory() {
        respond(ctx, &handler.index_page());
        return true;
    }
    set_header(ctx, b"Last-Modified", &handler.date_last_modified());
    let zheader = zval_of(ctx, Value::Header);
    if let Some(since) = entry(zheader, b"if-modified-since") {
        // SAFETY: the header's live string.
        let since = unsafe { string::bytes(since) };
        if !since.is_empty() && handler.is_modified(since) {
            set(ctx, Field::Status, NOT_MODIFIED);
            return true;
        }
    }
    let text = |key: &[u8]| {
        entry(zheader, key).map(|found| {
            // SAFETY: a live header string.
            c_text(unsafe { string::bytes(found) })
        })
    };
    handler.parse_range(text(b"range"), text(b"if-range"));
    let status = handler.status();
    set(
        ctx,
        Field::Status,
        u32::try_from(status).unwrap_or_default(),
    );
    let tasks = handler.tasks();
    if let [task] = tasks.as_slice() {
        if status == PARTIAL_CONTENT {
            let range = content_range(task.offset, task.length, handler.filesize());
            set_header(ctx, b"Content-Range", range.as_bytes());
        } else {
            set_header(ctx, b"Accept-Ranges", b"bytes");
        }
    }
    // SAFETY: a live context.
    unsafe { swoole_rs_phphttp::http2::unhook(ctx) };
    if entry(zserver, b"request_method").is_some_and(|method| {
        // SAFETY: the request's live string.
        unsafe { string::bytes(method) }.eq_ignore_ascii_case(b"HEAD")
    }) {
        respond(ctx, &[]);
        return true;
    }
    send_ranges(ctx, &handler)
}

/// The single range's `Content-Range`, as upstream's stringstream writes it
/// (`length + offset - 1` in `size_t`, and its trailing CRLF).
fn content_range(offset: i64, length: usize, size: usize) -> String {
    #[expect(clippy::cast_sign_loss, reason = "upstream's size_t sum")]
    let last = (length as u64).wrapping_add(offset as u64).wrapping_sub(1);
    format!("bytes {offset}-{last}/{size}\r\n")
}

/// `http2_server_send_range_file`: the file's ranges, the stream (and so the
/// context) held while a coroutine reads it.
fn send_ranges(ctx: *mut c_void, handler: &StaticHandler) -> bool {
    let _held = of(ctx);
    let trailer = begin_file(ctx, &handler.content_type(), handler.content_length());
    if trailer < 0 {
        return false;
    }
    let end_stream = trailer == 0;
    let tasks = handler.tasks();
    let first = if let [task] = tasks.as_slice() {
        task.length
    } else {
        0
    };
    let mode = sw_rs_http2_file_mode(tasks.len(), first);
    let mut error = false;
    if mode != MODE_EMPTY {
        let Some(file) = File::open(&handler.filename()) else {
            return false;
        };
        if mode == MODE_MULTI {
            for task in &tasks {
                if !body(ctx, c_text(&task.part_header), false) {
                    error = true;
                    break;
                }
                let mut buf = vec![0; task.length];
                if file.read(task.offset, &mut buf, false).is_none() {
                    return false;
                }
                if !body(ctx, &buf, false) {
                    error = true;
                    break;
                }
            }
            if !error && !body(ctx, &handler.end_part(), end_stream) {
                error = true;
            }
        } else if let (MODE_SINGLE, [task]) = (mode, tasks.as_slice()) {
            let mut buf = vec![0; task.length];
            let pooled = swoole_rs_phpcoroutines::in_coroutine();
            let Some(read) = file.read(task.offset, &mut buf, pooled) else {
                return false;
            };
            buf.truncate(read);
            if !body(ctx, &buf, end_stream) {
                error = true;
            }
        }
    }
    finish_file(ctx, trailer != 0, error);
    true
}

/// `swoole_http2_server_send_file`: `Response::sendfile` over HTTP/2, the
/// stream held while a coroutine reads the file.
/// # Safety
/// `ctx` is a live HTTP/2 stream context and `file` a live string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp2_server_send_file(
    ctx: *mut c_void,
    file: *mut zend_string,
    offset: i64,
    length: usize,
) -> bool {
    let Some((session, stream)) = of(ctx) else {
        return false;
    };
    set(ctx, Field::AcceptCompression, 0);
    // SAFETY: a live string.
    let path = unsafe { string::bytes(file) };
    let Some(content) = content(path) else {
        return false;
    };
    // Reading the file may suspend while the peer resets the stream or closes the connection.
    if !current(&session, &stream) {
        return false;
    }
    send_content(ctx, path, &content, offset, length)
}

/// The file's whole content (`System::read_file` in a coroutine), `None` if unreadable.
fn content(path: &[u8]) -> Option<Vec<u8>> {
    let path = CString::new(c_text(path)).ok()?;
    if swoole_rs_phpcoroutines::in_coroutine() {
        return swoole_rs_hooks::file_io::read_file(&path, false);
    }
    let mut content = Vec::new();
    let file = File::open(path.as_bytes())?;
    let mut buf = [0u8; 8192];
    loop {
        // SAFETY: the open descriptor and a writable buffer.
        let n = unsafe { libc::read(file.0, buf.as_mut_ptr().cast(), buf.len()) };
        let Ok(n) = usize::try_from(n) else { break };
        if n == 0 {
            break;
        }
        content.extend_from_slice(buf.get(..n).unwrap_or(&buf));
    }
    Some(content)
}

fn send_content(ctx: *mut c_void, path: &[u8], content: &[u8], offset: i64, length: usize) -> bool {
    let available = sw_rs_http2_file_length(length, content.len());
    // SAFETY: the table's type or the fallback, copied before PHP can change it.
    let content_type = unsafe { std::ffi::CStr::from_ptr(swoole_rs_framing::mime::get(path)) };
    let trailer = begin_file(ctx, content_type.to_bytes(), available);
    if trailer < 0 {
        return false;
    }
    let mut error = false;
    if available > 0 {
        let wanted = if length == 0 { available } else { length };
        let start = usize::try_from(offset)
            .unwrap_or_default()
            .min(content.len());
        let end = start.saturating_add(wanted).min(content.len());
        if !body(
            ctx,
            content.get(start..end).unwrap_or_default(),
            trailer == 0,
        ) {
            error = true;
        }
    }
    finish_file(ctx, trailer != 0, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_is_written_as_upstreams_stringstream_writes_it() {
        assert_eq!(content_range(0, 100, 1000), "bytes 0-99/1000\r\n");
        assert_eq!(content_range(900, 100, 1000), "bytes 900-999/1000\r\n");
        assert_eq!(
            content_range(0, 0, 1),
            format!("bytes 0-{}/1\r\n", u64::MAX)
        );
    }
}
