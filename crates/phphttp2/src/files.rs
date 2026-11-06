//! The HTTP/2 server's file responses: the static handler's (upstream's
//! `http2_server_is_static_file` and `http2_server_send_range_file`) and
//! `Response::sendfile`'s (`http2_server_send_file`). The `StaticHandler`
//! and the file reads (`swoole::File`, `coroutine::async`,
//! `System::read_file`) stay C++, reached through `swr_h2s_static_*` and
//! `swr_h2s_file_*` (`ext-src/swoole_http2_server.cc`).
use crate::server::{Value, begin_file, finish_file, of, respond, send_body, set, zval_of};
use libc::{c_char, c_int, c_void};
use std::ptr;
use swoole_rs_http2::{sw_rs_http2_file_length, sw_rs_http2_file_mode};
use swoole_rs_phphttp::http2::Field;
use swoole_rs_zend::{
    bailout, string,
    sys::{IS_STRING, swr_zend_string_release, zend_hash_str_find, zend_string, zval},
    type_of, value,
};

/// `SWR_H2S_STATIC_*`: what `swr_h2s_static` runs or reads.
const SERVE: c_int = 0;
const STATUS: c_int = 1;
const INDEX: c_int = 2;
const AUTOINDEX: c_int = 3;
const FILESIZE: c_int = 4;
const CONTENT_LENGTH: c_int = 5;
const TASKS: c_int = 6;
/// `SWR_H2S_TEXT_*`: what `swr_h2s_static_text` makes.
const INDEX_PAGE: c_int = 0;
const LAST_MODIFIED: c_int = 1;
const CONTENT_TYPE: c_int = 2;
const FILENAME: c_int = 3;
const END_PART: c_int = 4;
const PART_HEADER: c_int = 5;
const PAGE_404: c_int = 6;
/// `sw_rs_http2_file_mode`'s modes.
const MODE_EMPTY: u32 = 0;
const MODE_MULTI: u32 = 1;
const MODE_SINGLE: u32 = 2;
/// `SW_HTTP_NOT_FOUND`, `SW_HTTP_NOT_MODIFIED` and `SW_HTTP_PARTIAL_CONTENT`.
const NOT_FOUND: i64 = 404;
const NOT_MODIFIED: u32 = 304;
const PARTIAL_CONTENT: i64 = 206;

unsafe extern "C" {
    fn swr_h2s_static_new(server: *mut c_void, url: *const c_char, length: usize) -> *mut c_void;
    fn swr_h2s_static_free(handler: *mut c_void);
    fn swr_h2s_static(handler: *mut c_void, op: c_int) -> i64;
    fn swr_h2s_static_text(handler: *mut c_void, which: c_int, task: usize) -> *mut zend_string;
    fn swr_h2s_static_modified(handler: *mut c_void, date: *const c_char, length: usize) -> bool;
    fn swr_h2s_static_range(handler: *mut c_void, range: *const c_char, if_range: *const c_char);
    fn swr_h2s_static_task(handler: *mut c_void, task: usize, offset: *mut i64, length: *mut usize);
    fn swr_h2s_file_open(path: *const c_char) -> *mut c_void;
    fn swr_h2s_file_read(
        file: *mut c_void,
        offset: i64,
        buf: *mut c_char,
        length: usize,
        r#async: bool,
    ) -> isize;
    fn swr_h2s_file_close(file: *mut c_void);
    fn swr_h2s_file_content(
        path: *const c_char,
        length: *mut usize,
        owner: *mut *mut c_void,
    ) -> *const c_char;
    fn swr_h2s_file_content_free(owner: *mut c_void);
    fn swr_h2s_in_coroutine() -> bool;
}

/// A string `swr_h2s_static_text` made, released on drop.
struct Owned(*mut zend_string);

impl Owned {
    fn bytes(&self) -> &[u8] {
        // SAFETY: a live string this owns.
        unsafe { string::bytes(self.0) }
    }

    fn as_ptr(&self) -> *const c_char {
        // SAFETY: as above; Zend strings are NUL-terminated.
        unsafe { string::bytes(self.0) }.as_ptr().cast()
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the reference the host returned.
        unsafe { swr_zend_string_release(self.0) };
    }
}

/// A `swoole::http_server::StaticHandler` for the request's URI.
struct Handler(*mut c_void);

impl Handler {
    fn new(server: *mut c_void, url: &[u8]) -> Self {
        // SAFETY: the live server and the URI's bytes, which C++ copies.
        Self(unsafe { swr_h2s_static_new(server, url.as_ptr().cast(), url.len()) })
    }

    fn run(&self, op: c_int) -> i64 {
        // SAFETY: the live handler.
        unsafe { swr_h2s_static(self.0, op) }
    }

    fn yes(&self, op: c_int) -> bool {
        self.run(op) != 0
    }

    fn count(&self, op: c_int) -> usize {
        usize::try_from(self.run(op)).unwrap_or_default()
    }

    fn text(&self, which: c_int, task: usize) -> Owned {
        // SAFETY: the live handler; the result is a new string.
        Owned(unsafe { swr_h2s_static_text(self.0, which, task) })
    }

    fn modified(&self, date: &[u8]) -> bool {
        // SAFETY: the live handler and the date's bytes.
        unsafe { swr_h2s_static_modified(self.0, date.as_ptr().cast(), date.len()) }
    }

    fn task(&self, task: usize) -> (i64, usize) {
        let (mut offset, mut length) = (0, 0);
        // SAFETY: the live handler, one of its tasks and local outputs.
        unsafe { swr_h2s_static_task(self.0, task, &raw mut offset, &raw mut length) };
        (offset, length)
    }
}

impl Drop for Handler {
    fn drop(&mut self) {
        // SAFETY: the handler `new` made, freed once.
        unsafe { swr_h2s_static_free(self.0) };
    }
}

/// A file opened read-only, closed on drop.
struct File(*mut c_void);

impl File {
    fn open(path: &Owned) -> Option<Self> {
        // SAFETY: a NUL-terminated path.
        let file = unsafe { swr_h2s_file_open(path.as_ptr()) };
        (!file.is_null()).then_some(Self(file))
    }

    /// One read of up to `buf.len()` bytes at `offset`, on the thread pool
    /// when `pooled`; `None` if it failed.
    fn read(&self, offset: i64, buf: &mut [u8], pooled: bool) -> Option<usize> {
        // SAFETY: the open file and a writable buffer that outlives the read.
        let n = unsafe {
            swr_h2s_file_read(self.0, offset, buf.as_mut_ptr().cast(), buf.len(), pooled)
        };
        usize::try_from(n).ok()
    }
}

impl Drop for File {
    fn drop(&mut self) {
        // SAFETY: the file `open` made, closed once.
        unsafe { swr_h2s_file_close(self.0) };
    }
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
    let handler = Handler::new(server, unsafe { string::bytes(uri) });
    if !handler.yes(SERVE) {
        return false;
    }
    if handler.run(STATUS) == NOT_FOUND {
        set(ctx, Field::Status, 404);
        respond(ctx, handler.text(PAGE_404, 0).bytes());
        return true;
    }
    // if http_index_files is enabled, need to search the index file first.
    if !handler.yes(INDEX) {
        return false;
    }
    // no index file in the directory: with http_autoindex, list the directory.
    if handler.yes(AUTOINDEX) {
        respond(ctx, handler.text(INDEX_PAGE, 0).bytes());
        return true;
    }
    set_header(
        ctx,
        b"Last-Modified",
        handler.text(LAST_MODIFIED, 0).bytes(),
    );
    let zheader = zval_of(ctx, Value::Header);
    if let Some(since) = entry(zheader, b"if-modified-since") {
        // SAFETY: the header's live string.
        let since = unsafe { string::bytes(since) };
        if !since.is_empty() && handler.modified(since) {
            set(ctx, Field::Status, NOT_MODIFIED);
            return true;
        }
    }
    let text = |key: &[u8]| {
        entry(zheader, key).map_or(ptr::null(), |found| {
            // SAFETY: a live, NUL-terminated header string.
            unsafe { string::bytes(found) }.as_ptr().cast()
        })
    };
    // SAFETY: the live handler and NUL-terminated header values (or null).
    unsafe { swr_h2s_static_range(handler.0, text(b"range"), text(b"if-range")) };
    let status = handler.run(STATUS);
    set(
        ctx,
        Field::Status,
        u32::try_from(status).unwrap_or_default(),
    );
    if handler.count(TASKS) == 1 {
        if status == PARTIAL_CONTENT {
            let (offset, length) = handler.task(0);
            let range = content_range(offset, length, handler.count(FILESIZE));
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
fn send_ranges(ctx: *mut c_void, handler: &Handler) -> bool {
    let _held = of(ctx);
    let content_type = handler.text(CONTENT_TYPE, 0);
    let trailer = begin_file(ctx, content_type.bytes(), handler.count(CONTENT_LENGTH));
    if trailer < 0 {
        return false;
    }
    let end_stream = trailer == 0;
    let tasks = handler.count(TASKS);
    let first = if tasks == 1 { handler.task(0).1 } else { 0 };
    let mode = sw_rs_http2_file_mode(tasks, first);
    let mut error = false;
    if mode != MODE_EMPTY {
        let Some(file) = File::open(&handler.text(FILENAME, 0)) else {
            return false;
        };
        if mode == MODE_MULTI {
            for task in 0..tasks {
                if !body(ctx, handler.text(PART_HEADER, task).bytes(), false) {
                    error = true;
                    break;
                }
                let (offset, length) = handler.task(task);
                let mut buf = vec![0; length];
                if file.read(offset, &mut buf, false).is_none() {
                    return false;
                }
                if !body(ctx, &buf, false) {
                    error = true;
                    break;
                }
            }
            if !error && !body(ctx, handler.text(END_PART, 0).bytes(), end_stream) {
                error = true;
            }
        } else if mode == MODE_SINGLE {
            let (offset, length) = handler.task(0);
            let mut buf = vec![0; length];
            // SAFETY: reads the current coroutine.
            let pooled = unsafe { swr_h2s_in_coroutine() };
            let Some(read) = file.read(offset, &mut buf, pooled) else {
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
    let _held = of(ctx);
    set(ctx, Field::AcceptCompression, 0);
    // SAFETY: a live string.
    let path = unsafe { string::bytes(file) };
    let (mut available, mut owner) = (0, ptr::null_mut());
    // SAFETY: a NUL-terminated path and local outputs.
    let data =
        unsafe { swr_h2s_file_content(path.as_ptr().cast(), &raw mut available, &raw mut owner) };
    if data.is_null() {
        return false;
    }
    let sent = send_content(ctx, path, bytes(data, available), offset, length);
    // SAFETY: the content `swr_h2s_file_content` lent, freed once.
    unsafe { swr_h2s_file_content_free(owner) };
    sent
}

fn bytes<'a>(data: *const c_char, length: usize) -> &'a [u8] {
    if length == 0 {
        return &[];
    }
    // SAFETY: per every caller, `length` readable bytes.
    unsafe { std::slice::from_raw_parts(data.cast(), length) }
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
        if !body(ctx, &content[start..end], trailer == 0) {
            error = true;
        }
    }
    finish_file(ctx, trailer != 0, error);
    true
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
