//! Upstream's `Server::select_static_handler` (`src/server/static_handler.cc`):
//! the HTTP/1 server's static responses, a 404 page, a 304, a directory
//! listing or the file's ranges, sent to the connection.
use crate::static_handler::{Effects, StaticHandler, page_404};
use swoole_rs_corebase::{CCast, ServerEvent};
use swoole_rs_http::status::{NOT_FOUND, PARTIAL_CONTENT};
use swoole_rs_http::{HttpStaticHeaders, HttpStaticTask, PreconditionStatus};

/// What [`select`] asks of a static handler, in upstream `select_static_handler`'s order.
pub trait Handler {
    type File;
    fn open_file(&mut self) -> bool;
    fn file(&self) -> Option<&Self::File>;
    fn try_serve(&mut self) -> bool;
    fn status(&self) -> i32;
    fn date(&self) -> Vec<u8>;
    fn date_last_modified(&self) -> Vec<u8>;
    fn etag(&self) -> String;
    fn directory_redirect(&self) -> Option<Vec<u8>>;
    fn preconditions(&self, headers: [&[u8]; 4]) -> PreconditionStatus;
    fn try_serve_index_file(&mut self) -> bool;
    fn lists_directory(&self) -> bool;
    fn index_page(&mut self) -> Vec<u8>;
    /// `parse_range(range.c_str(), if_range.c_str())`: both always present.
    fn parse_range(&mut self, range: &[u8], if_range: &[u8]);
    fn tasks(&self) -> Vec<HttpStaticTask>;
    fn content_length(&self) -> usize;
    fn content_type(&mut self) -> Vec<u8>;
    fn filesize(&self) -> usize;
    fn end_part(&self) -> Vec<u8>;
}

impl<E: Effects> Handler for StaticHandler<'_, E> {
    fn try_serve(&mut self) -> bool {
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
    fn etag(&self) -> String {
        Self::etag(self)
    }
    fn directory_redirect(&self) -> Option<Vec<u8>> {
        Self::directory_redirect(self)
    }
    fn preconditions(
        &self,
        [if_match, unmodified, none_match, modified]: [&[u8]; 4],
    ) -> PreconditionStatus {
        Self::preconditions(self, if_match, unmodified, none_match, modified)
    }
    fn try_serve_index_file(&mut self) -> bool {
        Self::try_serve_index_file(self)
    }
    fn lists_directory(&self) -> bool {
        Self::lists_directory(self)
    }
    fn index_page(&mut self) -> Vec<u8> {
        Self::index_page(self)
    }
    fn parse_range(&mut self, range: &[u8], if_range: &[u8]) {
        Self::parse_range(self, Some(range), Some(if_range));
    }
    fn tasks(&self) -> Vec<HttpStaticTask> {
        Self::tasks(self)
    }
    fn content_length(&self) -> usize {
        Self::content_length(self)
    }
    fn content_type(&mut self) -> Vec<u8> {
        Self::content_type(self)
    }
    fn filesize(&self) -> usize {
        Self::filesize(self)
    }
    type File = std::sync::Arc<swoole_rs_corebase::ossupport::FileRecord>;
    fn open_file(&mut self) -> bool {
        Self::open_file(self)
    }
    fn file(&self) -> Option<&Self::File> {
        Self::file(self)
    }
    fn end_part(&self) -> Vec<u8> {
        Self::end_part(self)
    }
}

/// What [`select`] asks of the request and its connection. Each fact is read
/// where upstream reads it, so a send that changes the request is seen.
pub trait Exchange<F> {
    fn send_range(&self, file: &F, part: &HttpStaticTask) -> bool;
    /// `request->get_header(name)`, empty when absent.
    fn header(&self, name: &[u8]) -> Vec<u8>;
    fn keep_alive(&self) -> bool;
    /// `SW_HTTP_HEAD == request->method`.
    fn head(&self) -> bool;
    fn send(&self, kind: ServerEvent, data: &[u8], reset: bool) -> bool;
}

/// The text up to its first NUL, as a C string reads it.
fn c_text(text: &[u8]) -> &[u8] {
    text.split(|&byte| byte == 0).next().unwrap_or_default()
}

/// Answer an HTTP/1 static request. A failed write stops further body sends.
pub fn select<H: Handler>(handler: &mut H, exchange: &impl Exchange<H::File>) -> bool {
    if !handler.try_serve() {
        return false;
    }
    let status = handler.status();
    let redirect = (status == 200)
        .then(|| handler.directory_redirect())
        .flatten();
    if status == 200 && (!handler.try_serve_index_file() || !handler.open_file()) {
        return false;
    }
    let head = exchange.head();
    let ok = write_response(handler, exchange, status, redirect, head);
    if !ok || !exchange.keep_alive() {
        exchange.send(ServerEvent::Close, &[], !ok);
    }
    true
}

fn write_response<H: Handler>(
    handler: &mut H,
    exchange: &impl Exchange<H::File>,
    status: i32,
    redirect: Option<Vec<u8>>,
    head: bool,
) -> bool {
    let send = |data: &[u8]| data.is_empty() || exchange.send(ServerEvent::SendData, data, false);
    let header = |handler: &H, status, length, mime: &[u8], extra: &[u8]| {
        let date = handler.date();
        let modified = if matches!(status, 400 | 404) {
            Vec::new()
        } else {
            handler.date_last_modified()
        };
        let etag = if matches!(status, 200 | 206 | 304 | 412 | 416) {
            handler.etag()
        } else {
            String::new()
        };
        swoole_rs_http::static_headers(&HttpStaticHeaders {
            status,
            date: &date,
            modified: &modified,
            etag: etag.as_bytes(),
            content_type: mime,
            content_length: length,
            keep_alive: exchange.keep_alive(),
            extra,
        })
        .is_ok_and(|bytes| send(&bytes))
    };
    if matches!(status, NOT_FOUND | 400) {
        let body = if status == NOT_FOUND {
            page_404()
        } else {
            Vec::new()
        };
        return header(handler, status, body.len(), b"text/html", b"") && (head || send(&body));
    }
    if let Some(location) = redirect {
        let extra = [b"Location: ".as_slice(), &location, b"\r\n"].concat();
        return header(handler, 301, 0, b"text/html", &extra);
    }
    let values = [
        b"If-Match".as_slice(),
        b"If-Unmodified-Since",
        b"If-None-Match",
        b"If-Modified-Since",
    ]
    .map(|key| exchange.header(key));
    let condition = handler.preconditions(values.each_ref().map(Vec::as_slice));
    if condition != PreconditionStatus::Ok {
        return header(handler, condition as i32, 0, b"text/html", b"");
    }
    if handler.lists_directory() {
        let body = handler.index_page();
        return header(handler, 200, body.len(), b"text/html", b"") && (head || send(&body));
    }
    handler.parse_range(
        c_text(&exchange.header(b"Range")),
        c_text(&exchange.header(b"If-Range")),
    );
    let tasks = handler.tasks();
    let status = handler.status();
    let extra = match (status, tasks.as_slice()) {
        (416, _) => format!("Content-Range: bytes */{}\r\n", handler.filesize()),
        (PARTIAL_CONTENT, [task]) => {
            let end = task
                .offset
                .c_cast::<usize>()
                .wrapping_add(task.length)
                .wrapping_sub(1);
            format!(
                "Content-Range: bytes {}-{end}/{}\r\n",
                task.offset,
                handler.filesize()
            )
        }
        (200, _) => String::from("Accept-Ranges: bytes\r\n"),
        _ => String::new(),
    };
    let mime = handler.content_type();
    if !header(
        handler,
        status,
        handler.content_length(),
        &mime,
        extra.as_bytes(),
    ) {
        return false;
    }
    if head || tasks.is_empty() {
        return true;
    }
    for part in &tasks {
        if tasks.len() > 1 && !send(c_text(&part.part_header)) {
            return false;
        }
        if part.length == 0 {
            break;
        }
        if !handler
            .file()
            .is_some_and(|file| exchange.send_range(file, part))
        {
            return false;
        }
    }
    tasks.len() == 1 || send(&handler.end_part())
}

/// The server, request, session and socket of an HTTP/1 static response.
struct Wire<'a> {
    server: crate::view::Server<'a>,
    request: &'a swoole_rs_http::HttpRequest,
    input: &'a [u8],
    session_id: core::ffi::c_long,
}

impl Exchange<std::sync::Arc<swoole_rs_corebase::ossupport::FileRecord>> for Wire<'_> {
    fn send_range(
        &self,
        file: &std::sync::Arc<swoole_rs_corebase::ossupport::FileRecord>,
        part: &HttpStaticTask,
    ) -> bool {
        crate::send::send_static_range(self.server, self.session_id, file, part)
    }
    fn header(&self, name: &[u8]) -> Vec<u8> {
        self.request
            .header_bytes(self.input, name)
            .map_or_default(<[u8]>::to_vec)
    }
    fn keep_alive(&self) -> bool {
        self.request.keep_alive
    }
    fn head(&self) -> bool {
        self.request.method == swoole_rs_http::METHOD_HEAD
    }
    fn send(&self, kind: ServerEvent, data: &[u8], reset: bool) -> bool {
        let info = swoole_rs_corebase::DataHead {
            fd: self.session_id,
            len: u32::try_from(data.len()).unwrap_or(u32::MAX),
            kind: kind.code(),
            ext_flags: if reset {
                crate::factory::CLOSE_RESET.c_cast()
            } else {
                0
            },
            ..Default::default()
        };
        crate::send::send_event_to_connection(self.server, &info, data)
    }
}

/// `Server::select_static_handler(request, conn)` for the ports read path: `request`
/// is the request of the connection of session `session_id`.
/// The caller owns the request, which the last send's close leaves alone; nothing
/// reads it after that send.
#[must_use]
pub fn static_select(
    server: crate::view::Server<'_>,
    request: &swoole_rs_http::HttpRequest,
    input: &[u8],
    session_id: core::ffi::c_long,
) -> bool {
    if !matches!(
        request.method,
        swoole_rs_http::METHOD_GET | swoole_rs_http::METHOD_HEAD
    ) {
        return false;
    }
    let wire = Wire {
        server,
        request,
        input,
        session_id,
    };
    let Some(mut handler) = StaticHandler::new(wire.server, request.url_bytes(input)) else {
        return false;
    };
    select(&mut handler, &wire)
}

/// The cases of the retired `tests/modules/http/static_gate.py`: [`select`] over
/// a scripted handler and request against the pinned `select_static_handler`
/// over the same script (`swoole_rs_oracle::http_static_select`), comparing the
/// result, every traced call, every send's bytes and the request's final state.
#[cfg(test)]
mod tests {
    use super::{Exchange, Handler, ServerEvent, select};
    use core::cell::{Cell, RefCell};
    use std::ffi::{c_char, c_void};
    use swoole_rs_http::{HttpStaticTask, PreconditionStatus};
    use swoole_rs_oracle::http_static_context::swr_oracle_static_arguments_left_to_right;
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

    impl Handler for &Script<'_> {
        fn try_serve(&mut self) -> bool {
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
        fn etag(&self) -> String {
            self.note("etag");
            String::from("\"tag\"")
        }
        fn directory_redirect(&self) -> Option<Vec<u8>> {
            None
        }
        fn preconditions(&self, [_, _, _, modified]: [&[u8]; 4]) -> PreconditionStatus {
            self.note("conditions");
            if !modified.is_empty() && self.case.modified {
                PreconditionStatus::NotModified
            } else {
                PreconditionStatus::Ok
            }
        }
        fn try_serve_index_file(&mut self) -> bool {
            self.note("index");
            self.case.index_ok
        }
        fn lists_directory(&self) -> bool {
            !self.case.index && self.case.autoindex && self.case.directory
        }
        fn index_page(&mut self) -> Vec<u8> {
            self.note("index-page");
            self.case.body.clone()
        }
        fn parse_range(&mut self, _: &[u8], _: &[u8]) {
            self.note("range");
        }
        fn tasks(&self) -> Vec<HttpStaticTask> {
            self.case.tasks.clone()
        }
        fn content_length(&self) -> usize {
            self.case.content_length
        }
        fn content_type(&mut self) -> Vec<u8> {
            self.note("content-type");
            self.case.content_type.clone()
        }
        fn filesize(&self) -> usize {
            self.case.filesize
        }
        type File = Vec<u8>;
        fn open_file(&mut self) -> bool {
            true
        }
        fn file(&self) -> Option<&Vec<u8>> {
            Some(&self.case.filename)
        }
        fn end_part(&self) -> Vec<u8> {
            self.case.end_part.clone()
        }
    }

    impl Exchange<Vec<u8>> for Script<'_> {
        fn send_range(&self, file: &Vec<u8>, part: &HttpStaticTask) -> bool {
            let mut event = format!("{}:{}:", part.offset, part.length).into_bytes();
            event.extend_from_slice(file);
            event.extend_from_slice(b":16");
            self.send(ServerEvent::SendFile, &event, false)
        }
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
        fn send(&self, kind: ServerEvent, data: &[u8], reset: bool) -> bool {
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
            let mut event = format!("E:73:{}:", kind.code()).into_bytes();
            if kind != ServerEvent::Close {
                event.extend_from_slice(data);
            }
            if reset {
                event.extend_from_slice(b"reset");
            }
            self.events.borrow_mut().push(event);
            self.case.send_result >= 0
        }
    }

    use swoole_rs_corebase::sys::errno;

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
            i32::from(select(&mut &script, &script))
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
        // Compare the precondition and range header reads in source order, whichever order the
        // shim's compiler gave them (GCC on x86 goes right to left).
        if unsafe { swr_oracle_static_arguments_left_to_right() } == 0 {
            for i in 1..lines.len() {
                if lines[i - 1] == b"R:If-Modified-Since"
                    && i + 2 < lines.len()
                    && lines[i + 2] == b"R:If-Match"
                {
                    lines[i - 1..=i + 2].reverse();
                }
                if lines[i - 1] == b"R:If-Range" && lines[i] == b"R:Range" {
                    lines.swap(i - 1, i);
                }
            }
        }
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
    #[cfg_attr(miri, ignore = "Miri can't call the C++ oracle")]
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
    #[cfg_attr(miri, ignore = "Miri can't call the C++ oracle")]
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
    /// body, while HEAD is captured before the first send.
    #[test]
    #[cfg_attr(miri, ignore = "Miri can't call the C++ oracle")]
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
