//! Upstream's `Server::select_static_handler` (`src/server/static_handler.cc`):
//! the HTTP/1 server's static responses, a 404 page, a 304, a directory
//! listing or the file's ranges, sent to the connection.
use crate::static_handler::{Effects, StaticHandler, page_404};
use core::ffi::c_void;
use core::ptr;
use swoole_rs_http::{HttpStaticHeaders, HttpStaticTask};

/// What [`select`] asks of a static handler, in upstream `select_static_handler`'s order.
pub trait Handler {
    fn try_serve(&self) -> bool;
    fn status(&self) -> i32;
    fn date(&self) -> Vec<u8>;
    fn date_last_modified(&self) -> Vec<u8>;
    fn is_modified(&self, date: &[u8]) -> bool;
    fn try_serve_index_file(&self) -> bool;
    fn lists_directory(&self) -> bool;
    fn index_page(&self) -> Vec<u8>;
    /// `parse_range(range.c_str(), if_range.c_str())`: both always present.
    fn parse_range(&self, range: &[u8], if_range: &[u8]);
    fn tasks(&self) -> Vec<HttpStaticTask>;
    fn content_length(&self) -> usize;
    fn mimetype(&self) -> Vec<u8>;
    fn content_type(&self) -> Vec<u8>;
    fn filesize(&self) -> usize;
    fn filename(&self) -> Vec<u8>;
    fn end_part(&self) -> Vec<u8>;
}

impl<E: Effects> Handler for StaticHandler<E> {
    fn try_serve(&self) -> bool {
        Self::try_serve(self)
    }
    fn status(&self) -> i32 {
        Self::status(self)
    }
    fn date(&self) -> Vec<u8> {
        Self::date()
    }
    fn date_last_modified(&self) -> Vec<u8> {
        Self::date_last_modified(self)
    }
    fn is_modified(&self, date: &[u8]) -> bool {
        Self::is_modified(self, date)
    }
    fn try_serve_index_file(&self) -> bool {
        Self::try_serve_index_file(self)
    }
    fn lists_directory(&self) -> bool {
        Self::lists_directory(self)
    }
    fn index_page(&self) -> Vec<u8> {
        Self::index_page(self)
    }
    fn parse_range(&self, range: &[u8], if_range: &[u8]) {
        Self::parse_range(self, Some(range), Some(if_range));
    }
    fn tasks(&self) -> Vec<HttpStaticTask> {
        Self::tasks(self)
    }
    fn content_length(&self) -> usize {
        Self::content_length(self)
    }
    fn mimetype(&self) -> Vec<u8> {
        Self::mimetype(self)
    }
    fn content_type(&self) -> Vec<u8> {
        Self::content_type(self)
    }
    fn filesize(&self) -> usize {
        Self::filesize(self)
    }
    fn filename(&self) -> Vec<u8> {
        Self::filename(self)
    }
    fn end_part(&self) -> Vec<u8> {
        Self::end_part(self)
    }
}

/// What [`select`] asks of the request and its connection. Each fact is read
/// where upstream reads it, so a send that changes the request is seen.
pub trait Exchange {
    /// `request->get_header(name)`, empty when absent.
    fn header(&self, name: &[u8]) -> Vec<u8>;
    fn keep_alive(&self) -> bool;
    /// `SW_HTTP_HEAD == request->method`.
    fn head(&self) -> bool;
    /// `conn->socket->cork()`.
    fn cork(&self);
    /// `send_to_connection` of a `SendData` of event `kind` to the connection.
    fn send(&self, kind: u8, data: &[u8]);
}

/// `SW_SERVER_EVENT_SEND_DATA`, `SW_SERVER_EVENT_SEND_FILE`, `SW_SERVER_EVENT_CLOSE`.
pub const SEND_DATA: u8 = 2;
pub const SEND_FILE: u8 = 3;
pub const CLOSE: u8 = 4;
/// `SW_HTTP_NOT_FOUND`, `SW_HTTP_PARTIAL_CONTENT`.
const NOT_FOUND: i32 = 404;
const PARTIAL_CONTENT: i32 = 206;
/// `SW_HTTP_HEAD`.
const HTTP_HEAD: u8 = 3;
/// `SW_HTTP_SERVER_SOFTWARE`.
const SOFTWARE: &[u8] = b"swoole-http-server";

/// The text up to its first NUL, as a C string reads it.
fn c_text(text: &[u8]) -> &[u8] {
    text.split(|&byte| byte == 0).next().unwrap_or_default()
}

/// The response head, cut as upstream's 1024-byte `sw_snprintf` buffer cuts it.
fn encode(header: &HttpStaticHeaders<'_>) -> Vec<u8> {
    let mut head = swoole_rs_http::static_headers(header).unwrap_or_default();
    head.truncate(1023);
    head
}

#[expect(
    clippy::cast_possible_wrap,
    reason = "upstream prints the size_t length with %ld"
)]
fn signed(length: usize) -> i64 {
    length as i64
}

/// `Server::select_static_handler`: answer an HTTP/1 request from the
/// document root; false when the static handler does not serve it.
pub fn select(handler: &impl Handler, exchange: &impl Exchange) -> bool {
    if !handler.try_serve() {
        return false;
    }
    let mut header = HttpStaticHeaders {
        status: &[],
        date: &[],
        modified: &[],
        content_type: &[],
        software: SOFTWARE,
        page: &[],
        content_length: 0,
        range_offset: 0,
        range_length: 0,
        filesize: 0,
        variant: 0,
        keep_alive: false,
        range_kind: 0,
    };
    if handler.status() == NOT_FOUND {
        let status = swoole_rs_http::status_message(NOT_FOUND);
        let page = page_404();
        header.status = &status;
        header.page = &page;
        header.content_length = signed(page.len());
        exchange.send(SEND_DATA, &encode(&header));
        return true;
    }
    let date = handler.date();
    let modified = handler.date_last_modified();
    header.date = &date;
    header.modified = &modified;
    let since = exchange.header(b"If-Modified-Since");
    if !since.is_empty() && handler.is_modified(&since) {
        header.variant = 1;
        header.keep_alive = exchange.keep_alive();
        exchange.send(SEND_DATA, &encode(&header));
        return true;
    }
    // With http_index_files, the directory's index file becomes the file.
    if !handler.try_serve_index_file() {
        return false;
    }
    // No index file in the directory: with http_autoindex, list the directory.
    if handler.lists_directory() {
        let body = handler.index_page();
        header.variant = 2;
        header.keep_alive = exchange.keep_alive();
        header.content_length = signed(body.len());
        exchange.send(SEND_DATA, &encode(&header));
        exchange.send(SEND_DATA, &body);
        return true;
    }
    // Upstream reads both as one call's arguments: GCC (Linux) evaluates them
    // right to left, clang left to right.
    let (range, if_range) = if cfg!(target_os = "linux") {
        let if_range = exchange.header(b"If-Range");
        (exchange.header(b"Range"), if_range)
    } else {
        (exchange.header(b"Range"), exchange.header(b"If-Range"))
    };
    handler.parse_range(c_text(&range), c_text(&if_range));
    let tasks = handler.tasks();
    let status = handler.status();
    let message = swoole_rs_http::status_message(status);
    header.variant = 3;
    header.status = &message;
    header.keep_alive = exchange.keep_alive();
    let head = exchange.head();
    header.content_length = if head {
        0
    } else {
        signed(handler.content_length())
    };
    let content_type = if head {
        handler.mimetype()
    } else {
        handler.content_type()
    };
    header.content_type = &content_type;
    if let [task] = tasks.as_slice() {
        if status == PARTIAL_CONTENT {
            header.range_kind = 1;
            header.range_offset = task.offset;
            header.range_length = task.length;
            header.filesize = handler.filesize();
        } else {
            header.range_kind = 2;
        }
    }
    let head_bytes = encode(&header);
    // tcp_nopush for the header and body.
    exchange.cork();
    exchange.send(SEND_DATA, &head_bytes);
    if !exchange.head() && !tasks.is_empty() {
        let filename = handler.filename();
        let mut file = SendfileTask::new(c_text(&filename));
        if tasks.len() > 1 {
            for task in &tasks {
                exchange.send(SEND_DATA, c_text(&task.part_header));
                exchange.send(SEND_FILE, file.at(task));
            }
            exchange.send(SEND_DATA, &handler.end_part());
        } else if let [task] = tasks.as_slice()
            && task.length > 0
        {
            exchange.send(SEND_FILE, file.at(task));
        }
    }
    // Close the connection if keepalive is not used.
    if !exchange.keep_alive() {
        exchange.send(CLOSE, &[]);
    }
    true
}

/// A `network::SendfileTask`: offset, length and the NUL-terminated name.
struct SendfileTask(Vec<u8>);

impl SendfileTask {
    const HEAD: usize = 16;

    fn new(filename: &[u8]) -> Self {
        let mut bytes = Vec::with_capacity(Self::HEAD + filename.len() + 1);
        bytes.extend_from_slice(&[0; Self::HEAD]);
        bytes.extend_from_slice(filename);
        bytes.push(0);
        Self(bytes)
    }

    /// The task's bytes for one range.
    fn at(&mut self, task: &HttpStaticTask) -> &[u8] {
        if let Some((offset, rest)) = self.0.split_first_chunk_mut::<8>()
            && let Some((length, _)) = rest.split_first_chunk_mut::<8>()
        {
            *offset = task.offset.to_ne_bytes();
            *length = task.length.to_ne_bytes();
        }
        &self.0
    }
}

/// The live request, connection and server of an HTTP/1 static response.
struct Wire {
    server: *mut c_void,
    request: *const swoole_rs_http_ffi::HttpRequest,
    conn: *const crate::ConnectionRecord,
}

impl Wire {
    fn request(&self) -> &swoole_rs_http_ffi::HttpRequest {
        // SAFETY: the connection's live request, per `swr_v1_server_static_select`.
        unsafe { &*self.request }
    }

    fn connection(&self) -> &crate::ConnectionRecord {
        // SAFETY: the live connection, per `swr_v1_server_static_select`.
        unsafe { &*self.conn }
    }

    /// `length` bytes of the request's buffer at `offset`.
    fn bytes(&self, offset: usize, length: usize) -> Vec<u8> {
        // SAFETY: the request's live buffer.
        let start = unsafe { (*self.request().buffer).str };
        // SAFETY: the parser's spans lie in the buffer.
        let start = unsafe { start.add(offset) };
        // SAFETY: as above, `length` bytes of it.
        unsafe { core::slice::from_raw_parts(start, length) }.to_vec()
    }
}

impl Exchange for Wire {
    fn header(&self, name: &[u8]) -> Vec<u8> {
        self.request()
            .get_header(name)
            .map_or_else(Vec::new, |span| self.bytes(span.offset, span.length))
    }
    fn keep_alive(&self) -> bool {
        self.request().keep_alive
    }
    fn head(&self) -> bool {
        self.request().method == HTTP_HEAD
    }
    fn cork(&self) {
        // SAFETY: the connection's live socket.
        swoole_rs_reactor::socket_upstream::socket_cork(unsafe {
            &mut *self.connection().socket.cast()
        });
    }
    fn send(&self, kind: u8, data: &[u8]) {
        let send = crate::base_factory::SendData {
            info: swoole_rs_corebase::DataHead {
                fd: self.connection().session_id,
                len: u32::try_from(data.len()).unwrap_or(u32::MAX),
                kind,
                ..Default::default()
            },
            data: if data.is_empty() {
                ptr::null()
            } else {
                data.as_ptr().cast()
            },
        };
        // SAFETY: the live server and a `SendData` it reads during the call.
        unsafe { crate::server_methods::send_to_connection(self.server, (&raw const send).cast()) };
    }
}

/// `Server::select_static_handler(request, conn)` for the ports read path.
///
/// # Safety
/// `server` is the live server and `request` the live request of `conn`, one
/// of its connections.
pub unsafe fn swr_v1_server_static_select(
    server: *mut c_void,
    request: *const swoole_rs_http_ffi::HttpRequest,
    conn: *const crate::ConnectionRecord,
) -> bool {
    let wire = Wire {
        server,
        request,
        conn,
    };
    let url = wire.bytes(
        wire.request().url_offset as usize,
        wire.request().url_length as usize,
    );
    // SAFETY: the live server, which outlives the handler.
    let Some(handler) = (unsafe { StaticHandler::new(server, &url) }) else {
        return false;
    };
    select(&handler, &wire)
}

/// The cases of the retired `tests/modules/http/static_gate.py`: [`select`] over
/// a scripted handler and request against the pinned `select_static_handler`
/// over the same script (`swoole_rs_oracle::http_static_select`), comparing the
/// result, every traced call, every send's bytes and the request's final state.
#[cfg(test)]
mod tests {
    use super::{CLOSE, Exchange, Handler, SEND_FILE, select};
    use core::cell::{Cell, RefCell};
    use std::ffi::{c_char, c_void};
    use swoole_rs_http::HttpStaticTask;
    use swoole_rs_oracle::http_static_select::{Scenario, Task, Text, swr_oracle_static_select};

    const OK: i32 = 200;
    const PARTIAL: i32 = 206;
    const NOT_FOUND: i32 = 404;
    const UNSATISFIABLE: i32 = 416;

    #[derive(Clone)]
    struct Case {
        serve: bool,
        modified: bool,
        index_ok: bool,
        index: bool,
        autoindex: bool,
        directory: bool,
        status: i32,
        date: Vec<u8>,
        last_modified: Vec<u8>,
        mime: Vec<u8>,
        content_type: Vec<u8>,
        body: Vec<u8>,
        filename: Vec<u8>,
        end_part: Vec<u8>,
        filesize: usize,
        content_length: usize,
        tasks: Vec<HttpStaticTask>,
        throw_on_send: bool,
        send_result: i32,
        toggle_keep_on_send: u32,
        toggle_method_on_send: u32,
    }

    impl Default for Case {
        fn default() -> Self {
            Self {
                serve: true,
                modified: false,
                index_ok: true,
                index: false,
                autoindex: false,
                directory: false,
                status: OK,
                date: b"Mon, 01 Jan 2024 00:00:00 GMT".to_vec(),
                last_modified: b"Sun, 31 Dec 2023 00:00:00 GMT".to_vec(),
                mime: b"text/plain".to_vec(),
                content_type: b"text/plain".to_vec(),
                body: b"directory body".to_vec(),
                filename: b"/static/file.txt".to_vec(),
                end_part: b"--boundary--\r\n".to_vec(),
                filesize: 12345,
                content_length: 12345,
                tasks: vec![task(0, 12345, b"")],
                throw_on_send: false,
                send_result: 0,
                toggle_keep_on_send: 0,
                toggle_method_on_send: 0,
            }
        }
    }

    fn task(offset: i64, length: usize, header: &[u8]) -> HttpStaticTask {
        let mut task = HttpStaticTask {
            offset,
            length,
            ..HttpStaticTask::default()
        };
        task.part_header[..header.len()].copy_from_slice(header);
        task
    }

    /// The scripted handler, request and connection, tracing as the oracle's facade does.
    struct Script<'a> {
        case: &'a Case,
        modified_header: Vec<u8>,
        trace: RefCell<Vec<Vec<u8>>>,
        request_trace: RefCell<Vec<Vec<u8>>>,
        events: RefCell<Vec<Vec<u8>>>,
        keep_alive: Cell<bool>,
        head: Cell<bool>,
        sends: Cell<u32>,
    }

    /// A send's `throw`.
    struct Thrown;

    impl Script<'_> {
        fn note(&self, entry: &str) {
            self.trace.borrow_mut().push(entry.as_bytes().to_vec());
        }
    }

    impl Handler for Script<'_> {
        fn try_serve(&self) -> bool {
            self.note("serve");
            self.case.serve
        }
        fn status(&self) -> i32 {
            self.case.status
        }
        fn date(&self) -> Vec<u8> {
            self.note("date");
            self.case.date.clone()
        }
        fn date_last_modified(&self) -> Vec<u8> {
            self.note("modified-date");
            self.case.last_modified.clone()
        }
        fn is_modified(&self, _: &[u8]) -> bool {
            self.note("is-modified");
            self.case.modified
        }
        fn try_serve_index_file(&self) -> bool {
            self.note("index");
            self.case.index_ok
        }
        fn lists_directory(&self) -> bool {
            !self.case.index && self.case.autoindex && self.case.directory
        }
        fn index_page(&self) -> Vec<u8> {
            self.note("index-page");
            self.case.body.clone()
        }
        fn parse_range(&self, _: &[u8], _: &[u8]) {
            self.note("range");
        }
        fn tasks(&self) -> Vec<HttpStaticTask> {
            self.case.tasks.clone()
        }
        fn content_length(&self) -> usize {
            self.case.content_length
        }
        fn mimetype(&self) -> Vec<u8> {
            self.note("mime");
            self.case.mime.clone()
        }
        fn content_type(&self) -> Vec<u8> {
            self.note("content-type");
            self.case.content_type.clone()
        }
        fn filesize(&self) -> usize {
            self.case.filesize
        }
        fn filename(&self) -> Vec<u8> {
            self.case.filename.clone()
        }
        fn end_part(&self) -> Vec<u8> {
            self.case.end_part.clone()
        }
    }

    impl Exchange for Script<'_> {
        fn header(&self, name: &[u8]) -> Vec<u8> {
            self.request_trace.borrow_mut().push(name.to_vec());
            if name == b"If-Modified-Since" {
                self.modified_header.clone()
            } else {
                Vec::new()
            }
        }
        fn keep_alive(&self) -> bool {
            self.keep_alive.get()
        }
        fn head(&self) -> bool {
            self.head.get()
        }
        fn cork(&self) {
            self.note("cork");
        }
        fn send(&self, kind: u8, data: &[u8]) {
            self.note("send");
            if self.case.throw_on_send {
                std::panic::resume_unwind(Box::new(Thrown));
            }
            self.sends.set(self.sends.get() + 1);
            if self.sends.get() == self.case.toggle_keep_on_send {
                self.keep_alive.set(!self.keep_alive.get());
            }
            if self.sends.get() == self.case.toggle_method_on_send {
                self.head.set(!self.head.get());
            }
            let mut event = format!("E:73:{kind}:").into_bytes();
            if kind == SEND_FILE {
                let offset = i64::from_ne_bytes(data[..8].try_into().unwrap());
                let length = usize::from_ne_bytes(data[8..16].try_into().unwrap());
                let name = data[16..].split(|&b| b == 0).next().unwrap();
                event.extend_from_slice(format!("{offset}:{length}:").as_bytes());
                event.extend_from_slice(name);
                event.extend_from_slice(format!(":{}", data.len()).as_bytes());
            } else if kind != CLOSE {
                event.extend_from_slice(data);
            }
            self.events.borrow_mut().push(event);
        }
    }

    fn errno() -> i32 {
        std::io::Error::last_os_error().raw_os_error().unwrap()
    }

    fn set_errno(value: i32) {
        #[cfg(target_os = "linux")]
        unsafe {
            *libc::__errno_location() = value;
        };
        #[cfg(not(target_os = "linux"))]
        unsafe {
            *libc::__error() = value;
        };
    }

    /// [`select`]'s result (2 for a throw) and lines, as the oracle reports them.
    fn rust(case: &Case, head: bool, keep_alive: bool, modified: &[u8]) -> (i32, Vec<Vec<u8>>) {
        let script = Script {
            case,
            modified_header: modified.to_vec(),
            trace: RefCell::default(),
            request_trace: RefCell::default(),
            events: RefCell::default(),
            keep_alive: Cell::new(keep_alive),
            head: Cell::new(head),
            sends: Cell::new(0),
        };
        set_errno(libc::EINTR);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            i32::from(select(&script, &script))
        }))
        .unwrap_or(2);
        let error = errno();
        let mut lines: Vec<Vec<u8>> = Vec::new();
        let prefixed = |prefix: &[u8], entry: &Vec<u8>| [prefix, entry].concat();
        lines.extend(script.trace.borrow().iter().map(|e| prefixed(b"T:", e)));
        lines.extend(
            script
                .request_trace
                .borrow()
                .iter()
                .map(|e| prefixed(b"R:", e)),
        );
        lines.extend(script.events.borrow().iter().cloned());
        lines.push(format!("K:{}", u8::from(script.keep_alive.get())).into_bytes());
        lines.push(format!("M:{}", if script.head.get() { 3 } else { 2 }).into_bytes());
        lines.push(format!("errno:{error}").into_bytes());
        (result, lines)
    }

    unsafe extern "C" fn line(out: *mut c_void, line: *const c_char, length: usize) {
        let lines = unsafe { &mut *out.cast::<Vec<Vec<u8>>>() };
        lines.push(unsafe { core::slice::from_raw_parts(line.cast::<u8>(), length) }.to_vec());
    }

    fn upstream(case: &Case, head: bool, keep_alive: bool, modified: &[u8]) -> (i32, Vec<Vec<u8>>) {
        let tasks: Vec<Task> = case
            .tasks
            .iter()
            .map(|t| Task {
                offset: t.offset,
                length: t.length,
                part_header: t.part_header,
            })
            .collect();
        let scenario = Scenario {
            serve: case.serve.into(),
            modified: case.modified.into(),
            index_ok: case.index_ok.into(),
            index: case.index.into(),
            autoindex: case.autoindex.into(),
            directory: case.directory.into(),
            status: case.status,
            date: Text::of(&case.date),
            last_modified: Text::of(&case.last_modified),
            mime: Text::of(&case.mime),
            content_type: Text::of(&case.content_type),
            body: Text::of(&case.body),
            filename: Text::of(&case.filename),
            end_part: Text::of(&case.end_part),
            modified_header: Text::of(modified),
            filesize: case.filesize,
            content_length: case.content_length,
            tasks: tasks.as_ptr(),
            task_count: tasks.len(),
            throw_on_send: case.throw_on_send.into(),
            send_result: case.send_result,
            toggle_keep_on_send: case.toggle_keep_on_send,
            toggle_method_on_send: case.toggle_method_on_send,
            head: head.into(),
            keep_alive: keep_alive.into(),
        };
        let mut lines: Vec<Vec<u8>> = Vec::new();
        let result =
            unsafe { swr_oracle_static_select(&raw const scenario, line, (&raw mut lines).cast()) };
        (result, lines)
    }

    fn compare(case: &Case, head: bool, keep_alive: bool, modified: bool) -> usize {
        let header: &[u8] = if modified { b"if modified" } else { b"" };
        let expected = upstream(case, head, keep_alive, header);
        let actual = rust(case, head, keep_alive, header);
        assert!(
            expected == actual,
            "status {} head {head} keep {keep_alive} modified {modified}\nupstream {:?}\nrust {:?}",
            case.status,
            expected
                .1
                .iter()
                .map(|l| String::from_utf8_lossy(l))
                .collect::<Vec<_>>(),
            actual
                .1
                .iter()
                .map(|l| String::from_utf8_lossy(l))
                .collect::<Vec<_>>(),
        );
        1
    }

    fn two_ranges(case: &mut Case, first: &[u8], second: &[u8]) {
        case.tasks = vec![task(0, 0, first), task(2, 17, second)];
    }

    #[test]
    fn former_static_gate_response_branches() {
        let mut cases = 0;
        for status in [OK, NOT_FOUND, PARTIAL, UNSATISFIABLE] {
            for mode in 0..8 {
                for head in [false, true] {
                    for keep in [false, true] {
                        for modified in [false, true] {
                            let mut case = Case {
                                status,
                                serve: mode != 1,
                                modified: mode == 2,
                                index_ok: mode != 3,
                                autoindex: mode == 4,
                                directory: mode == 4,
                                index: mode == 5,
                                throw_on_send: mode == 6,
                                ..Case::default()
                            };
                            if mode == 7 {
                                case.tasks.clear();
                            }
                            cases += compare(&case, head, keep, modified);
                            if !case.throw_on_send {
                                two_ranges(
                                    &mut case,
                                    b"--boundary\r\nfirst\r\n\r\n",
                                    b"--boundary\r\nsecond\r\n\r\n",
                                );
                                case.content_type =
                                    b"multipart/byteranges; boundary=boundary".to_vec();
                                cases += compare(&case, head, keep, modified);
                                case.date = vec![b'd'; 1500];
                                case.last_modified = b"date\0ignored".to_vec();
                                case.mime = vec![b'm'; 300];
                                cases += compare(&case, head, keep, modified);
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(cases, 4 * 8 * 8 + 4 * 7 * 8 * 2);
    }

    #[test]
    fn former_static_gate_range_extremes() {
        for length in [0, 1, usize::MAX] {
            for offset in [0, 17, i64::MAX] {
                let case = Case {
                    status: PARTIAL,
                    tasks: vec![task(offset, length, b"")],
                    content_length: 0,
                    filesize: usize::MAX,
                    ..Case::default()
                };
                compare(&case, false, true, false);
                compare(&case, true, false, false);
            }
        }
    }

    /// Sends may fail or change the request: keep-alive is read again after the
    /// body and HEAD again after the header, as upstream reads them.
    #[test]
    fn former_static_gate_sends_change_the_request() {
        for toggle_keep_on_send in 0..7 {
            for toggle_method_on_send in 0..4 {
                for send_result in [0, -1] {
                    for head in [false, true] {
                        for keep in [false, true] {
                            let mut case = Case {
                                send_result,
                                toggle_keep_on_send,
                                toggle_method_on_send,
                                ..Case::default()
                            };
                            compare(&case, head, keep, false);
                            two_ranges(&mut case, b"first", b"second");
                            compare(&case, head, keep, false);
                            case.tasks = vec![task(0, 0, b"")];
                            compare(&case, head, keep, false);
                            case.tasks.clear();
                            compare(&case, head, keep, false);
                        }
                    }
                }
            }
        }
    }
}
