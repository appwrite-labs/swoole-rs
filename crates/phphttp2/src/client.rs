//! The HTTP/2 coroutine client: upstream's `swoole::coroutine::http2::Client`
//! and its PHP classes.
use crate::server::entries;
use crate::session::IdMap;
use libc::c_int;
use request::{Field, Held, Request};
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::VecDeque,
    ffi::CStr,
    rc::Rc,
};
use swoole_rs_corebase::CCast;
use swoole_rs_hooks::{co_socket::Co, packet::recv_packet, socket_extra::set_close_hook};
use swoole_rs_http2::{
    FrameAction, HeaderKind, HeaderRole, Http2FrameFacts, Http2Settings, Http2WindowPlan, Media,
    QueueAction,
    hpack::{Decoder, Encoder, Error},
    send_frame_size, sw_rs_http2_chunk, sw_rs_http2_decode_frame, sw_rs_http2_default_settings,
    sw_rs_http2_header_plan, sw_rs_http2_headers_flags, sw_rs_http2_inflate_step,
    sw_rs_http2_media_kind, sw_rs_http2_pack_frame_header, sw_rs_http2_pack_settings,
    sw_rs_http2_queue_action, sw_rs_http2_response, sw_rs_http2_send_complete,
    sw_rs_http2_setting_plan, sw_rs_http2_stream_flags, sw_rs_http2_stream_update,
    sw_rs_http2_window_plan, sw_rs_http2_write_allowed,
};
use swoole_rs_inflate::Inflate;
use swoole_rs_phpruntime::co::WRITE;
use swoole_rs_phpstubs::http2_client_coro;
use swoole_rs_zend::{
    ArrayMut, Call, Class, Custom, CustomClass, Minit, Owned, OwnedArray, Props, Value, Weak,
    ZString, sys::ZEND_ACC_PUBLIC,
};
use swoole_rs_zend::{method_table, sys as z};

/// `SW_HTTP2_*` frame sizes and the client preface.
const FRAME_HEADER: usize = 9;
const PING_PAYLOAD: usize = 8;
const GOAWAY_SIZE: usize = 8;
const WINDOW_UPDATE_SIZE: usize = 4;
const SETTING_FRAME: usize = FRAME_HEADER + 6 * 6;
const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
/// Frame types and flags.
const TYPE_DATA: u32 = 0;
const TYPE_HEADERS: u32 = 1;
const TYPE_RST_STREAM: u32 = 3;
const TYPE_SETTINGS: u32 = 4;
const TYPE_PING: u32 = 6;
const TYPE_GOAWAY: u32 = 7;
const TYPE_WINDOW_UPDATE: u32 = 8;
const FLAG_ACK: u32 = 1;
const FLAG_END_STREAM: u32 = 1;
const FLAG_PRIORITY: u32 = 0x20;
/// `SW_HTTP2_STREAM_PIPELINE_REQUEST`.
const STREAM_PIPELINE_REQUEST: u32 = 2;
/// `SW_HTTP2_SETTING_HEADER_TABLE_SIZE` and `..._MAX_HEADER_LIST_SIZE`.
const SETTING_HEADER_TABLE_SIZE: u16 = 1;
const SETTING_MAX_HEADER_LIST_SIZE: u16 = 6;
/// The `:authority` slot (`HTTP2_CLIENT_HOST_HEADER_INDEX`).
const AUTHORITY: usize = 3;
/// `SW_ERROR_*` and `HTTP_ESTATUS_SERVER_RESET`.
const INVALID_PARAMS: i64 = 505;
const QUEUE_FULL: c_int = 506;
const NO_CONNECTION: c_int = 5001;
const SEND_CONTROL_FRAME_FAILED: i64 = 3005;
const SERVER_RESET: i64 = -3;

static CLIENT: CustomClass<Data> = CustomClass::new();
static EXCEPTION: Class = Class::new();
static REQUEST: CustomClass<Plain<0>> = CustomClass::new();
static RESPONSE: CustomClass<Plain<1>> = CustomClass::new();

/// `Http2ClientObject`, holding the client itself rather than a pointer to it.
pub(crate) struct Data {
    /// The constructor's client (unset before it), dropped with the object.
    client: OnceCell<Client>,
}

/// The client is taken out before it closes, as upstream clears the pointer first.
impl Drop for Data {
    fn drop(&mut self) {
        drop(self.client.take());
    }
}

impl Custom for Data {
    fn class() -> &'static CustomClass<Self> {
        &CLIENT
    }

    fn new() -> Self {
        Self {
            client: OnceCell::new(),
        }
    }
}

/// `Swoole\Http2\Request` (0) and `Response` (1): no data, but not
/// cloneable, serializable or `unset`.
pub(crate) struct Plain<const KIND: u8>;

impl Custom for Plain<0> {
    fn class() -> &'static CustomClass<Self> {
        &REQUEST
    }

    fn new() -> Self {
        Self
    }
}

impl Custom for Plain<1> {
    fn class() -> &'static CustomClass<Self> {
        &RESPONSE
    }

    fn new() -> Self {
        Self
    }
}

/// What the socket's close hook changes, shared with the client. The hook
/// drops the socket; the client releases what hangs off it (streams, queue,
/// HPACK) the next time it looks (`reap`).
struct Conn {
    /// The client object, which a conn must not keep alive.
    object: Weak<Data>,
    socket: Cell<Option<Co>>,
    zsocket: RefCell<Owned>,
    closed: Cell<bool>,
}

impl Conn {
    /// The client object, while it lives and this is its client's conn.
    fn this(&self) -> Option<Owned> {
        let this = self.object.get()?;
        let conn = &this.custom::<Data>()?.client.get()?.conn;
        std::ptr::eq(Rc::as_ptr(conn), self).then_some(this)
    }

    /// `write` over the client object's properties, under its class; nothing
    /// once the object is gone.
    fn props(&self, write: impl FnOnce(Props<'_>)) {
        if let Some(this) = self.this()
            && let Some(props) = this.props(Some(&CLIENT))
        {
            write(props);
        }
    }
}

struct Stream {
    flags: u32,
    buffer: Vec<u8>,
    gzip: Option<Inflate>,
    response: Response,
    remote_window: u32,
    local_window: u32,
}

/// A stream's `Swoole\Http2\Response`, which this owns.
struct Response {
    zval: Owned,
}

impl Response {
    fn new(id: u32) -> Self {
        let response = Self {
            zval: RESPONSE.create_object(),
        };
        if let Some(props) = response.props() {
            props.long(c"streamId", i64::from(id));
        }
        response
    }

    /// Writes under the response class; None if Zend made no object.
    fn props(&self) -> Option<Props<'_>> {
        self.zval.props(Some(&RESPONSE))
    }
}

/// What a frame's payload held, read while the read buffer was on loan.
enum Payload {
    /// Nothing that outlives the read.
    Unread,
    /// A PING's eight bytes, if it had eight.
    Ping(Option<[u8; PING_PAYLOAD]>),
    Goaway {
        server_last: u32,
        code: u32,
        debug: Vec<u8>,
    },
    /// A header block, decoded, and its failure.
    Headers(Option<Error>),
    /// A DATA frame's length and what buffering it came to.
    Data(usize, Buffered),
}

/// A DATA frame's payload on its stream.
#[derive(Clone, Copy)]
enum Buffered {
    /// No stream, or no bytes.
    Nothing,
    /// The body did not decompress.
    Failed,
    /// Buffered: the stream's window plan.
    Window(Http2WindowPlan),
}

/// `ReturnCode` of `parse_frame`.
enum Frame {
    Continue,
    Ready,
    Fail,
}

/// The object's client. Coroutines share it: a method that yields (on socket
/// IO) or runs PHP code lets another coroutine call methods on the same object,
/// so it is only borrowed shared, its state is in cells, and no `RefCell`
/// borrow is held across a yield or a call into PHP.
struct Client {
    conn: Rc<Conn>,
    host: RefCell<Vec<u8>>,
    port: i64,
    ssl: bool,
    stream_id: Cell<u32>,
    last_stream_id: Cell<u32>,
    local: Http2Settings,
    remote: Cell<Http2Settings>,
    remote_window: Cell<u32>,
    local_window: Cell<u32>,
    streams: RefCell<Streams>,
    /// The request header arena, taken out while a request is built.
    headers: Cell<Headers>,
    queue: RefCell<VecDeque<Vec<u8>>>,
    /// The connection's HPACK codecs, while connected.
    hpack: RefCell<Option<(Decoder, Encoder)>>,
}

fn strerror(code: c_int) -> String {
    swoole_rs_corebase::sys::strerror(code)
}

/// `php_swoole_socket_set` when `settings` is an array.
fn apply(socket: Co, settings: &Value) {
    if let Some(settings) = settings.array() {
        // Every setting.
        swoole_rs_phpclients::socket_set(socket, settings);
    }
}

fn frame_header(kind: u32, length: usize, flags: u32, stream: u32) -> [u8; FRAME_HEADER] {
    let length = u32::try_from(length).unwrap_or(u32::MAX);
    sw_rs_http2_pack_frame_header(kind, length, flags, stream)
}

fn window(current: u32, amount: usize, initial: u32, credit: bool) -> Http2WindowPlan {
    sw_rs_http2_window_plan(current, amount, initial, credit)
}

/// The HPACK header list upstream's `HeaderSet` builds (names lowercased),
/// in one byte arena the client reuses across requests.
#[derive(Default)]
struct Headers {
    bytes: Vec<u8>,
    /// Each slot's name and value as (start, name length, value length).
    slots: Vec<Option<(usize, usize, usize)>>,
}

impl Headers {
    fn clear(&mut self) {
        self.bytes.clear();
        self.slots.clear();
    }

    fn store(&mut self, name: &[u8], value: &[&[u8]]) -> (usize, usize, usize) {
        let start = self.bytes.len();
        self.bytes.extend(name.iter().map(u8::to_ascii_lowercase));
        for part in value {
            self.bytes.extend_from_slice(part);
        }
        (start, name.len(), self.bytes.len() - start - name.len())
    }

    fn add(&mut self, name: &[u8], value: &[&[u8]]) {
        let slot = self.store(name, value);
        self.slots.push(Some(slot));
    }

    fn set(&mut self, index: usize, name: &[u8], value: &[u8]) {
        let stored = self.store(name, &[value]);
        if let Some(slot) = self.slots.get_mut(index) {
            *slot = Some(stored);
        }
    }

    /// Each slot's name and value (an unset slot's both empty).
    fn list(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
        self.slots.iter().map(|slot| {
            let (start, name, value) = slot.unwrap_or_default();
            let field = self
                .bytes
                .get(start..start + name + value)
                .unwrap_or_default();
            field.split_at(name.min(field.len()))
        })
    }
}

type Streams<T = Stream> = IdMap<u32, T>;

/// The socket's close hook (`socket_dtor`), over the conn `connect` gave it.
fn socket_closed(conn: &Conn) {
    conn.socket.set(None);
    conn.closed.set(true);
    conn.props(|props| {
        props.bool(c"connected", false);
        props.null(c"socket");
    });
    // Emptied before the socket object's reference goes.
    let zsocket = conn.zsocket.take();
    drop(zsocket);
}

impl Client {
    fn new(object: Weak<Data>, host: &[u8], port: i64, ssl: bool) -> Self {
        let conn = Rc::new(Conn {
            object,
            socket: Cell::new(None),
            zsocket: RefCell::default(),
            closed: Cell::new(false),
        });
        let local = sw_rs_http2_default_settings();
        Self {
            conn,
            host: RefCell::new(host.to_vec()),
            port,
            ssl,
            stream_id: Cell::new(0),
            last_stream_id: Cell::new(0),
            local,
            remote: Cell::default(),
            remote_window: Cell::new(0),
            local_window: Cell::new(local.init_window_size),
            streams: RefCell::default(),
            headers: Cell::default(),
            queue: RefCell::default(),
            hpack: RefCell::new(None),
        }
    }

    /// The live socket, read fresh: the close hook may have cleared it.
    fn socket(&self) -> Option<Co> {
        self.conn.socket.get()
    }

    /// `SW_CLIENT_PRESERVE_SOCKET`: a reference to the socket object for the call.
    fn hold_socket(&self) -> Owned {
        self.conn.zsocket.borrow().copy()
    }

    /// `socket_dtor`'s release of what hung off the closed socket.
    fn reap(&self) {
        if !self.conn.closed.replace(false) {
            return;
        }
        self.queue.take();
        self.streams.take();
        self.hpack.take();
    }

    /// `php_swoole_socket_set_error_properties(zobject, code, message)`.
    fn update_error(&self, code: c_int, message: &[u8]) {
        swoole_rs_corebase::log::set_last_error(code);
        self.conn.props(|props| {
            props.long(c"errCode", i64::from(code));
            props.bytes(c"errMsg", message);
        });
    }

    /// `io_error`: the socket's errCode and errMsg.
    fn io_error(&self, socket: Co) {
        swoole_rs_phpclients::Co::from(socket)
            .with_error(|code, message| self.update_error(code, message.to_bytes()));
    }

    fn is_available(&self) -> bool {
        if !self.socket().is_some_and(Co::is_connected) {
            let message = swoole_rs_phpruntime::strerror(NO_CONNECTION);
            self.update_error(NO_CONNECTION, message.to_bytes());
            return false;
        }
        true
    }

    fn connect(&self) -> bool {
        self.reap();
        if self.socket().is_some() {
            self.update_error(libc::EISCONN, strerror(libc::EISCONN).as_bytes());
            return false;
        }
        // `php_swoole_create_socket(convert_to_type(host))`.
        let kind = swoole_rs_reactor::host_type(&self.host.borrow());
        {
            let mut host = self.host.borrow_mut();
            let erase = kind.erase.min(host.len());
            host.drain(..erase);
        }
        // The conn takes the new object's reference.
        let Some(zsocket) = swoole_rs_phpclients::create_socket(kind.socket_type) else {
            if !swoole_rs_zend::bailout::pending() {
                let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                self.update_error(error, strerror(error).as_bytes());
            }
            return false;
        };
        // The new socket object's socket, which lives as long as it; the conn's
        // reference to the object keeps it live until the close hook clears this
        // handle.
        let Some(socket) =
            swoole_rs_phpclients::get_socket(&zsocket).map(swoole_rs_phpclients::Co::handle)
        else {
            return false;
        };
        *self.conn.zsocket.borrow_mut() = zsocket;
        self.conn.socket.set(Some(socket));
        let conn = Rc::clone(&self.conn);
        set_close_hook(socket, Box::new(move |_| socket_closed(&conn)));
        socket.set_zero_copy(true);
        if self.ssl && !socket.enable_ssl() {
            return self.connect_failed(socket);
        }
        // HTTP/2 framing on the socket, then the settings.
        socket.set_http2(true);
        socket.set_open_length_check(true);
        let mut protocol = socket.protocol();
        // SW_HTTP2_FRAME_HEADER_SIZE.
        protocol.package_length_size = 9;
        protocol.package_length_offset = 0;
        protocol.package_body_offset = 0;
        protocol.get_package_length = Some(swoole_rs_ports::http_routes::exports::http2_length);
        socket.set_protocol(protocol);
        if let Some(this) = self.conn.this()
            && let Some(setting) = this.property(c"setting")
        {
            apply(socket, setting);
        }
        let host = self.host.borrow().clone();
        if !socket.connect_host(&host, c_int::try_from(self.port).unwrap_or(-1)) {
            return self.connect_failed(socket);
        }
        self.stream_id.set(1);
        let remote = sw_rs_http2_default_settings();
        self.remote.set(remote);
        self.remote_window.set(remote.init_window_size);
        self.hpack.replace(Some((
            Decoder::default(),
            Encoder::new(self.local.header_table_size),
        )));
        if !self.send(PREFACE) || !self.send_setting() {
            self.close();
            return false;
        }
        self.conn.props(|props| {
            props.own().value(c"socket", &self.conn.zsocket.borrow());
            props.bool(c"connected", true);
        });
        true
    }

    fn connect_failed(&self, socket: Co) -> bool {
        self.io_error(socket);
        self.close();
        false
    }

    fn close(&self) -> bool {
        let Some(socket) = self.socket() else {
            self.update_error(libc::EBADF, strerror(libc::EBADF).as_bytes());
            return false;
        };
        self.conn
            .props(|props| props.own().bool(c"connected", false));
        // Its members are not read after a close.
        if !socket.close() {
            self.io_error(socket);
            return false;
        }
        self.reap();
        true
    }

    /// `send`: directly, or queued behind a coroutine writing already.
    fn send(&self, data: &[u8]) -> bool {
        let Some(socket) = self.socket() else {
            return false;
        };
        let bound = socket.has_bound(WRITE);
        let (queued, limit) = if bound {
            (
                self.queue.borrow().len(),
                self.remote.get().max_concurrent_streams,
            )
        } else {
            (0, 0)
        };
        match sw_rs_http2_queue_action(bound, queued, limit) {
            QueueAction::Send => {}
            QueueAction::Full => {
                socket.set_err_code(QUEUE_FULL);
                socket.set_static_err_msg(c"the send queue is full, try again later");
                self.io_error(socket);
                return false;
            }
            QueueAction::Append => {
                self.queue.borrow_mut().push_back(data.to_vec());
                return true;
            }
        }
        if !Self::send_all(socket, data) {
            self.io_error(socket);
            return false;
        }
        // Each frame is copied out: sending yields, and the queue's borrow ends first.
        loop {
            let next = self.queue.borrow().front().cloned();
            let Some(frame) = next else { break };
            if !Self::send_all(socket, &frame) {
                self.io_error(socket);
                EXCEPTION.throw(c"failed to send control frame", SEND_CONTROL_FRAME_FAILED);
                return false;
            }
            self.queue.borrow_mut().pop_front();
        }
        true
    }

    fn send_all(socket: Co, data: &[u8]) -> bool {
        sw_rs_http2_send_complete(socket.send_all(data), data.len())
    }

    fn send_setting(&self) -> bool {
        let mut frame = [0_u8; SETTING_FRAME];
        let length = sw_rs_http2_pack_settings(&mut frame, &self.local, false);
        self.send(frame.get(..length).unwrap_or(&frame))
    }

    fn send_window_update(&self, stream: u32, size: u32) -> bool {
        let mut frame = frame_header(TYPE_WINDOW_UPDATE, WINDOW_UPDATE_SIZE, 0, stream).to_vec();
        frame.extend_from_slice(&size.to_be_bytes());
        self.send(&frame)
    }

    fn send_ping(&self) -> bool {
        let mut frame = frame_header(TYPE_PING, PING_PAYLOAD, 0, 0).to_vec();
        frame.extend_from_slice(&[0; PING_PAYLOAD]);
        self.send(&frame)
    }

    /// DATA frames within the peer's frame size; one empty frame for no data.
    fn send_data(&self, stream: u32, mut data: &[u8], flag: u32) -> bool {
        if data.is_empty() {
            return self.send(&frame_header(TYPE_DATA, 0, flag, stream));
        }
        while !data.is_empty() {
            let max_frame_size = send_frame_size(self.remote.get().max_frame_size);
            let chunk = sw_rs_http2_chunk(data.len(), max_frame_size, flag);
            let (part, rest) = data.split_at(chunk.length.min(data.len()));
            if !self.send(&frame_header(TYPE_DATA, part.len(), chunk.flags, stream))
                || !self.send(part)
            {
                return false;
            }
            data = rest;
        }
        true
    }

    fn send_goaway(&self, error_code: i64, debug: &[u8]) -> bool {
        let code = error_code.c_cast::<u32>();
        let mut frame =
            frame_header(TYPE_GOAWAY, GOAWAY_SIZE + debug.len(), code & 0xff, 0).to_vec();
        frame.extend_from_slice(&self.last_stream_id.get().to_be_bytes());
        frame.extend_from_slice(&code.to_be_bytes());
        frame.extend_from_slice(debug);
        self.send(&frame)
    }

    /// `build_header`: a HEADERS frame whose first nine bytes are left for
    /// its header, then the request's HPACK block.
    fn build_header(&self, request: Request<'_>) -> Vec<u8> {
        let text = |field| {
            request
                .property(field)?
                .bytes()
                .filter(|text| !text.is_empty())
        };
        let (method, path) = (text(Field::Method), text(Field::Path));
        let headers = request.array(Held::Headers);
        let cookies = request.array(Held::Cookies);
        // Converting a header value can run PHP code, which may build another request.
        let mut set = self.headers.take();
        set.clear();
        set.add(b":method", &[method.unwrap_or(b"GET")]);
        set.add(b":path", &[path.unwrap_or(b"/")]);
        set.add(b":scheme", &[if self.ssl { b"https" } else { b"http" }]);
        set.slots.push(None);
        let mut host = false;
        for (key, value) in entries(headers.as_ref()) {
            let Some(key) = key.filter(|key| key.first() != Some(&b':') && !value.is_null()) else {
                continue;
            };
            let value = value.string();
            if header_kind(HeaderRole::Client, key) == HeaderKind::Host {
                set.set(AUTHORITY, b":authority", value.bytes());
                host = true;
            } else {
                set.add(key, &[value.bytes()]);
            }
        }
        if !host {
            let authority = if self.port == if self.ssl { 443 } else { 80 } {
                self.host.borrow().clone()
            } else {
                let host = self.host.borrow();
                let host = host.split(|&b| b == 0).next().unwrap_or_default();
                [host, format!(":{}", self.port).as_bytes()].concat()
            };
            set.set(AUTHORITY, b":authority", &authority);
        }
        for (key, value) in entries(cookies.as_ref()) {
            let Some(key) = key.filter(|_| !value.is_null()) else {
                continue;
            };
            let value = value.string();
            let encoded = ZString::url_encode(value.bytes());
            set.add(b"cookie", &[key, b"=", encoded.bytes()]);
        }
        let mut frame = vec![0_u8; FRAME_HEADER];
        if let Some((_, encoder)) = self.hpack.borrow_mut().as_mut() {
            encoder.encode(set.list(), &mut frame);
        }
        self.headers.set(set);
        frame
    }

    /// `send_request`: the new stream's id, or 0.
    fn send_request(&self, request: Request<'_>) -> u32 {
        // The class declares every field, so a request has its data.
        let Some(data) = request.property(Field::Data) else {
            return 0;
        };
        // `headers` becomes an array, and a form's type is added to it.
        let form = data.array().is_some();
        if let Some(props) = request.props() {
            props.edit_array(c"headers", |headers| {
                if form {
                    headers
                        .insert_bytes(b"content-type", Some(b"application/x-www-form-urlencoded"));
                }
            });
        }
        let truthy = |field| request.property(field).is_some_and(Value::truthy);
        let pipeline = truthy(Field::Pipeline);
        let use_pipeline_read = truthy(Field::UsePipelineRead);
        let empty = match data.bytes() {
            Some(text) => text.is_empty(),
            None => !data.truthy(),
        };
        let mut frame = self.build_header(request);
        let flags = sw_rs_http2_stream_flags(pipeline, use_pipeline_read);
        // The id is taken before a send yields or the body's conversion runs PHP
        // code, so an overlapping send gets the next one. A failed send gives it
        // back unless a later send has taken one since.
        let id = self.stream_id.get();
        self.stream_id.set(id + 2);
        self.create_stream(id, flags);
        let header_flags = sw_rs_http2_headers_flags(empty, true, flags);
        let header = frame_header(TYPE_HEADERS, frame.len() - FRAME_HEADER, header_flags, id);
        if let Some(head) = frame.first_chunk_mut::<FRAME_HEADER>() {
            *head = header;
        }
        let flag = if flags & STREAM_PIPELINE_REQUEST != 0 {
            0
        } else {
            FLAG_END_STREAM
        };
        let sent = self.send(&frame)
            && (empty
                || body_bytes(data).is_some_and(|body| self.send_data(id, body.bytes(), flag)));
        if !sent {
            if self.stream_id.get() == id + 2 {
                self.stream_id.set(id);
            }
            return 0;
        }
        id
    }

    fn create_stream(&self, id: u32, flags: u32) {
        let stream = Stream {
            flags,
            buffer: Vec::new(),
            gzip: None,
            response: Response::new(id),
            remote_window: self.remote.get().init_window_size,
            local_window: self.local.init_window_size,
        };
        let replaced = self.streams.borrow_mut().insert(id, stream);
        drop(replaced);
    }

    /// `write_data`: more of a pipeline request's body.
    fn write_data(&self, id: u32, data: Option<&Value>, end: bool) -> bool {
        let flags = self.streams.borrow().get(&id).map(|stream| stream.flags);
        if !sw_rs_http2_write_allowed(flags.is_some(), flags.unwrap_or(0)) {
            let message = format!("unable to found active pipeline stream#{id}");
            self.update_error(libc::EINVAL, message.as_bytes());
            return false;
        }
        let flag = if end { FLAG_END_STREAM } else { 0 };
        let Some(body) = data.and_then(body_bytes) else {
            return false;
        };
        if !self.send_data(id, body.bytes(), flag) {
            return false;
        }
        if let Some(stream) = self.streams.borrow_mut().get_mut(&id) {
            stream.flags = sw_rs_http2_stream_update(stream.flags, end, false);
        }
        true
    }

    /// `recv_packet` then `parse_frame`, until a response is ready.
    fn recv(&self, call: &Call, timeout: f64, pipeline_read: bool) {
        loop {
            self.reap();
            let Some(socket) = self.socket().filter(|_| self.is_available()) else {
                return call.set_bool(false);
            };
            let received = recv_packet(socket, timeout);
            if received <= 0 {
                self.io_error(socket);
                return call.set_bool(false);
            }
            match self.parse_frame(call, socket, received.unsigned_abs(), pipeline_read) {
                Frame::Continue => {}
                Frame::Ready => return,
                Frame::Fail => {
                    self.reap();
                    return call.set_bool(false);
                }
            }
        }
    }

    /// What the frame at the front of `packet` holds, with everything its
    /// payload is needed for done: the read buffer is on loan to the caller, so
    /// nothing here yields or runs PHP code.
    fn read_frame(&self, packet: &[u8]) -> (Http2FrameFacts, Payload) {
        let (head, body) = packet.split_at_checked(FRAME_HEADER).unwrap_or_default();
        let facts =
            sw_rs_http2_decode_frame(head, self.last_stream_id.get(), true).unwrap_or_default();
        let length = facts.length as usize;
        // The length protocol's packet is the header and `length` payload bytes.
        let payload = body.get(..length).unwrap_or(body);
        let id = facts.stream_id;
        if facts.publish_last_id {
            self.last_stream_id.set(id);
        }
        let known = || self.streams.borrow().contains_key(&id);
        let read = match facts.action {
            FrameAction::Settings => {
                for setting in payload.chunks(6) {
                    let mut raw = [0_u8; 6];
                    for (slot, byte) in raw.iter_mut().zip(setting) {
                        *slot = *byte;
                    }
                    let key = u16::from_be_bytes([raw[0], raw[1]]);
                    let number = u32::from_be_bytes([raw[2], raw[3], raw[4], raw[5]]);
                    self.apply_remote_setting(key, number);
                }
                Payload::Unread
            }
            FrameAction::WindowUpdate => {
                let credit = read_u32(payload);
                if id == 0 {
                    let plan = window(self.remote_window.get(), credit as usize, 0, true);
                    self.remote_window.set(plan.value);
                } else if let Some(stream) = self.streams.borrow_mut().get_mut(&id) {
                    stream.remote_window =
                        window(stream.remote_window, credit as usize, 0, true).value;
                }
                Payload::Unread
            }
            // RFC 9113 6.7: the ACK echoes the PING's eight payload bytes; any
            // other length is a FRAME_SIZE_ERROR.
            FrameAction::PingReply => Payload::Ping(<[u8; PING_PAYLOAD]>::try_from(payload).ok()),
            FrameAction::Goaway => Payload::Goaway {
                server_last: read_u32(payload),
                code: read_u32(payload.get(4..).unwrap_or_default()),
                debug: payload.get(GOAWAY_SIZE..).unwrap_or_default().to_vec(),
            },
            FrameAction::Headers if known() => {
                Payload::Headers(self.parse_header(id, facts.flags, payload))
            }
            FrameAction::Data if known() => {
                Payload::Data(payload.len(), self.buffer_data(id, facts.flags, payload))
            }
            _ => Payload::Unread,
        };
        (facts, read)
    }

    /// The frame in the `received` bytes `recv_packet` left in the read buffer.
    fn parse_frame(&self, call: &Call, socket: Co, received: usize, pipeline_read: bool) -> Frame {
        // The packet `recv_packet` just returned starts the read buffer. It is
        // read under the buffer's loan; what sends or runs PHP code comes after.
        let (facts, read) = socket.with_buffer(false, |buffer| {
            let buffered = buffer.view();
            self.read_frame(buffered.get(..received).unwrap_or(buffered))
        });
        let id = facts.stream_id;
        match facts.action {
            FrameAction::SettingsAck
            | FrameAction::PingAck
            | FrameAction::IgnorePush
            | FrameAction::WindowUpdate => {
                return Frame::Continue;
            }
            FrameAction::Settings => {
                if !self.send(&frame_header(TYPE_SETTINGS, 0, FLAG_ACK, id)) {
                    return Frame::Fail;
                }
                return Frame::Continue;
            }
            FrameAction::PingReply => {
                let Payload::Ping(Some(echo)) = read else {
                    return Frame::Fail;
                };
                let mut frame = frame_header(TYPE_PING, PING_PAYLOAD, FLAG_ACK, id).to_vec();
                frame.extend_from_slice(&echo);
                return if self.send(&frame) {
                    Frame::Continue
                } else {
                    Frame::Fail
                };
            }
            FrameAction::Goaway => {
                if let Payload::Goaway {
                    server_last,
                    code,
                    debug,
                } = read
                {
                    self.conn.props(|props| {
                        props.long(c"errCode", i64::from(code));
                        props.bytes(c"errMsg", &debug);
                        props.long(c"serverLastStreamId", i64::from(server_last));
                    });
                }
                self.close();
                return Frame::Fail;
            }
            FrameAction::Reset => {
                let reset = self.streams.borrow_mut().remove(&id);
                drop(reset);
                return Frame::Continue;
            }
            _ => {}
        }
        if !self.streams.borrow().contains_key(&id) {
            // Upstream's "%d" of the unsigned stream id.
            let id = id.c_cast::<i32>();
            swoole_rs_corebase::log::notice(format_args!(
                "http2 stream#{id} belongs to an unknown type or it never registered"
            ));
            return Frame::Continue;
        }
        match read {
            Payload::Headers(Some(error)) => {
                let text = format!("nghttp2_hd_inflate_hd failed with error: {error}");
                self.update_error(Error::CODE, text.as_bytes());
            }
            Payload::Data(length, buffered) => {
                if let Err(frame) = self.open_windows(id, length, buffered) {
                    return frame;
                }
            }
            _ => {}
        }
        let ready = {
            let mut streams = self.streams.borrow_mut();
            let Some(stream) = streams.get_mut(&id) else {
                return Frame::Continue;
            };
            let plan = sw_rs_http2_response(facts.kind, facts.flags, stream.flags, pipeline_read);
            if !plan.ready {
                return Frame::Continue;
            }
            // Only this client has seen the response so far: no user code runs
            // on these writes.
            if let Some(response) = stream.response.props() {
                if facts.kind == TYPE_RST_STREAM {
                    response.own().long(c"statusCode", SERVER_RESET);
                    // A reset deletes its stream above, so upstream's stream_error is 0 here.
                    response.own().long(c"errCode", 0);
                }
                if !stream.buffer.is_empty() {
                    response.bytes(c"data", &stream.buffer);
                }
            }
            stream.buffer.clear();
            // The return value takes the stream's response; a pipeline stream
            // goes on with a fresh one.
            if plan.end {
                streams.remove(&id).map(|stream| stream.response.zval)
            } else {
                if let Some(response) = stream.response.props() {
                    response.bool(c"pipeline", true);
                }
                Some(std::mem::replace(&mut stream.response, Response::new(id)).zval)
            }
        };
        if let Some(response) = ready {
            call.set_owned(response);
        }
        Frame::Ready
    }

    fn apply_remote_setting(&self, key: u16, number: u32) {
        let mut remote = self.remote.get();
        let old = match key {
            SETTING_HEADER_TABLE_SIZE => remote.header_table_size,
            SETTING_MAX_HEADER_LIST_SIZE => remote.max_header_list_size,
            _ => 0,
        };
        let plan = sw_rs_http2_setting_plan(u32::from(key), number, old, false, false);
        if plan.set_value {
            let field = match plan.field {
                0 => &mut remote.header_table_size,
                1 => &mut remote.enable_push,
                2 => &mut remote.max_concurrent_streams,
                3 => &mut remote.init_window_size,
                4 => &mut remote.max_frame_size,
                _ => &mut remote.max_header_list_size,
            };
            *field = number;
            self.remote.set(remote);
        }
        if plan.set_window {
            self.remote_window.set(number);
        }
        if plan.change_deflater
            && let Some((_, encoder)) = self.hpack.borrow_mut().as_mut()
        {
            encoder.set_table_size(number);
        }
    }

    /// A DATA frame: buffer (inflating gzip) and replenish the windows.
    /// A DATA frame's payload onto its stream's body, and the stream's window
    /// after it.
    fn buffer_data(&self, id: u32, flags: u32, payload: &[u8]) -> Buffered {
        let mut streams = self.streams.borrow_mut();
        let Some(stream) = streams.get_mut(&id) else {
            return Buffered::Nothing;
        };
        stream.flags = sw_rs_http2_stream_update(stream.flags, flags & FLAG_END_STREAM != 0, true);
        if payload.is_empty() {
            return Buffered::Nothing;
        }
        let buffered = if let Some(gzip) = &mut stream.gzip {
            gzip.decode(payload)
                .map(|output| stream.buffer.extend_from_slice(output))
        } else {
            stream.buffer.extend_from_slice(payload);
            Some(())
        };
        if buffered.is_none() {
            return Buffered::Failed;
        }
        stream.local_window = window(stream.local_window, payload.len(), 0, false).value;
        Buffered::Window(window(
            stream.local_window,
            0,
            self.local.init_window_size,
            false,
        ))
    }

    /// The window updates a buffered DATA frame of `length` bytes calls for.
    fn open_windows(&self, id: u32, length: usize, buffered: Buffered) -> Result<(), Frame> {
        let initial = self.local.init_window_size;
        let stream_window = match buffered {
            Buffered::Nothing => return Ok(()),
            Buffered::Failed => {
                swoole_rs_corebase::log::warning(
                    c"Client::parse_frame",
                    format_args!("decompress failed"),
                );
                return Err(Frame::Fail);
            }
            Buffered::Window(window) => window,
        };
        let connection = window(self.local_window.get(), length, initial, false);
        self.local_window.set(connection.value);
        if connection.increment != 0 {
            if !self.send_window_update(0, connection.increment) {
                return Err(Frame::Fail);
            }
            self.local_window.set(initial);
        }
        if stream_window.increment != 0 {
            if !self.send_window_update(id, stream_window.increment) {
                return Err(Frame::Fail);
            }
            if let Some(stream) = self.streams.borrow_mut().get_mut(&id) {
                stream.local_window = initial;
            }
        }
        Ok(())
    }

    /// `parse_header`: HPACK-decode a HEADERS block onto the stream's response.
    /// Its failure is the caller's to report.
    fn parse_header(&self, id: u32, flags: u32, mut input: &[u8]) -> Option<Error> {
        if flags & FLAG_PRIORITY != 0 {
            input = input.get(5..).unwrap_or_default();
        }
        // The response, held apart from the streams' borrow.
        let response = self
            .streams
            .borrow()
            .get(&id)
            .map(|stream| stream.response.zval.copy());
        let props = response.as_ref()?.props(Some(&RESPONSE))?;
        // The three arrays the block fills, out of their properties meanwhile.
        props
            .edit_array(c"headers", |headers| {
                props.edit_array(c"cookies", |cookies| {
                    props.edit_array(c"set_cookie_headers", |set_cookie_headers| {
                        self.decode_headers(id, input, headers, cookies, set_cookie_headers)
                    })
                })
            })
            .flatten()
            .flatten()
            .flatten()
    }

    /// A header block onto the response's `headers`, `cookies` and
    /// `set_cookie_headers`; its failure.
    fn decode_headers(
        &self,
        id: u32,
        mut input: &[u8],
        headers: &mut ArrayMut,
        cookies: &mut ArrayMut,
        set_cookie_headers: &mut ArrayMut,
    ) -> Option<Error> {
        // Nothing in the loop yields or runs user code: the response is this
        // client's alone until a receive returns it.
        let mut hpack = self.hpack.borrow_mut();
        let (decoder, _) = hpack.as_mut()?;
        let failed = loop {
            let step = match decoder.decode(input, true) {
                Ok(step) => step,
                Err(error) => break Some(error),
            };
            input = input.get(step.read..).unwrap_or_default();
            let flags = step.flags();
            if let Some((name, text)) = step.header {
                let kind = header_kind(HeaderRole::Response, name);
                if name.first() == Some(&b':') {
                    if kind == HeaderKind::Status
                        && let Some(stream) = self.streams.borrow().get(&id)
                        && let Some(props) = stream.response.props()
                    {
                        props.own().long(c"statusCode", atol(text));
                    }
                } else {
                    if kind == HeaderKind::ContentEncoding && media_kind(text) == Media::Gzip {
                        if let Some(stream) = self.streams.borrow_mut().get_mut(&id) {
                            stream.gzip = Some(Inflate::gzip_only());
                        }
                    } else if kind == HeaderKind::SetCookie {
                        swoole_rs_phpclients::parse_set_cookies(text, cookies, set_cookie_headers);
                    }
                    headers.insert_bytes(name, Some(text));
                }
            }
            let step = sw_rs_http2_inflate_step(flags, input.len(), true);
            // The input is always the whole block, so a finished step ends it, even
            // on a field before a final step (where upstream leaves nghttp2 in the
            // block and refuses the next block's table size update).
            if step.end_headers || step.finish {
                decoder.end_headers();
                break None;
            }
        };
        drop(hpack);
        failed
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if self.socket().is_some() {
            self.close();
        }
        self.reap();
        self.streams.get_mut().clear();
        // Emptied first, as the close hook does, in case releasing it runs the hook.
        let zsocket = self.conn.zsocket.take();
        drop(zsocket);
    }
}

/// `php_swoole_error(E_WARNING, "%s", message)`.
fn warn(message: &CStr) {
    use swoole_rs_phpruntime::{Report, report};
    swoole_rs_zend::bailout::defer(report(Report::Swoole, swoole_rs_zend::E_WARNING, message));
}

fn read_u32(bytes: &[u8]) -> u32 {
    let mut raw = [0_u8; 4];
    for (slot, byte) in raw.iter_mut().zip(bytes) {
        *slot = *byte;
    }
    u32::from_be_bytes(raw)
}

/// `sw_atol`.
fn atol(text: &[u8]) -> i64 {
    let digits = text.split(|b| !b.is_ascii_digit()).next();
    std::str::from_utf8(digits.unwrap_or_default())
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(0)
}

/// `rust_fold_header`: at most 33 bytes, lowercased.
fn fold(bytes: &[u8]) -> ([u8; 33], usize) {
    let mut folded = [0_u8; 33];
    let n = bytes.len().min(folded.len());
    for (slot, byte) in folded.iter_mut().zip(bytes) {
        *slot = byte.to_ascii_lowercase();
    }
    (folded, n)
}

fn header_kind(role: HeaderRole, name: &[u8]) -> HeaderKind {
    let (folded, n) = fold(name);
    sw_rs_http2_header_plan(role, folded.get(..n).unwrap_or_default(), name.len(), false).kind
}

fn media_kind(value: &[u8]) -> Media {
    let (folded, n) = fold(value);
    sw_rs_http2_media_kind(folded.get(..n).unwrap_or_default())
}

/// A request body: an array as a query string, else the value as a string;
/// None after `http_build_query failed`.
fn body_bytes(data: &Value) -> Option<ZString> {
    if data.array().is_some() {
        let query = data.http_build_query();
        if query.is_none() {
            warn(c"http_build_query failed");
        }
        return query;
    }
    Some(data.string())
}

/// A table of no methods, as `register_object` takes one.
static NO_METHODS: swoole_rs_zend::arginfo::Methods<1> =
    swoole_rs_zend::arginfo::Methods([swoole_rs_zend::arginfo::END]);

/// `SW_REGISTER_LONG_CONSTANT`s of the frame types and error codes.
const CONSTANTS: [(&CStr, i64); 24] = [
    (c"SWOOLE_HTTP2_TYPE_DATA", 0),
    (c"SWOOLE_HTTP2_TYPE_HEADERS", 1),
    (c"SWOOLE_HTTP2_TYPE_PRIORITY", 2),
    (c"SWOOLE_HTTP2_TYPE_RST_STREAM", 3),
    (c"SWOOLE_HTTP2_TYPE_SETTINGS", 4),
    (c"SWOOLE_HTTP2_TYPE_PUSH_PROMISE", 5),
    (c"SWOOLE_HTTP2_TYPE_PING", 6),
    (c"SWOOLE_HTTP2_TYPE_GOAWAY", 7),
    (c"SWOOLE_HTTP2_TYPE_WINDOW_UPDATE", 8),
    (c"SWOOLE_HTTP2_TYPE_CONTINUATION", 9),
    (c"SWOOLE_HTTP2_ERROR_NO_ERROR", 0),
    (c"SWOOLE_HTTP2_ERROR_PROTOCOL_ERROR", 1),
    (c"SWOOLE_HTTP2_ERROR_INTERNAL_ERROR", 2),
    (c"SWOOLE_HTTP2_ERROR_FLOW_CONTROL_ERROR", 3),
    (c"SWOOLE_HTTP2_ERROR_SETTINGS_TIMEOUT", 4),
    (c"SWOOLE_HTTP2_ERROR_STREAM_CLOSED", 5),
    (c"SWOOLE_HTTP2_ERROR_FRAME_SIZE_ERROR", 6),
    (c"SWOOLE_HTTP2_ERROR_REFUSED_STREAM", 7),
    (c"SWOOLE_HTTP2_ERROR_CANCEL", 8),
    (c"SWOOLE_HTTP2_ERROR_COMPRESSION_ERROR", 9),
    (c"SWOOLE_HTTP2_ERROR_CONNECT_ERROR", 10),
    (c"SWOOLE_HTTP2_ERROR_ENHANCE_YOUR_CALM", 11),
    (c"SWOOLE_HTTP2_ERROR_INADEQUATE_SECURITY", 12),
    (c"SWOOLE_HTTP2_ERROR_HTTP_1_1_REQUIRED", 13),
];

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_http2_client_coro___construct, &http2_client_coro::arginfo_class_Swoole_Coroutine_Http2_Client___construct, z::ZEND_ACC_PUBLIC;
        c"__destruct" => zim_swoole_http2_client_coro___destruct, &http2_client_coro::arginfo_class_Swoole_Coroutine_Http2_Client___destruct, z::ZEND_ACC_PUBLIC;
        c"set" => zim_swoole_http2_client_coro_set, &http2_client_coro::arginfo_class_Swoole_Coroutine_Http2_Client_set, z::ZEND_ACC_PUBLIC;
        c"connect" => zim_swoole_http2_client_coro_connect, &http2_client_coro::arginfo_class_Swoole_Coroutine_Http2_Client_connect, z::ZEND_ACC_PUBLIC;
        c"stats" => zim_swoole_http2_client_coro_stats, &http2_client_coro::arginfo_class_Swoole_Coroutine_Http2_Client_stats, z::ZEND_ACC_PUBLIC;
        c"isStreamExist" => zim_swoole_http2_client_coro_isStreamExist, &http2_client_coro::arginfo_class_Swoole_Coroutine_Http2_Client_isStreamExist, z::ZEND_ACC_PUBLIC;
        c"send" => zim_swoole_http2_client_coro_send, &http2_client_coro::arginfo_class_Swoole_Coroutine_Http2_Client_send, z::ZEND_ACC_PUBLIC;
        c"write" => zim_swoole_http2_client_coro_write, &http2_client_coro::arginfo_class_Swoole_Coroutine_Http2_Client_write, z::ZEND_ACC_PUBLIC;
        c"recv" => zim_swoole_http2_client_coro_recv, &http2_client_coro::arginfo_class_Swoole_Coroutine_Http2_Client_recv, z::ZEND_ACC_PUBLIC;
        c"read" => zim_swoole_http2_client_coro_read, &http2_client_coro::arginfo_class_Swoole_Coroutine_Http2_Client_read, z::ZEND_ACC_PUBLIC;
        c"goaway" => zim_swoole_http2_client_coro_goaway, &http2_client_coro::arginfo_class_Swoole_Coroutine_Http2_Client_goaway, z::ZEND_ACC_PUBLIC;
        c"ping" => zim_swoole_http2_client_coro_ping, &http2_client_coro::arginfo_class_Swoole_Coroutine_Http2_Client_ping, z::ZEND_ACC_PUBLIC;
        c"close" => zim_swoole_http2_client_coro_close, &http2_client_coro::arginfo_class_Swoole_Coroutine_Http2_Client_close, z::ZEND_ACC_PUBLIC;
    ];
}

/// MINIT, as `php_swoole_http2_client_coro_minit` did.
pub fn client_minit(args: &swoole_rs_phpswoole::MinitArgs) {
    // phphttp2's one MINIT entry also hands its server to phphttp.
    swoole_rs_phphttp::http2::register(&crate::server::HOOKS);
    let minit = args.minit;
    if let Err(error) = register(minit, args.shortname) {
        error.defer();
        swoole_rs_zend::bailout::finish();
    }
}

fn register(minit: &Minit, shortname: bool) -> Result<(), swoole_rs_zend::bailout::Bailout> {
    minit.register_object::<Data>(c"Swoole\\Coroutine\\Http2\\Client", &METHODS.0, 0);
    if shortname {
        minit.alias(&CLIENT, c"Co\\Http2\\Client")?;
    }
    let exception = minit.parent(swoole_rs_phpswoole::exception());
    minit.register_child(
        &EXCEPTION,
        c"Swoole\\Coroutine\\Http2\\Client\\Exception",
        &[],
        exception,
        |_, _| {},
    );
    if shortname {
        minit.alias(&EXCEPTION, c"Co\\Http2\\Client\\Exception")?;
    }
    minit.register_object::<Plain<0>>(REQUEST_NAME, &NO_METHODS.0, 0);
    minit.register_object::<Plain<1>>(c"Swoole\\Http2\\Response", &NO_METHODS.0, 0);
    let public = ZEND_ACC_PUBLIC;
    minit.property_long(&CLIENT, c"errCode", 0, public);
    minit.property_string(&CLIENT, c"errMsg", c"", public);
    minit.property_long(&CLIENT, c"sock", -1, public);
    minit.property_long(&CLIENT, c"type", 0, public);
    minit.property_null(&CLIENT, c"setting", public);
    minit.property_null(&CLIENT, c"socket", public);
    minit.property_bool(&CLIENT, c"connected", false, public);
    minit.property_null(&CLIENT, c"host", public);
    minit.property_long(&CLIENT, c"port", 0, public);
    minit.property_bool(&CLIENT, c"ssl", false, public);
    minit.property_long(&CLIENT, c"serverLastStreamId", 0, public);
    minit.property_string(&REQUEST, c"path", c"/", public);
    minit.property_string(&REQUEST, c"method", c"GET", public);
    minit.property_null(&REQUEST, c"headers", public);
    minit.property_null(&REQUEST, c"cookies", public);
    minit.property_string(&REQUEST, c"data", c"", public);
    minit.property_bool(&REQUEST, c"pipeline", false, public);
    minit.property_bool(&REQUEST, c"usePipelineRead", false, public);
    for name in [c"streamId", c"errCode", c"statusCode"] {
        minit.property_long(&RESPONSE, name, 0, public);
    }
    minit.property_bool(&RESPONSE, c"pipeline", false, public);
    for name in [c"headers", c"set_cookie_headers", c"cookies", c"data"] {
        minit.property_null(&RESPONSE, name, public);
    }
    for (name, number) in CONSTANTS {
        minit.long_constant(name, number);
    }
    Ok(())
}

/// `$this`'s client, none before its constructor.
fn this(call: &Call) -> Option<&Client> {
    call.custom::<Data>()?.client.get()
}

/// `http2_client_coro_get_client`: the constructed client; a fatal error if not.
fn client(call: &Call) -> Option<&Client> {
    let Some(client) = this(call) else {
        swoole_rs_zend::bailout::defer(swoole_rs_phpswoole::swoole_fatal_error(
            swoole_rs_error::ErrorCode::SW_ERROR_WRONG_OPERATION as c_int,
            c"must call constructor first",
        ));
        return None;
    };
    client.reap();
    Some(client)
}

/// `__construct(string $host, int $port = 80, bool $ssl = false)`.
fn construct(call: &Call) {
    let parsed = call.arity(1, 3).and_then(|()| {
        Ok((
            call.string(1, b"")?,
            call.long(2, 80)?,
            call.bool(3, false)?,
        ))
    });
    let Ok((host, port, ssl)) = parsed else {
        return call.set_bool(false);
    };
    if this(call).is_some() {
        call.throw_constructed_twice();
        return call.set_bool(false);
    }
    if host.is_empty() {
        call.throw(&EXCEPTION, c"host is empty", INVALID_PARAMS);
        return call.set_bool(false);
    }
    let Some(object) = call.this_value().and_then(Weak::of) else {
        return call.set_bool(false);
    };
    let client = Client::new(object, host, port, ssl);
    if let Some(props) = call.props(Some(&CLIENT)) {
        props.bytes(c"host", host);
        props.long(c"port", port);
        props.bool(c"ssl", ssl);
    }
    if let Some(data) = call.custom::<Data>()
        && data.client.set(client).is_err()
    {
        call.throw_constructed_twice();
    }
}

fn destruct(_call: &Call) {}

/// `set(array $settings): bool`.
fn set(call: &Call) {
    let Some(client) = client(call) else { return };
    let Ok(settings) = call.arity(1, 1).and_then(|()| call.array(1)) else {
        return call.set_bool(false);
    };
    if let Some(props) = call.props(Some(&CLIENT)) {
        props.merge(c"setting", settings);
    }
    if let Some(socket) = client.socket() {
        apply(socket, settings);
    }
    call.set_bool(true);
}

fn connect(call: &Call) {
    let Some(client) = client(call) else { return };
    let result = client.connect();
    call.set_bool(result);
}

fn close(call: &Call) {
    let Some(client) = client(call) else { return };
    let _hold = client.hold_socket();
    let result = client.close();
    call.set_bool(result);
}

/// `Swoole\Http2\Request` arguments. Outside this module a request's
/// `headers` and `cookies` are readable only held ([`Request::array`]).
mod request {
    use std::ffi::CStr;
    use swoole_rs_zend::{HeldArray, Props, Value};

    /// A `Swoole\Http2\Request` argument, live for the call.
    #[derive(Clone, Copy)]
    pub(super) struct Request<'a>(&'a Value);

    /// The request properties read in place: none is a header array.
    #[derive(Clone, Copy)]
    pub(super) enum Field {
        Method,
        Path,
        Data,
        Pipeline,
        UsePipelineRead,
    }

    /// The request's header arrays, which [`Request::array`] reads only held.
    #[derive(Clone, Copy)]
    pub(super) enum Held {
        Headers,
        Cookies,
    }

    impl<'a> Request<'a> {
        /// `object` holds an instance of the request class.
        pub(super) fn new(object: &'a Value) -> Self {
            Self(object)
        }

        /// Property writes, under the request class.
        pub(super) fn props(self) -> Option<Props<'a>> {
            self.0.props(Some(&super::REQUEST))
        }

        /// The slot of a property the request class declares.
        fn slot(self, name: &CStr) -> Option<&'a Value> {
            self.0.property(name)
        }

        pub(super) fn property(self, field: Field) -> Option<&'a Value> {
            self.slot(match field {
                Field::Method => c"method",
                Field::Path => c"path",
                Field::Data => c"data",
                Field::Pipeline => c"pipeline",
                Field::UsePipelineRead => c"usePipelineRead",
            })
        }

        /// `headers` or `cookies` with a reference of its own: converting a
        /// value may replace or unset the property while
        /// [`crate::server::entries`] walks it.
        pub(super) fn array(self, which: Held) -> Option<HeldArray> {
            self.slot(match which {
                Held::Headers => c"headers",
                Held::Cookies => c"cookies",
            })?
            .hold()
        }
    }
}

/// The name `Swoole\Http2\Request` is registered under.
const REQUEST_NAME: &CStr = c"Swoole\\Http2\\Request";

/// `Z_PARAM_OBJECT_OF_CLASS(.., Swoole\Http2\Request)` of the first argument.
fn request_arg(call: &Call) -> Option<Request<'_>> {
    let arg = call.value(1)?;
    if arg.instance_of(&REQUEST) {
        return Some(Request::new(arg));
    }
    call.wrong_class(1, REQUEST_NAME, arg);
    None
}

/// `send(Swoole\Http2\Request $request): int|false`.
fn send(call: &Call) {
    let Some(client) = client(call) else { return };
    if !client.is_available() {
        return call.set_bool(false);
    }
    let Some(request) = call.arity(1, 1).ok().and_then(|()| request_arg(call)) else {
        return call.set_bool(false);
    };
    let _hold = client.hold_socket();
    match client.send_request(request) {
        0 => call.set_bool(false),
        id => call.set_long(i64::from(id)),
    }
}

/// `recv(float $timeout = 0)` and `read(float $timeout = 0)`.
fn receive(call: &Call, pipeline_read: bool) {
    let Some(client) = client(call) else { return };
    let Ok(timeout) = call.arity(0, 1).and_then(|()| call.double(1, 0.0)) else {
        return call.set_bool(false);
    };
    // Upstream preserves `$this` here, not the socket.
    let _hold = call.this_owned();
    client.recv(call, timeout, pipeline_read);
}

fn recv(call: &Call) {
    receive(call, false);
}

fn read(call: &Call) {
    receive(call, true);
}

/// `write(int $streamId, mixed $data, bool $end = false): bool`.
fn write(call: &Call) {
    let Some(client) = client(call) else { return };
    if !client.is_available() {
        return call.set_bool(false);
    }
    let parsed = call
        .arity(2, 3)
        .and_then(|()| Ok((call.long(1, 0)?, call.bool(3, false)?)));
    let Ok((id, end)) = parsed else {
        return call.set_bool(false);
    };
    let _hold = client.hold_socket();
    let result = client.write_data(id.c_cast::<u32>(), call.value(2), end);
    call.set_bool(result);
}

fn ping(call: &Call) {
    let Some(client) = client(call) else { return };
    if !client.is_available() {
        return call.set_bool(false);
    }
    let result = client.send_ping();
    call.set_bool(result);
}

/// `goaway(int $error_code = NO_ERROR, string $debug_data = ''): bool`.
fn goaway(call: &Call) {
    let Some(client) = client(call) else { return };
    if !client.is_available() {
        return call.set_bool(false);
    }
    let parsed = call
        .arity(0, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.string(2, b"")?)));
    let Ok((code, debug)) = parsed else {
        return call.set_bool(false);
    };
    let _hold = client.hold_socket();
    let result = client.send_goaway(code, debug);
    call.set_bool(result);
}

/// `isStreamExist(int $stream_id): bool`.
fn is_stream_exist(call: &Call) {
    let Ok(id) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        return call.set_bool(false);
    };
    if id < 0 {
        return call.set_bool(false);
    }
    let Some(client) = client(call) else { return };
    if client.socket().is_none() {
        return call.set_bool(false);
    }
    if id == 0 {
        return call.set_bool(true);
    }
    let exists = u32::try_from(id).is_ok_and(|id| client.streams.borrow().contains_key(&id));
    call.set_bool(exists);
}

fn settings_array(settings: &Http2Settings) -> Owned {
    let mut array = OwnedArray::default();
    for (name, number) in [
        (&b"header_table_size"[..], settings.header_table_size),
        (b"init_window_size", settings.init_window_size),
        (b"max_concurrent_streams", settings.max_concurrent_streams),
        (b"max_frame_size", settings.max_frame_size),
        (b"max_header_list_size", settings.max_header_list_size),
    ] {
        array.insert_long(name, i64::from(number));
    }
    array.into_owned()
}

/// `stats(string $key = ''): array|int`.
fn stats(call: &Call) {
    let Some(client) = client(call) else { return };
    let Ok(key) = call.arity(0, 1).and_then(|()| call.string(1, b"")) else {
        return call.set_bool(false);
    };
    let active = i64::try_from(client.streams.borrow().len()).unwrap_or(i64::MAX);
    let (stream_id, last_stream_id) = (client.stream_id.get(), client.last_stream_id.get());
    let remote = client.remote.get();
    if !key.is_empty() {
        match key {
            b"current_stream_id" => call.set_long(i64::from(stream_id)),
            b"last_stream_id" => call.set_long(i64::from(last_stream_id)),
            b"local_settings" => call.set_owned(settings_array(&client.local)),
            b"remote_settings" => call.set_owned(settings_array(&remote)),
            b"active_stream_num" => call.set_long(active),
            _ => {}
        }
        return;
    }
    let mut array = OwnedArray::default();
    array.insert_long(b"current_stream_id", i64::from(stream_id));
    array.insert_long(b"last_stream_id", i64::from(last_stream_id));
    array.insert(b"local_settings", settings_array(&client.local));
    array.insert(b"remote_settings", settings_array(&remote));
    array.insert_long(b"active_stream_num", active);
    call.set_owned(array.into_owned());
}

swoole_rs_zend::handlers! {
    zim_swoole_http2_client_coro___construct => construct;
    zim_swoole_http2_client_coro___destruct => destruct;
    zim_swoole_http2_client_coro_set => set;
    zim_swoole_http2_client_coro_connect => connect;
    zim_swoole_http2_client_coro_stats => stats;
    zim_swoole_http2_client_coro_isStreamExist => is_stream_exist;
    zim_swoole_http2_client_coro_send => send;
    zim_swoole_http2_client_coro_write => write;
    zim_swoole_http2_client_coro_recv => recv;
    zim_swoole_http2_client_coro_read => read;
    zim_swoole_http2_client_coro_ping => ping;
    zim_swoole_http2_client_coro_goaway => goaway;
    zim_swoole_http2_client_coro_close => close;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_lowercase_names_into_one_arena() {
        let mut set = Headers::default();
        set.add(b":method", &[b"GET"]);
        set.slots.push(None);
        set.add(b"X-Token", &[b"a", b"=", b"b"]);
        set.set(1, b":authority", b"host:8080");
        let list: Vec<_> = set.list().collect();
        assert_eq!(
            list,
            [
                (&b":method"[..], &b"GET"[..]),
                (b":authority", b"host:8080"),
                (b"x-token", b"a=b"),
            ],
            "the reserved slot set, names lowercased, values joined"
        );
        set.slots.push(None);
        assert_eq!(
            set.list().last(),
            Some((&b""[..], &b""[..])),
            "an unset slot"
        );
    }

    #[test]
    fn stream_ids_hash_apart() {
        let mut streams = Streams::<u32>::default();
        for id in (1..200).step_by(2) {
            streams.insert(id, id);
        }
        assert_eq!(streams.len(), 100, "distinct ids");
        assert!(streams.contains_key(&199), "lookup");
    }
}
