//! The HTTP/2 coroutine client: upstream's `swoole::coroutine::http2::Client`
//! and its PHP classes.
use crate::native::{self, Header, Socket};
use libc::{c_char, c_int, c_void};
use std::{
    collections::{HashMap, VecDeque},
    ffi::CStr,
    hash::{BuildHasherDefault, Hasher},
    mem::MaybeUninit,
    ptr,
};
use swoole_rs_http2::{
    Http2ChunkPlan, Http2FrameFacts, Http2HeaderPlan, Http2InflateStep, Http2ResponsePlan,
    Http2SettingPlan, Http2Settings, Http2WindowPlan, sw_rs_http2_chunk, sw_rs_http2_decode_frame,
    sw_rs_http2_default_settings, sw_rs_http2_header_plan, sw_rs_http2_headers_flags,
    sw_rs_http2_inflate_step, sw_rs_http2_media_kind, sw_rs_http2_pack_frame_header,
    sw_rs_http2_pack_settings, sw_rs_http2_queue_action, sw_rs_http2_response,
    sw_rs_http2_send_complete, sw_rs_http2_setting_plan, sw_rs_http2_stream_flags,
    sw_rs_http2_stream_update, sw_rs_http2_window_plan, sw_rs_http2_write_allowed,
};
use swoole_rs_inflate::Inflate;
use swoole_rs_phptable::{self as table, Owned};
use swoole_rs_zend::{
    Call, Class, Object, declare_bool, declare_long, declare_string, register_long_constant,
    set_object, string,
    sys::{
        _zval_struct__bindgen_ty_1, IS_NULL, IS_OBJECT, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC,
        add_assoc_long_ex, add_assoc_stringl_ex, add_assoc_zval_ex, object_init_ex,
        swr_zend_http_build_query, swr_zend_instanceof, swr_zend_property_merge,
        swr_zval_try_addref, zend_class_entry, zend_function_entry, zend_object,
        zend_object_handlers, zend_wrong_parameter_class_error, zval, zval_ptr_dtor,
    },
    throw_constructed_twice, throw_exception, type_of, unset_property_deny, value,
};

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
/// `SwRsHttp2FrameAction`.
const ACTION_SETTINGS: u32 = 0;
const ACTION_HEADERS: u32 = 1;
const ACTION_DATA: u32 = 2;
const ACTION_PING_REPLY: u32 = 3;
const ACTION_WINDOW_UPDATE: u32 = 4;
const ACTION_RESET: u32 = 5;
const ACTION_GOAWAY: u32 = 6;
const ACTION_SETTINGS_ACK: u32 = 8;
const ACTION_PING_ACK: u32 = 9;
const ACTION_IGNORE_PUSH: u32 = 10;
/// `SwRsHttp2HeaderRole` and `SwRsHttp2HeaderKind`.
const RESPONSE_HEADERS: u32 = 1;
const CLIENT_HEADERS: u32 = 3;
const HEADER_STATUS: u32 = 8;
const HEADER_CONTENT_ENCODING: u32 = 9;
const HEADER_SET_COOKIE: u32 = 10;
const HEADER_HOST: u32 = 13;
const MEDIA_GZIP: u32 = 3;
/// `SW_HTTP2_SETTING_HEADER_TABLE_SIZE` and `..._MAX_HEADER_LIST_SIZE`.
const SETTING_HEADER_TABLE_SIZE: u16 = 1;
const SETTING_MAX_HEADER_LIST_SIZE: u16 = 6;
/// `NGHTTP2_HD_INFLATE_EMIT`.
const INFLATE_EMIT: c_int = 2;
/// `NGHTTP2_NV_FLAG_NO_COPY_NAME | NGHTTP2_NV_FLAG_NO_COPY_VALUE`.
const NV_NO_COPY: u8 = 0x02 | 0x04;
/// The `:authority` slot (`HTTP2_CLIENT_HOST_HEADER_INDEX`).
const AUTHORITY: usize = 3;
/// `SW_ERROR_*` and `HTTP_ESTATUS_SERVER_RESET`.
const INVALID_PARAMS: i64 = 505;
const QUEUE_FULL: c_int = 506;
const NO_CONNECTION: c_int = 5001;
const SEND_CONTROL_FRAME_FAILED: i64 = 3005;
const SERVER_RESET: i64 = -3;

static CLIENT: Class = Class::new();
static EXCEPTION: Class = Class::new();
static REQUEST: Class = Class::new();
static RESPONSE: Class = Class::new();

/// `struct Http2ClientObject { Client *client; zend_object std; }`.
#[repr(C)]
pub(crate) struct Data {
    client: *mut Client,
}

type ClientObject = Object<Data>;
type PlainObject = Object<()>;

/// What the socket's close hook changes, allocated apart from the client.
/// The hook drops the socket; the client releases what hangs off it
/// (streams, queue, HPACK) the next time it looks (`reap`).
struct Conn {
    object: *mut zend_object,
    socket: Socket,
    zsocket: zval,
    closed: bool,
}

struct Stream {
    flags: u32,
    buffer: Vec<u8>,
    gzip: Option<Inflate>,
    response: zval,
    remote_window: u32,
    local_window: u32,
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: the stream's own response reference.
        unsafe { zval_ptr_dtor(&raw mut self.response) };
    }
}

/// `ReturnCode` of `parse_frame`.
enum Frame {
    Continue,
    Ready,
    Fail,
}

struct Client {
    object: *mut zend_object,
    conn: *mut Conn,
    host: Vec<u8>,
    port: i64,
    ssl: bool,
    stream_id: u32,
    last_stream_id: u32,
    local: Http2Settings,
    remote: Http2Settings,
    remote_window: u32,
    local_window: u32,
    streams: Streams,
    headers: Headers,
    queue: VecDeque<Vec<u8>>,
    inflater: *mut c_void,
    deflater: *mut c_void,
}

fn strerror<'a>(code: c_int) -> &'a CStr {
    // SAFETY: a pure query; the text stays valid until the next call.
    let text = unsafe { libc::strerror(code) };
    // SAFETY: strerror returns a NUL-terminated string.
    unsafe { CStr::from_ptr(text) }
}

fn ce() -> *mut zend_class_entry {
    CLIENT.entry()
}

fn set_long(object: *mut zend_object, ce: *mut zend_class_entry, name: &CStr, number: i64) {
    // SAFETY: a live object of `ce`, which declares `name`.
    unsafe { value::update_long(ce, object, name, number) };
}

fn set_bytes(object: *mut zend_object, ce: *mut zend_class_entry, name: &CStr, data: &[u8]) {
    // SAFETY: as above.
    unsafe { table::update_bytes(ce, object, name, data) };
}

/// `php_swoole_socket_set_error_properties(zobject, code, message)`.
fn error_properties(object: *mut zend_object, code: c_int, message: &[u8]) {
    // SAFETY: a plain store into Swoole's thread error.
    unsafe { native::swoole_http2_client_coro_last_error(code) };
    set_long(object, ce(), c"errCode", i64::from(code));
    set_bytes(object, ce(), c"errMsg", message);
}

fn frame_header(kind: u32, length: usize, flags: u32, stream: u32) -> [u8; FRAME_HEADER] {
    let mut out = [0; FRAME_HEADER];
    let length = u32::try_from(length).unwrap_or(u32::MAX);
    // SAFETY: a nine-byte buffer.
    unsafe { sw_rs_http2_pack_frame_header(out.as_mut_ptr(), kind, length, flags, stream) };
    out
}

fn window(current: u32, amount: usize, initial: u32, credit: bool) -> Http2WindowPlan {
    let mut plan = Http2WindowPlan::default();
    // SAFETY: a writable plan.
    unsafe { sw_rs_http2_window_plan(current, amount, initial, u32::from(credit), &raw mut plan) };
    plan
}

/// The HPACK header list upstream's `HeaderSet` builds (names lowercased),
/// in one byte arena the client reuses across requests.
#[derive(Default)]
struct Headers {
    bytes: Vec<u8>,
    /// Each slot's name and value as (start, name length, value length).
    slots: Vec<Option<(usize, usize, usize)>>,
    list: Vec<Header>,
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
        self.slots[index] = Some(self.store(name, &[value]));
    }

    /// The `nghttp2_nv` list over the arena, valid until the next change.
    fn list(&mut self) -> &[Header] {
        let base = self.bytes.as_ptr();
        self.list.clear();
        self.list.extend(self.slots.iter().map(|slot| match *slot {
            Some((start, name, value)) => Header {
                name: base.wrapping_add(start),
                value: base.wrapping_add(start + name),
                name_length: name,
                value_length: value,
                flags: NV_NO_COPY,
            },
            None => Header {
                name: ptr::null(),
                value: ptr::null(),
                name_length: 0,
                value_length: 0,
                flags: 0,
            },
        }));
        &self.list
    }
}

/// Stream ids are small and spread by the server's odd/even rule: hash them
/// by a multiply instead of `SipHash`.
#[derive(Default)]
struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 << 8 | u64::from(byte)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        }
    }

    fn write_u32(&mut self, id: u32) {
        self.0 = u64::from(id).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}

type Streams<T = Stream> = HashMap<u32, T, BuildHasherDefault<IdHasher>>;

/// The socket's close hook.
/// # Safety
/// `conn` is the live `Conn` the client registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp2_client_socket_dtor(conn: *mut c_void) {
    let conn = conn.cast::<Conn>();
    // SAFETY: per the caller; no `&mut` to the conn is live across the hook.
    unsafe { (*conn).socket = ptr::null_mut() };
    // SAFETY: as above.
    unsafe { (*conn).closed = true };
    // SAFETY: as above.
    let object = unsafe { (*conn).object };
    // SAFETY: a live client object.
    unsafe { value::update_bool(ce(), object, c"connected", false) };
    // SAFETY: as above.
    unsafe { table::update_null(ce(), object, c"socket") };
    // SAFETY: as above.
    let zsocket = unsafe { &raw mut (*conn).zsocket };
    // SAFETY: the conn's reference to the socket object.
    unsafe { zval_ptr_dtor(zsocket) };
    // SAFETY: as above.
    unsafe { (*zsocket).u1 = _zval_struct__bindgen_ty_1 { type_info: IS_NULL } };
}

impl Client {
    fn new(object: *mut zend_object, host: &[u8], port: i64, ssl: bool) -> Box<Self> {
        let mut zsocket = MaybeUninit::<zval>::zeroed();
        // SAFETY: zeroed storage, typed null.
        unsafe { (*zsocket.as_mut_ptr()).u1 = _zval_struct__bindgen_ty_1 { type_info: IS_NULL } };
        let conn = Box::into_raw(Box::new(Conn {
            object,
            socket: ptr::null_mut(),
            // SAFETY: initialised above.
            zsocket: unsafe { zsocket.assume_init() },
            closed: false,
        }));
        let local = sw_rs_http2_default_settings();
        Box::new(Self {
            object,
            conn,
            host: host.to_vec(),
            port,
            ssl,
            stream_id: 0,
            last_stream_id: 0,
            local,
            remote: Http2Settings::default(),
            remote_window: 0,
            local_window: local.init_window_size,
            streams: Streams::default(),
            headers: Headers::default(),
            queue: VecDeque::new(),
            inflater: ptr::null_mut(),
            deflater: ptr::null_mut(),
        })
    }

    /// The live socket, read fresh: the close hook may have cleared it.
    fn socket(&self) -> Socket {
        // SAFETY: the conn lives as long as the client.
        unsafe { (*self.conn).socket }
    }

    fn zsocket(&self) -> *mut zval {
        // SAFETY: as above.
        unsafe { &raw mut (*self.conn).zsocket }
    }

    /// `socket_dtor`'s release of what hung off the closed socket.
    fn reap(&mut self) {
        // SAFETY: as above.
        if !std::mem::take(unsafe { &mut (*self.conn).closed }) {
            return;
        }
        self.queue.clear();
        self.streams.clear();
        // SAFETY: the client's HPACK codecs, freed once.
        unsafe { native::swoole_http2_client_coro_hpack_free(self.inflater, self.deflater) };
        self.inflater = ptr::null_mut();
        self.deflater = ptr::null_mut();
    }

    fn update_error(&self, code: c_int, message: &[u8]) {
        error_properties(self.object, code, message);
    }

    /// `io_error`: the socket's errCode and errMsg.
    fn io_error(&self, socket: Socket) {
        let mut message: *const c_char = ptr::null();
        // SAFETY: a live socket and a writable local.
        let code = unsafe { native::swoole_http2_client_coro_error(socket, &raw mut message) };
        let message = if message.is_null() {
            &b""[..]
        } else {
            // SAFETY: the socket's NUL-terminated errMsg.
            unsafe { CStr::from_ptr(message) }.to_bytes()
        };
        self.update_error(code, message);
    }

    fn nghttp2_error(&self, code: c_int, message: &str) {
        // SAFETY: nghttp2's static error text.
        let reason = unsafe { native::swoole_http2_client_coro_strerror(code) };
        // SAFETY: as above.
        let reason = unsafe { CStr::from_ptr(reason) };
        let text = format!("{message} with error: {}", reason.to_string_lossy());
        self.update_error(code, text.as_bytes());
    }

    fn is_available(&self) -> bool {
        let socket = self.socket();
        // SAFETY: a live socket.
        if socket.is_null() || !unsafe { native::swoole_http2_client_coro_is_connected(socket) } {
            // SAFETY: Swoole's static message for a known code.
            let message =
                unsafe { native::swoole_http2_client_coro_swoole_strerror(NO_CONNECTION) };
            // SAFETY: as above.
            let message = unsafe { CStr::from_ptr(message) };
            self.update_error(NO_CONNECTION, message.to_bytes());
            return false;
        }
        true
    }

    fn connect(&mut self) -> bool {
        self.reap();
        if !self.socket().is_null() {
            self.update_error(libc::EISCONN, strerror(libc::EISCONN).to_bytes());
            return false;
        }
        let (mut converted, mut socket, mut error) = (ptr::null_mut(), ptr::null_mut(), 0);
        // SAFETY: the host bytes and writable locals.
        let object = unsafe {
            native::swoole_http2_client_coro_create_socket(
                self.host.as_ptr().cast(),
                self.host.len(),
                &raw mut converted,
                &raw mut socket,
                &raw mut error,
            )
        };
        if !converted.is_null() {
            // SAFETY: the new string's reference.
            self.host = unsafe { Owned::from_raw(converted) }.bytes().to_vec();
        }
        if object.is_null() {
            self.update_error(error, strerror(error).to_bytes());
            return false;
        }
        // SAFETY: the conn takes the new object's reference.
        unsafe { set_object(self.zsocket(), object) };
        // SAFETY: the conn lives as long as the client.
        unsafe { (*self.conn).socket = socket };
        // SAFETY: a live socket and the client's conn.
        unsafe { native::swoole_http2_client_coro_setup(socket, self.conn.cast()) };
        // SAFETY: a live socket.
        if self.ssl && !unsafe { native::swoole_http2_client_coro_enable_ssl(socket) } {
            return self.connect_failed(socket, None);
        }
        // SAFETY: `$this` declares `setting`.
        let settings = unsafe { table::property(self.object, c"setting") };
        // SAFETY: a live socket and property.
        unsafe { native::swoole_http2_client_coro_protocol(socket, settings) };
        // SAFETY: a live socket and the host bytes.
        let connected = unsafe {
            native::swoole_http2_client_coro_connect(
                socket,
                self.host.as_ptr().cast(),
                self.host.len(),
                c_int::try_from(self.port).unwrap_or(-1),
            )
        };
        if !connected {
            return self.connect_failed(socket, None);
        }
        self.stream_id = 1;
        self.remote = sw_rs_http2_default_settings();
        self.remote_window = self.remote.init_window_size;
        let mut code = 0;
        // SAFETY: a writable local.
        self.inflater = unsafe { native::swoole_http2_client_coro_inflater(&raw mut code) };
        if code != 0 {
            return self.connect_failed(socket, Some((code, "nghttp2_hd_inflate_new2() failed")));
        }
        // SAFETY: as above.
        self.deflater = unsafe {
            native::swoole_http2_client_coro_deflater(self.local.header_table_size, &raw mut code)
        };
        if code != 0 {
            return self.connect_failed(socket, Some((code, "nghttp2_hd_deflate_new2() failed")));
        }
        if !self.send(PREFACE) || !self.send_setting() {
            self.close();
            return false;
        }
        // SAFETY: a live client object.
        let object_ce = unsafe { (*self.object).ce };
        // SAFETY: as above.
        unsafe { table::update(object_ce, self.object, c"socket", self.zsocket()) };
        // SAFETY: as above.
        unsafe { value::update_bool(ce(), self.object, c"connected", true) };
        true
    }

    fn connect_failed(&mut self, socket: Socket, nghttp2: Option<(c_int, &str)>) -> bool {
        match nghttp2 {
            Some((code, message)) => self.nghttp2_error(code, message),
            None => self.io_error(socket),
        }
        self.close();
        false
    }

    fn close(&mut self) -> bool {
        let socket = self.socket();
        if socket.is_null() {
            self.update_error(libc::EBADF, strerror(libc::EBADF).to_bytes());
            return false;
        }
        // SAFETY: a live client object.
        let object_ce = unsafe { (*self.object).ce };
        // SAFETY: as above.
        unsafe { value::update_bool(object_ce, self.object, c"connected", false) };
        // SAFETY: a live socket; its members are not read after a close.
        if !unsafe { native::swoole_http2_client_coro_close(socket) } {
            self.io_error(socket);
            return false;
        }
        self.reap();
        true
    }

    /// `send`: directly, or queued behind a coroutine writing already.
    fn send(&mut self, data: &[u8]) -> bool {
        let socket = self.socket();
        // SAFETY: a live socket.
        let bound = unsafe { native::swoole_http2_client_coro_write_bound(socket) };
        let (queued, limit) = if bound {
            (self.queue.len(), self.remote.max_concurrent_streams)
        } else {
            (0, 0)
        };
        match sw_rs_http2_queue_action(u32::from(bound), queued, limit) {
            0 => {}
            2 => {
                // SAFETY: a live socket; the message is static.
                unsafe {
                    native::swoole_http2_client_coro_set_error(
                        socket,
                        QUEUE_FULL,
                        c"the send queue is full, try again later".as_ptr(),
                    );
                }
                self.io_error(socket);
                return false;
            }
            _ => {
                self.queue.push_back(data.to_vec());
                return true;
            }
        }
        if !Self::send_all(socket, data) {
            self.io_error(socket);
            return false;
        }
        while let Some(frame) = self.queue.front() {
            let frame = frame.clone();
            if !Self::send_all(socket, &frame) {
                self.io_error(socket);
                // SAFETY: from a handler, with the registered exception class.
                unsafe {
                    throw_exception(
                        EXCEPTION.entry(),
                        c"failed to send control frame",
                        SEND_CONTROL_FRAME_FAILED,
                    );
                };
                return false;
            }
            self.queue.pop_front();
        }
        true
    }

    fn send_all(socket: Socket, data: &[u8]) -> bool {
        // SAFETY: a live socket and the bytes to send.
        let sent = unsafe {
            native::swoole_http2_client_coro_send_all(socket, data.as_ptr().cast(), data.len())
        };
        sw_rs_http2_send_complete(sent, data.len()) != 0
    }

    fn send_setting(&mut self) -> bool {
        let mut frame = [0_u8; SETTING_FRAME];
        // SAFETY: the frame buffer and the client's settings.
        let length = unsafe {
            sw_rs_http2_pack_settings(frame.as_mut_ptr(), frame.len(), &raw const self.local, 0)
        };
        self.send(&frame[..length.min(SETTING_FRAME)])
    }

    fn send_window_update(&mut self, stream: u32, size: u32) -> bool {
        let mut frame = frame_header(TYPE_WINDOW_UPDATE, WINDOW_UPDATE_SIZE, 0, stream).to_vec();
        frame.extend_from_slice(&size.to_be_bytes());
        self.send(&frame)
    }

    fn send_ping(&mut self) -> bool {
        let mut frame = frame_header(TYPE_PING, PING_PAYLOAD, 0, 0).to_vec();
        frame.extend_from_slice(&[0; PING_PAYLOAD]);
        self.send(&frame)
    }

    fn send_data(&mut self, stream: u32, mut data: &[u8], flag: u32) -> bool {
        while !data.is_empty() {
            let mut chunk = Http2ChunkPlan::default();
            // SAFETY: a writable plan.
            unsafe {
                sw_rs_http2_chunk(data.len(), self.local.max_frame_size, flag, &raw mut chunk);
            };
            let length = chunk.length.min(data.len());
            if !self.send(&frame_header(TYPE_DATA, length, chunk.flags, stream))
                || !self.send(&data[..length])
            {
                return false;
            }
            data = &data[length..];
        }
        true
    }

    fn send_goaway(&mut self, error_code: i64, debug: &[u8]) -> bool {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "upstream packs the code as frame flags and a uint32_t"
        )]
        let code = error_code as u32;
        let mut frame =
            frame_header(TYPE_GOAWAY, GOAWAY_SIZE + debug.len(), code & 0xff, 0).to_vec();
        frame.extend_from_slice(&self.last_stream_id.to_be_bytes());
        frame.extend_from_slice(&code.to_be_bytes());
        frame.extend_from_slice(debug);
        self.send(&frame)
    }

    /// `build_header`: a HEADERS frame whose first nine bytes are left for
    /// its header, then the request's HPACK block; None after the error.
    fn build_header(&mut self, request: *mut zend_object) -> Option<Vec<u8>> {
        // SAFETY: a live request object, which declares each property.
        let method = unsafe { table::property(request, c"method") };
        // SAFETY: as above.
        let path = unsafe { table::property(request, c"path") };
        // SAFETY: as above.
        let headers = unsafe { table::property(request, c"headers") };
        // SAFETY: as above.
        let cookies = unsafe { table::property(request, c"cookies") };
        let mut set = std::mem::take(&mut self.headers);
        set.clear();
        let text = |value: *mut zval| {
            // SAFETY: live property zvals.
            unsafe { table::string_bytes(value) }.filter(|text| !text.is_empty())
        };
        set.add(b":method", &[text(method).unwrap_or(b"GET")]);
        set.add(b":path", &[text(path).unwrap_or(b"/")]);
        set.add(b":scheme", &[if self.ssl { b"https" } else { b"http" }]);
        set.slots.push(None);
        let mut host = false;
        // SAFETY: as above.
        if unsafe { table::is_array(headers) } {
            // SAFETY: as above.
            for (key, value) in unsafe { table::entries(headers) } {
                // SAFETY: values in the array.
                let Some(key) = key
                    .filter(|key| key.first() != Some(&b':') && !unsafe { table::is_null(value) })
                else {
                    continue;
                };
                // SAFETY: as above.
                let value = unsafe { Owned::of(value) };
                if header_kind(CLIENT_HEADERS, key) == HEADER_HOST {
                    set.set(AUTHORITY, b":authority", value.bytes());
                    host = true;
                } else {
                    set.add(key, &[value.bytes()]);
                }
            }
        }
        if !host {
            let authority = if self.port == if self.ssl { 443 } else { 80 } {
                self.host.clone()
            } else {
                let end = self
                    .host
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(self.host.len());
                [&self.host[..end], format!(":{}", self.port).as_bytes()].concat()
            };
            set.set(AUTHORITY, b":authority", &authority);
        }
        // SAFETY: as above.
        if unsafe { table::is_array(cookies) } {
            // SAFETY: as above.
            for (key, value) in unsafe { table::entries(cookies) } {
                // SAFETY: values in the array.
                let Some(key) = key.filter(|_| !unsafe { table::is_null(value) }) else {
                    continue;
                };
                // SAFETY: as above.
                let value = unsafe { Owned::of(value) };
                // SAFETY: the value's bytes; Zend returns a new string.
                let encoded = unsafe {
                    swoole_rs_zend::sys::php_url_encode(
                        value.bytes().as_ptr().cast(),
                        value.bytes().len(),
                    )
                };
                // SAFETY: the reference php_url_encode returned.
                let encoded = unsafe { Owned::from_raw(encoded) };
                set.add(b"cookie", &[key, b"=", encoded.bytes()]);
            }
        }
        let list = set.list();
        let (headers, count) = (list.as_ptr(), list.len());
        // SAFETY: the codec and a list of live headers.
        let bound = unsafe {
            native::swoole_http2_client_coro_deflate_bound(self.deflater, headers, count)
        };
        let mut frame = vec![0_u8; FRAME_HEADER + bound];
        // SAFETY: as above, into the frame's `bound` bytes after its header.
        let written = unsafe {
            native::swoole_http2_client_coro_deflate(
                self.deflater,
                frame.as_mut_ptr().wrapping_add(FRAME_HEADER),
                bound,
                headers,
                count,
            )
        };
        self.headers = set;
        if written < 0 {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "nghttp2 error codes are small ints"
            )]
            self.nghttp2_error(written as c_int, "nghttp2_hd_deflate_hd() failed");
            return None;
        }
        frame.truncate(FRAME_HEADER + written.unsigned_abs());
        Some(frame)
    }

    /// `send_request`: the new stream's id, or 0.
    fn send_request(&mut self, request: *mut zend_object) -> u32 {
        // SAFETY: a live request object, which declares each property.
        let headers = unsafe { table::property_array(REQUEST.entry(), request, c"headers") };
        // SAFETY: as above.
        let data = unsafe { table::property(request, c"data") };
        // SAFETY: as above.
        let pipeline = unsafe { table::property(request, c"pipeline") };
        // SAFETY: a live property.
        let pipeline = unsafe { table::truthy(pipeline) };
        // SAFETY: as above.
        let use_pipeline_read = unsafe { table::property(request, c"usePipelineRead") };
        // SAFETY: a live property.
        let use_pipeline_read = unsafe { table::truthy(use_pipeline_read) };
        // SAFETY: a live zval.
        let empty = match unsafe { table::string_bytes(data) } {
            Some(text) => text.is_empty(),
            // SAFETY: as above.
            None => !unsafe { table::truthy(data) },
        };
        // SAFETY: as above.
        if unsafe { table::is_array(data) } {
            let name = c"content-type";
            let kind = b"application/x-www-form-urlencoded";
            // SAFETY: a live array property; Zend copies both.
            unsafe {
                add_assoc_stringl_ex(
                    headers,
                    name.as_ptr(),
                    name.count_bytes(),
                    kind.as_ptr().cast(),
                    kind.len(),
                );
            };
        }
        let Some(mut frame) = self
            .build_header(request)
            .filter(|frame| frame.len() > FRAME_HEADER)
        else {
            return 0;
        };
        let flags = sw_rs_http2_stream_flags(u32::from(pipeline), u32::from(use_pipeline_read));
        let id = self.stream_id;
        self.create_stream(id, flags);
        let header_flags = sw_rs_http2_headers_flags(u32::from(empty), 1, flags);
        let header = frame_header(TYPE_HEADERS, frame.len() - FRAME_HEADER, header_flags, id);
        frame[..FRAME_HEADER].copy_from_slice(&header);
        if !self.send(&frame) {
            return 0;
        }
        if !empty {
            let flag = if flags & STREAM_PIPELINE_REQUEST != 0 {
                0
            } else {
                FLAG_END_STREAM
            };
            let Some(body) = body_bytes(data) else {
                return 0;
            };
            if !self.send_data(id, body.bytes(), flag) {
                return 0;
            }
        }
        self.stream_id += 2;
        id
    }

    fn create_stream(&mut self, id: u32, flags: u32) {
        let mut response = MaybeUninit::<zval>::uninit();
        // SAFETY: a writable zval; the response class is registered.
        unsafe { object_init_ex(response.as_mut_ptr(), RESPONSE.entry()) };
        // SAFETY: initialised above.
        let response = unsafe { response.assume_init() };
        // SAFETY: an object zval.
        set_long(
            unsafe { value::obj_of(&raw const response) },
            RESPONSE.entry(),
            c"streamId",
            i64::from(id),
        );
        self.streams.insert(
            id,
            Stream {
                flags,
                buffer: Vec::new(),
                gzip: None,
                response,
                remote_window: self.remote.init_window_size,
                local_window: self.local.init_window_size,
            },
        );
    }

    /// `write_data`: more of a pipeline request's body.
    fn write_data(&mut self, id: u32, data: *mut zval, end: bool) -> bool {
        let flags = self.streams.get(&id).map(|stream| stream.flags);
        if sw_rs_http2_write_allowed(u32::from(flags.is_some()), flags.unwrap_or(0)) == 0 {
            let message = format!("unable to found active pipeline stream#{id}");
            self.update_error(libc::EINVAL, message.as_bytes());
            return false;
        }
        let flag = if end { FLAG_END_STREAM } else { 0 };
        let Some(body) = body_bytes(data) else {
            return false;
        };
        if !self.send(&frame_header(TYPE_DATA, body.bytes().len(), flag, id))
            || !self.send(body.bytes())
        {
            return false;
        }
        if let Some(stream) = self.streams.get_mut(&id) {
            stream.flags = sw_rs_http2_stream_update(stream.flags, u32::from(end), 0);
        }
        true
    }

    /// `recv_packet` then `parse_frame`, until a response is ready.
    fn recv(&mut self, call: &Call, timeout: f64, pipeline_read: bool) {
        loop {
            self.reap();
            if !self.is_available() {
                return call.set_bool(false);
            }
            let socket = self.socket();
            // SAFETY: a live socket.
            if unsafe { native::swoole_http2_client_coro_recv_packet(socket, timeout) } <= 0 {
                self.io_error(socket);
                return call.set_bool(false);
            }
            match self.parse_frame(call, socket, pipeline_read) {
                Frame::Continue => {}
                Frame::Ready => return,
                Frame::Fail => {
                    self.reap();
                    return call.set_bool(false);
                }
            }
        }
    }

    fn parse_frame(&mut self, call: &Call, socket: Socket, pipeline_read: bool) -> Frame {
        // SAFETY: a live socket whose read buffer holds one whole frame.
        let buffer = unsafe { native::swoole_http2_client_coro_read_buffer(socket) }.cast::<u8>();
        let mut facts = Http2FrameFacts::default();
        // SAFETY: the frame header and a writable output.
        unsafe {
            sw_rs_http2_decode_frame(buffer, FRAME_HEADER, self.last_stream_id, 1, &raw mut facts)
        };
        let length = facts.length as usize;
        // SAFETY: the packet holds the header and `length` payload bytes.
        let payload =
            unsafe { std::slice::from_raw_parts(buffer.wrapping_add(FRAME_HEADER), length) };
        let id = facts.stream_id;
        if facts.publish_last_id != 0 {
            self.last_stream_id = id;
        }
        match facts.action {
            ACTION_SETTINGS_ACK | ACTION_PING_ACK | ACTION_IGNORE_PUSH => return Frame::Continue,
            ACTION_SETTINGS => {
                for setting in payload.chunks(6) {
                    let mut raw = [0_u8; 6];
                    raw[..setting.len()].copy_from_slice(setting);
                    let key = u16::from_be_bytes([raw[0], raw[1]]);
                    let number = u32::from_be_bytes([raw[2], raw[3], raw[4], raw[5]]);
                    if !self.apply_remote_setting(key, number) {
                        return Frame::Fail;
                    }
                }
                if !self.send(&frame_header(TYPE_SETTINGS, 0, FLAG_ACK, id)) {
                    return Frame::Fail;
                }
                return Frame::Continue;
            }
            ACTION_WINDOW_UPDATE => {
                let credit = read_u32(payload);
                if id == 0 {
                    self.remote_window = window(self.remote_window, credit as usize, 0, true).value;
                } else if let Some(stream) = self.streams.get_mut(&id) {
                    stream.remote_window =
                        window(stream.remote_window, credit as usize, 0, true).value;
                }
                return Frame::Continue;
            }
            ACTION_PING_REPLY => {
                let mut frame = frame_header(TYPE_PING, PING_PAYLOAD, FLAG_ACK, id).to_vec();
                // Upstream copies the 8 bytes after the payload's own 9 (its
                // pointer was already past the header).
                // SAFETY: the read buffer is at least a frame header plus
                // `length` bytes; upstream reads the same span.
                let echo = unsafe {
                    std::slice::from_raw_parts(buffer.wrapping_add(FRAME_HEADER * 2), PING_PAYLOAD)
                };
                frame.extend_from_slice(echo);
                return if self.send(&frame) {
                    Frame::Continue
                } else {
                    Frame::Fail
                };
            }
            ACTION_GOAWAY => {
                let server_last = read_u32(payload);
                let code = read_u32(&payload[4.min(length)..]);
                let debug = &payload[GOAWAY_SIZE.min(length)..];
                set_long(self.object, ce(), c"errCode", i64::from(code));
                set_bytes(self.object, ce(), c"errMsg", debug);
                set_long(
                    self.object,
                    ce(),
                    c"serverLastStreamId",
                    i64::from(server_last),
                );
                self.close();
                return Frame::Fail;
            }
            ACTION_RESET => {
                self.streams.remove(&id);
                return Frame::Continue;
            }
            _ => {}
        }
        if !self.streams.contains_key(&id) {
            // SAFETY: a notice with the stream id.
            unsafe { native::swoole_http2_client_coro_log(0, id) };
            return Frame::Continue;
        }
        if facts.action == ACTION_HEADERS {
            self.parse_header(id, facts.flags, payload);
        } else if facts.action == ACTION_DATA
            && let Err(frame) = self.receive_data(id, facts.flags, payload)
        {
            return frame;
        }
        let Some(stream) = self.streams.get_mut(&id) else {
            return Frame::Continue;
        };
        let mut plan = Http2ResponsePlan::default();
        // SAFETY: a writable plan.
        unsafe {
            sw_rs_http2_response(
                facts.kind,
                facts.flags,
                stream.flags,
                u32::from(pipeline_read),
                &raw mut plan,
            );
        };
        if plan.ready == 0 {
            return Frame::Continue;
        }
        let end = plan.end != 0;
        // SAFETY: the stream's response object.
        let response = unsafe { value::obj_of(&raw const stream.response) };
        if facts.kind == TYPE_RST_STREAM {
            // SAFETY: a live response object.
            let response_ce = unsafe { (*response).ce };
            set_long(response, response_ce, c"statusCode", SERVER_RESET);
            // A reset deletes its stream above, so upstream's stream_error is 0 here.
            set_long(response, response_ce, c"errCode", 0);
        }
        if !stream.buffer.is_empty() {
            set_bytes(response, RESPONSE.entry(), c"data", &stream.buffer);
            stream.buffer.clear();
        }
        if !end {
            // SAFETY: a live response object.
            unsafe { value::update_bool(RESPONSE.entry(), response, c"pipeline", true) };
        }
        call.set_value(stream.response);
        if end {
            // The return value takes the stream's reference.
            stream.response.u1 = _zval_struct__bindgen_ty_1 { type_info: IS_NULL };
            self.streams.remove(&id);
        } else {
            // SAFETY: the return value shares the response.
            unsafe { swr_zval_try_addref(call.return_value()) };
            // SAFETY: as above, then the stream's own reference is released.
            unsafe { zval_ptr_dtor(&raw mut stream.response) };
            let mut fresh = MaybeUninit::<zval>::uninit();
            // SAFETY: a writable zval; the response class is registered.
            unsafe { object_init_ex(fresh.as_mut_ptr(), RESPONSE.entry()) };
            // SAFETY: initialised above.
            stream.response = unsafe { fresh.assume_init() };
            // SAFETY: an object zval.
            let fresh = unsafe { value::obj_of(&raw const stream.response) };
            set_long(fresh, RESPONSE.entry(), c"streamId", i64::from(id));
        }
        Frame::Ready
    }

    fn apply_remote_setting(&mut self, key: u16, number: u32) -> bool {
        let old = match key {
            SETTING_HEADER_TABLE_SIZE => self.remote.header_table_size,
            SETTING_MAX_HEADER_LIST_SIZE => self.remote.max_header_list_size,
            _ => 0,
        };
        let mut plan = Http2SettingPlan::default();
        // SAFETY: a writable plan.
        unsafe { sw_rs_http2_setting_plan(u32::from(key), number, old, 0, 0, &raw mut plan) };
        if plan.set_value != 0 {
            let remote = &mut self.remote;
            let field = match plan.field {
                0 => &mut remote.header_table_size,
                1 => &mut remote.enable_push,
                2 => &mut remote.max_concurrent_streams,
                3 => &mut remote.init_window_size,
                4 => &mut remote.max_frame_size,
                _ => &mut remote.max_header_list_size,
            };
            *field = number;
        }
        if plan.set_window != 0 {
            self.remote_window = number;
        }
        if plan.change_deflater != 0 {
            // SAFETY: the client's deflater.
            let code =
                unsafe { native::swoole_http2_client_coro_deflate_table(self.deflater, number) };
            if code != 0 {
                self.nghttp2_error(code, "nghttp2_hd_deflate_change_table_size() failed");
                return false;
            }
        }
        true
    }

    /// A DATA frame: buffer (inflating gzip) and replenish the windows.
    fn receive_data(&mut self, id: u32, flags: u32, payload: &[u8]) -> Result<(), Frame> {
        let initial = self.local.init_window_size;
        let Some(stream) = self.streams.get_mut(&id) else {
            return Ok(());
        };
        stream.flags = sw_rs_http2_stream_update(stream.flags, flags & FLAG_END_STREAM, 1);
        if payload.is_empty() {
            return Ok(());
        }
        if let Some(gzip) = &mut stream.gzip {
            let Some(output) = gzip.decode(payload) else {
                // SAFETY: a warning.
                unsafe { native::swoole_http2_client_coro_log(1, id) };
                return Err(Frame::Fail);
            };
            stream.buffer.extend_from_slice(output);
        } else {
            stream.buffer.extend_from_slice(payload);
        }
        let connection = window(self.local_window, payload.len(), initial, false);
        self.local_window = connection.value;
        stream.local_window = window(stream.local_window, payload.len(), 0, false).value;
        let stream_window = window(stream.local_window, 0, initial, false);
        if connection.increment != 0 {
            if !self.send_window_update(0, connection.increment) {
                return Err(Frame::Fail);
            }
            self.local_window = initial;
        }
        if stream_window.increment != 0 {
            if !self.send_window_update(id, stream_window.increment) {
                return Err(Frame::Fail);
            }
            if let Some(stream) = self.streams.get_mut(&id) {
                stream.local_window = initial;
            }
        }
        Ok(())
    }

    /// `parse_header`: HPACK-decode a HEADERS block onto the stream's response.
    fn parse_header(&mut self, id: u32, flags: u32, mut input: &[u8]) {
        if flags & FLAG_PRIORITY != 0 {
            input = &input[5.min(input.len())..];
        }
        let Some(stream) = self.streams.get_mut(&id) else {
            return;
        };
        // SAFETY: the stream's response object.
        let response = unsafe { value::obj_of(&raw const stream.response) };
        let property = |name: &CStr| {
            // SAFETY: a live response object, which declares `name`.
            unsafe { table::property_array(RESPONSE.entry(), response, name) }
        };
        let (headers, cookies, set_cookie_headers) = (
            property(c"headers"),
            property(c"cookies"),
            property(c"set_cookie_headers"),
        );
        loop {
            let mut inflate_flags = 0;
            let mut header = Header {
                name: ptr::null(),
                name_length: 0,
                value: ptr::null(),
                value_length: 0,
                flags: 0,
            };
            // SAFETY: the codec, the block and writable locals.
            let read = unsafe {
                native::swoole_http2_client_coro_inflate(
                    self.inflater,
                    input.as_ptr(),
                    input.len(),
                    &raw mut inflate_flags,
                    &raw mut header,
                )
            };
            if read < 0 {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "nghttp2 error codes are small ints"
                )]
                self.nghttp2_error(read as c_int, "nghttp2_hd_inflate_hd failed");
                return;
            }
            input = &input[read.unsigned_abs().min(input.len())..];
            if inflate_flags & INFLATE_EMIT != 0 {
                // SAFETY: nghttp2's header, live until the next inflate call.
                let name = unsafe { std::slice::from_raw_parts(header.name, header.name_length) };
                // SAFETY: as above.
                let text = unsafe { std::slice::from_raw_parts(header.value, header.value_length) };
                let kind = header_kind(RESPONSE_HEADERS, name);
                if name.first() == Some(&b':') {
                    if kind == HEADER_STATUS {
                        // SAFETY: a live response object.
                        let response_ce = unsafe { (*response).ce };
                        set_long(response, response_ce, c"statusCode", atol(text));
                    }
                } else {
                    let Some(stream) = self.streams.get_mut(&id) else {
                        return;
                    };
                    if kind == HEADER_CONTENT_ENCODING && media_kind(text) == MEDIA_GZIP {
                        stream.gzip = Some(Inflate::gzip_only());
                    } else if kind == HEADER_SET_COOKIE {
                        // SAFETY: the value's bytes and two live array properties.
                        unsafe {
                            native::swoole_http2_client_coro_set_cookies(
                                header.value.cast(),
                                header.value_length,
                                cookies,
                                set_cookie_headers,
                            );
                        }
                    }
                    // SAFETY: a live array property; Zend copies both.
                    unsafe {
                        add_assoc_stringl_ex(
                            headers,
                            header.name.cast(),
                            header.name_length,
                            header.value.cast(),
                            header.value_length,
                        );
                    };
                }
            }
            let mut step = Http2InflateStep {
                end_headers: 0,
                finish: 0,
            };
            // SAFETY: a writable step.
            unsafe {
                sw_rs_http2_inflate_step(
                    u32::try_from(inflate_flags).unwrap_or(0),
                    input.len(),
                    1,
                    &raw mut step,
                );
            };
            if step.end_headers != 0 {
                // SAFETY: the client's inflater.
                unsafe { native::swoole_http2_client_coro_inflate_end(self.inflater) };
                return;
            }
            if step.finish != 0 {
                return;
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if !self.socket().is_null() {
            self.close();
        }
        self.reap();
        self.streams.clear();
        // SAFETY: the conn the client allocated; the socket is closed or gone.
        let conn = unsafe { Box::from_raw(self.conn) };
        let mut zsocket = conn.zsocket;
        // SAFETY: the conn's reference to its socket object, if any.
        unsafe { zval_ptr_dtor(&raw mut zsocket) };
        if !self.inflater.is_null() || !self.deflater.is_null() {
            // SAFETY: codecs still held (the socket closed without its hook).
            unsafe { native::swoole_http2_client_coro_hpack_free(self.inflater, self.deflater) };
        }
    }
}

/// `php_swoole_error(E_WARNING, "%s", message)`.
fn warn(message: &CStr) {
    // SAFETY: a NUL-terminated message, from a handler.
    unsafe { native::swoole_http2_client_coro_warning(message.as_ptr()) };
}

fn read_u32(bytes: &[u8]) -> u32 {
    let mut raw = [0_u8; 4];
    let n = bytes.len().min(4);
    raw[..n].copy_from_slice(&bytes[..n]);
    u32::from_be_bytes(raw)
}

/// `sw_atol`.
fn atol(text: &[u8]) -> i64 {
    let digits = text
        .iter()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(text.len());
    std::str::from_utf8(&text[..digits])
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(0)
}

/// `rust_fold_header`: at most 33 bytes, lowercased.
fn fold(bytes: &[u8]) -> ([u8; 33], usize) {
    let mut folded = [0_u8; 33];
    let n = bytes.len().min(folded.len());
    for (slot, byte) in folded.iter_mut().zip(&bytes[..n]) {
        *slot = byte.to_ascii_lowercase();
    }
    (folded, n)
}

fn header_kind(role: u32, name: &[u8]) -> u32 {
    let (folded, n) = fold(name);
    let mut plan = Http2HeaderPlan::default();
    // SAFETY: the folded bytes and a writable plan.
    unsafe { sw_rs_http2_header_plan(role, folded.as_ptr(), n, name.len(), 0, &raw mut plan) };
    plan.kind
}

fn media_kind(value: &[u8]) -> u32 {
    let (folded, n) = fold(value);
    // SAFETY: the folded bytes.
    unsafe { sw_rs_http2_media_kind(folded.as_ptr(), n) }
}

/// A request body: an array as a query string, else the value as a string;
/// None after `http_build_query failed`.
fn body_bytes(data: *mut zval) -> Option<Owned> {
    // SAFETY: a live zval.
    if unsafe { table::is_array(data) } {
        // SAFETY: an array zval.
        let query = unsafe { swr_zend_http_build_query(data) };
        if query.is_null() {
            warn(c"http_build_query failed");
            return None;
        }
        // SAFETY: the reference the builder returned.
        return Some(unsafe { Owned::from_raw(query) });
    }
    // SAFETY: as above.
    Some(unsafe { Owned::of(data) })
}

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend creates this class or a subclass; zeroed data is no client.
    unsafe { ClientObject::create(&CLIENT, ce) }
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees a client object.
    let data = unsafe { ClientObject::data(std) };
    let client = std::mem::replace(&mut data.client, ptr::null_mut());
    if !client.is_null() {
        // SAFETY: the client the constructor boxed, dropped once.
        drop(unsafe { Box::from_raw(client) });
    }
    // SAFETY: as above.
    unsafe { ClientObject::dtor(std) };
}

unsafe extern "C" fn create_request(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend creates this class or a subclass; there is no data.
    unsafe { PlainObject::create(&REQUEST, ce) }
}

unsafe extern "C" fn create_response(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: as above.
    unsafe { PlainObject::create(&RESPONSE, ce) }
}

unsafe extern "C" fn free_plain(std: *mut zend_object) {
    // SAFETY: Zend frees an object of a class without data.
    unsafe { PlainObject::dtor(std) };
}

fn deny(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
}

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

/// MINIT, as `php_swoole_http2_client_coro_minit` did.
/// # Safety
/// Called once, at MINIT of module `module`, with upstream's static method
/// table and Swoole's exception class.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp2_client_minit(
    methods: *const zend_function_entry,
    exception: *mut zend_class_entry,
    shortname: bool,
    module: c_int,
) {
    // SAFETY: MINIT, with the caller's table.
    unsafe {
        CLIENT.register(
            c"Swoole\\Coroutine\\Http2\\Client",
            methods,
            |entry, handlers| {
                deny(entry, handlers);
                ClientObject::install(entry, handlers, create, free);
            },
        );
    }
    if shortname {
        // SAFETY: MINIT, after `register`.
        unsafe { CLIENT.alias(c"Co\\Http2\\Client") };
    }
    // SAFETY: MINIT, extending Swoole's registered exception class.
    unsafe {
        EXCEPTION.register_child(
            c"Swoole\\Coroutine\\Http2\\Client\\Exception",
            ptr::null(),
            exception,
            |_, _| {},
        );
    }
    if shortname {
        // SAFETY: as above.
        unsafe { EXCEPTION.alias(c"Co\\Http2\\Client\\Exception") };
    }
    // SAFETY: MINIT, without methods.
    unsafe {
        REQUEST.register(c"Swoole\\Http2\\Request", ptr::null(), |entry, handlers| {
            deny(entry, handlers);
            PlainObject::install(entry, handlers, create_request, free_plain);
        });
    }
    // SAFETY: as above.
    unsafe {
        RESPONSE.register(
            c"Swoole\\Http2\\Response",
            ptr::null(),
            |entry, handlers| {
                deny(entry, handlers);
                PlainObject::install(entry, handlers, create_response, free_plain);
            },
        );
    }
    let public = ZEND_ACC_PUBLIC;
    let ce = CLIENT.entry();
    // SAFETY: MINIT, with the classes just registered (each declaration below too).
    unsafe { declare_long(ce, c"errCode", 0, public) };
    // SAFETY: as above.
    unsafe { declare_string(ce, c"errMsg", c"", public) };
    // SAFETY: as above.
    unsafe { declare_long(ce, c"sock", -1, public) };
    // SAFETY: as above.
    unsafe { declare_long(ce, c"type", 0, public) };
    // SAFETY: as above.
    unsafe { CLIENT.property_null(c"setting") };
    // SAFETY: as above.
    unsafe { CLIENT.property_null(c"socket") };
    // SAFETY: as above.
    unsafe { declare_bool(ce, c"connected", false, public) };
    // SAFETY: as above.
    unsafe { CLIENT.property_null(c"host") };
    // SAFETY: as above.
    unsafe { declare_long(ce, c"port", 0, public) };
    // SAFETY: as above.
    unsafe { declare_bool(ce, c"ssl", false, public) };
    // SAFETY: as above.
    unsafe { declare_long(ce, c"serverLastStreamId", 0, public) };
    let request = REQUEST.entry();
    // SAFETY: as above.
    unsafe { declare_string(request, c"path", c"/", public) };
    // SAFETY: as above.
    unsafe { declare_string(request, c"method", c"GET", public) };
    // SAFETY: as above.
    unsafe { REQUEST.property_null(c"headers") };
    // SAFETY: as above.
    unsafe { REQUEST.property_null(c"cookies") };
    // SAFETY: as above.
    unsafe { declare_string(request, c"data", c"", public) };
    // SAFETY: as above.
    unsafe { declare_bool(request, c"pipeline", false, public) };
    // SAFETY: as above.
    unsafe { declare_bool(request, c"usePipelineRead", false, public) };
    let response = RESPONSE.entry();
    for name in [c"streamId", c"errCode", c"statusCode"] {
        // SAFETY: as above.
        unsafe { declare_long(response, name, 0, public) };
    }
    // SAFETY: as above.
    unsafe { declare_bool(response, c"pipeline", false, public) };
    for name in [c"headers", c"set_cookie_headers", c"cookies", c"data"] {
        // SAFETY: as above.
        unsafe { RESPONSE.property_null(name) };
    }
    for (name, number) in CONSTANTS {
        // SAFETY: MINIT of `module`.
        unsafe { register_long_constant(name, number, module) };
    }
}

fn this<'a>(call: &Call) -> &'a mut Data {
    // SAFETY: client methods are non-static, so `$this` is a client object.
    unsafe { ClientObject::data(call.this()) }
}

/// `http2_client_coro_get_client`: the constructed client; a fatal error if not.
fn client<'a>(call: &Call) -> &'a mut Client {
    let client = this(call).client;
    if client.is_null() {
        // SAFETY: a fatal error; it does not return.
        unsafe { native::swoole_http2_client_coro_unconstructed() };
    }
    // SAFETY: a constructed client lives as long as its object.
    let client = unsafe { &mut *client };
    client.reap();
    client
}

/// A reference to an object for the call (`SW_CLIENT_PRESERVE_SOCKET`).
struct Hold(*mut zend_object);

impl Hold {
    fn object(object: *mut zend_object) -> Self {
        // SAFETY: a live object; the hold releases this reference.
        unsafe { (*object).gc.refcount += 1 };
        Self(object)
    }

    fn new(zsocket: *const zval) -> Option<Self> {
        // SAFETY: a live zval.
        if unsafe { type_of(zsocket) } != IS_OBJECT {
            return None;
        }
        // SAFETY: an object zval.
        Some(Self::object(unsafe { value::obj_of(zsocket) }))
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        let mut held = MaybeUninit::<zval>::zeroed();
        // SAFETY: the held object, whose reference this zval takes over.
        unsafe { set_object(held.as_mut_ptr(), self.0) };
        // SAFETY: releases the reference the hold took.
        unsafe { zval_ptr_dtor(held.as_mut_ptr()) };
    }
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
    let object = call.this();
    if !this(call).client.is_null() {
        // SAFETY: from a handler, with `$this`.
        unsafe { throw_constructed_twice(object) };
        return call.set_bool(false);
    }
    if host.is_empty() {
        // SAFETY: from a handler, with the registered exception class.
        unsafe { throw_exception(EXCEPTION.entry(), c"host is empty", INVALID_PARAMS) };
        return call.set_bool(false);
    }
    this(call).client = Box::into_raw(Client::new(object, host, port, ssl));
    set_bytes(object, ce(), c"host", host);
    set_long(object, ce(), c"port", port);
    // SAFETY: a live client object.
    unsafe { value::update_bool(ce(), object, c"ssl", ssl) };
}

fn destruct(_call: &Call) {}

/// `set(array $settings): bool`.
fn set(call: &Call) {
    let client = client(call);
    let Ok(settings) = call.arity(1, 1).and_then(|()| call.array(1)) else {
        return call.set_bool(false);
    };
    // SAFETY: `$this` declares `setting`; the settings are a live array.
    unsafe { swr_zend_property_merge(ce(), call.this(), c"setting".as_ptr(), 7, settings) };
    let socket = client.socket();
    if !socket.is_null() {
        // SAFETY: a live socket and array.
        unsafe { native::swoole_http2_client_coro_apply(socket, settings) };
    }
    call.set_bool(true);
}

fn connect(call: &Call) {
    let result = client(call).connect();
    call.set_bool(result);
}

fn close(call: &Call) {
    let client = client(call);
    let _hold = Hold::new(client.zsocket());
    let result = client.close();
    call.set_bool(result);
}

/// `Z_PARAM_OBJECT_OF_CLASS(.., Swoole\Http2\Request)` of the first argument.
fn request_arg(call: &Call) -> Option<*mut zend_object> {
    let arg = call.zval(1);
    let ce = REQUEST.entry();
    // SAFETY: a passed argument.
    if unsafe { type_of(arg) } == IS_OBJECT {
        // SAFETY: an object zval.
        let object = unsafe { value::obj_of(arg) };
        // SAFETY: a live object.
        let class = unsafe { (*object).ce };
        // SAFETY: two registered classes.
        if class == ce || unsafe { swr_zend_instanceof(class, ce) } {
            return Some(object);
        }
    }
    // SAFETY: from a handler, as Zend's macro reports it; the name is the
    // class's NUL-terminated name.
    unsafe { zend_wrong_parameter_class_error(1, class_name_of(ce), arg) };
    None
}

fn class_name_of(ce: *mut zend_class_entry) -> *const c_char {
    // SAFETY: a registered class entry.
    let name = unsafe { (*ce).name };
    string::val(name).cast()
}

/// `send(Swoole\Http2\Request $request): int|false`.
fn send(call: &Call) {
    let client = client(call);
    if !client.is_available() {
        return call.set_bool(false);
    }
    let Some(request) = call.arity(1, 1).ok().and_then(|()| request_arg(call)) else {
        return call.set_bool(false);
    };
    let _hold = Hold::new(client.zsocket());
    match client.send_request(request) {
        0 => call.set_bool(false),
        id => call.set_long(i64::from(id)),
    }
}

/// `recv(float $timeout = 0)` and `read(float $timeout = 0)`.
fn receive(call: &Call, pipeline_read: bool) {
    let client = client(call);
    let Ok(timeout) = call.arity(0, 1).and_then(|()| call.double(1, 0.0)) else {
        return call.set_bool(false);
    };
    // Upstream preserves `$this` here, not the socket.
    let _hold = Hold::object(call.this());
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
    let client = client(call);
    if !client.is_available() {
        return call.set_bool(false);
    }
    let parsed = call
        .arity(2, 3)
        .and_then(|()| Ok((call.long(1, 0)?, call.bool(3, false)?)));
    let Ok((id, end)) = parsed else {
        return call.set_bool(false);
    };
    let _hold = Hold::new(client.zsocket());
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream's uint32_t stream id"
    )]
    let result = client.write_data(id as u32, call.zval(2), end);
    call.set_bool(result);
}

fn ping(call: &Call) {
    let client = client(call);
    if !client.is_available() {
        return call.set_bool(false);
    }
    let result = client.send_ping();
    call.set_bool(result);
}

/// `goaway(int $error_code = NO_ERROR, string $debug_data = ''): bool`.
fn goaway(call: &Call) {
    let client = client(call);
    if !client.is_available() {
        return call.set_bool(false);
    }
    let parsed = call
        .arity(0, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.string(2, b"")?)));
    let Ok((code, debug)) = parsed else {
        return call.set_bool(false);
    };
    let _hold = Hold::new(client.zsocket());
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
    let client = client(call);
    if client.socket().is_null() {
        return call.set_bool(false);
    }
    if id == 0 {
        return call.set_bool(true);
    }
    let exists = u32::try_from(id).is_ok_and(|id| client.streams.contains_key(&id));
    call.set_bool(exists);
}

fn settings_array(target: *mut zval, settings: &Http2Settings) {
    // SAFETY: a writable zval.
    unsafe { value::array_init(target) };
    for (name, number) in [
        (c"header_table_size", settings.header_table_size),
        (c"init_window_size", settings.init_window_size),
        (c"max_concurrent_streams", settings.max_concurrent_streams),
        (c"max_frame_size", settings.max_frame_size),
        (c"max_header_list_size", settings.max_header_list_size),
    ] {
        // SAFETY: a live array.
        unsafe { add_assoc_long_ex(target, name.as_ptr(), name.count_bytes(), i64::from(number)) };
    }
}

/// `stats(string $key = ''): array|int`.
fn stats(call: &Call) {
    let client = client(call);
    let Ok(key) = call.arity(0, 1).and_then(|()| call.string(1, b"")) else {
        return call.set_bool(false);
    };
    let active = i64::try_from(client.streams.len()).unwrap_or(i64::MAX);
    if !key.is_empty() {
        match key {
            b"current_stream_id" => call.set_long(i64::from(client.stream_id)),
            b"last_stream_id" => call.set_long(i64::from(client.last_stream_id)),
            b"local_settings" => settings_array(call.return_value(), &client.local),
            b"remote_settings" => settings_array(call.return_value(), &client.remote),
            b"active_stream_num" => call.set_long(active),
            _ => {}
        }
        return;
    }
    call.set_array();
    call.add_assoc_long(c"current_stream_id", i64::from(client.stream_id));
    call.add_assoc_long(c"last_stream_id", i64::from(client.last_stream_id));
    for (name, settings) in [
        (c"local_settings", client.local),
        (c"remote_settings", client.remote),
    ] {
        let mut array = MaybeUninit::<zval>::uninit();
        settings_array(array.as_mut_ptr(), &settings);
        // SAFETY: the return array takes the settings array.
        unsafe {
            add_assoc_zval_ex(
                call.return_value(),
                name.as_ptr(),
                name.count_bytes(),
                array.as_mut_ptr(),
            );
        }
    }
    call.add_assoc_long(c"active_stream_num", active);
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
        let list = set.list().to_vec();
        let text = |pointer: *const u8, length: usize| {
            // SAFETY: the arena outlives these reads.
            unsafe { std::slice::from_raw_parts(pointer, length) }.to_vec()
        };
        assert_eq!(list.len(), 3, "three slots");
        assert_eq!(
            text(list[1].name, list[1].name_length),
            b":authority",
            "reserved slot"
        );
        assert_eq!(
            text(list[2].name, list[2].name_length),
            b"x-token",
            "lowercased"
        );
        assert_eq!(
            text(list[2].value, list[2].value_length),
            b"a=b",
            "joined value"
        );
        assert_eq!(list[0].flags, NV_NO_COPY, "no-copy flags");
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
