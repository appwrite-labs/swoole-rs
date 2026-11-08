//! `Swoole\WebSocket\Server`, `Frame` and `CloseFrame`: registration and
//! constants, frames (packing, unpacking, permessage-deflate), the server's
//! push family, the handshake and the `onOpen`/`onMessage` dispatch, over the
//! server's Rust-owned connections and ports (crates/server), and
//! `Coroutine\Http\Client`'s WebSocket state.
use crate::server::new_zval;
use crate::{
    context,
    host::{self, CO_SOCKET, Context},
    send,
};
use std::{
    ffi::{CStr, c_int, c_void},
    mem::MaybeUninit,
    ptr, slice,
    sync::atomic::{AtomicBool, Ordering::Relaxed},
};
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::DataHead;
use swoole_rs_phpruntime::co;
use swoole_rs_phpstubs::websocket as stubs;
use swoole_rs_server::{server_methods, view};
use swoole_rs_websocket::{
    DecodedFrame, INVALID_CONTROL, encode_close_into, encode_into, swr_v1_ws_control_invalid,
    swr_v1_ws_encoded_size, swr_v1_ws_message_policy, swr_v1_ws_pack_policy,
};
use swoole_rs_zend::{
    Call, Class, bailout, declare_long, declare_property_null, declare_string, handlers,
    init_object, method_table, register_long_constant,
    string::{self, bytes},
    sys::{
        _zval_struct__bindgen_ty_1, HashTable, IS_FALSE, IS_NULL, IS_OBJECT, IS_STRING,
        ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, ZEND_ACC_STATIC, swr_zend_instanceof,
        swr_zend_is_true, swr_zend_property_slot, swr_zend_string_release,
        swr_zend_ws_property_not_null, swr_zval_is_true, swr_zval_str, zend_ce_stringable,
        zend_class_entry, zend_class_implements, zend_hash_str_find, zend_long, zend_object,
        zend_string, zend_update_property, zval, zval_ptr_dtor,
    },
    type_of,
    value::{self, get_long, get_string, obj_of},
};

static SERVER: Class = Class::new();
static FRAME: Class = Class::new();
static CLOSE_FRAME: Class = Class::new();
/// `SW_HAVE_ZLIB`.
pub(crate) static ZLIB: AtomicBool = AtomicBool::new(false);

/// `swoole::websocket::Opcode`.
const OPCODE_CONTINUATION: u8 = 0;
const OPCODE_TEXT: u8 = 1;
const OPCODE_BINARY: u8 = 2;
const OPCODE_CLOSE: u8 = 8;
pub(crate) const OPCODE_PING: u8 = 9;
const OPCODE_PONG: u8 = 10;
/// `swoole::websocket::Flag`.
pub(crate) const FLAG_FIN: u8 = 1;
const FLAG_COMPRESS: u8 = 2;
const FLAG_RSV1: u8 = 4;
const FLAG_RSV2: u8 = 8;
const FLAG_RSV3: u8 = 16;
const FLAG_MASK: u8 = 32;
const FLAG_ENCODE_HEADER_ONLY: u8 = 64;
const FLAGS_ALL: i64 = 63;
/// `swoole::websocket::Status`.
const STATUS_CONNECTION: i64 = 1;
const STATUS_HANDSHAKE: i64 = 2;
const STATUS_ACTIVE: i64 = 3;
const STATUS_CLOSING: i64 = 4;
const STATUS_HANDSHAKE_FAILED: i64 = 5;
/// `swoole::websocket::CloseReason`, as their constants name them.
const CLOSE_NORMAL: i64 = 1000;
const CLOSE_REASONS: [(&CStr, i64); 15] = [
    (c"NORMAL", 1000),
    (c"GOING_AWAY", 1001),
    (c"PROTOCOL_ERROR", 1002),
    (c"DATA_ERROR", 1003),
    (c"STATUS_ERROR", 1005),
    (c"ABNORMAL", 1006),
    (c"MESSAGE_ERROR", 1007),
    (c"POLICY_ERROR", 1008),
    (c"MESSAGE_TOO_BIG", 1009),
    (c"EXTENSION_MISSING", 1010),
    (c"SERVER_ERROR", 1011),
    (c"CLOSE_SERVICE_RESTART", 1012),
    (c"TRY_AGAIN_LATER", 1013),
    (c"BAD_GATEWAY", 1014),
    (c"TLS", 1015),
];
/// `SW_WEBSOCKET_CLOSE_REASON_MAX_LEN`.
const CLOSE_REASON_MAX: usize = 125;
/// `SW_WEBSOCKET_SEC_KEY_LEN` base64-encoded.
const SEC_KEY_LENGTH: usize = 24;

/// Swoole error codes and log levels (`include/swoole_error.h`, `swoole_log.h`).
const PROTOCOL_ERROR: c_int = 508;
const SESSION_NOT_EXIST: c_int = 1005;
const WEBSOCKET_UNCONNECTED: c_int = 8503;
const WEBSOCKET_PACK_FAILED: c_int = 8505;
const WEBSOCKET_INCOMPLETE_PACKET: c_int = 8507;
const LOG_TRACE: c_int = 1;
const LOG_WARNING: c_int = 4;

/// The callbacks the handshake and frames call (`SW_SERVER_CB_*`).
const ON_BEFORE_HANDSHAKE_RESPONSE: c_int = 6;
const ON_OPEN: c_int = 7;
const ON_MESSAGE: c_int = 8;

/// `SW_OK`/`SW_ERR`.
const OK: c_int = 0;
const ERR: c_int = -1;

/// `swoole::Server`, opaque.
type Server = c_void;

/// `Swoole\WebSocket\Server`; `swoole_server.cc` tells a WebSocket server by it.
#[unsafe(no_mangle)]
pub static mut swoole_websocket_server_ce: *mut zend_class_entry = ptr::null_mut();

/// What the methods read of a `swoole::Connection`.
#[derive(Clone, Copy)]
struct Connection {
    status: u8,
    server_fd: c_int,
    compression: bool,
    closed: bool,
}

unsafe extern "C" {
    #[link_name = "_Z25php_swoole_server_get_ptrP12_zval_struct"]
    fn php_swoole_server_get_ptr(object: *mut zval) -> *mut Server;
    #[link_name = "_Z26php_swoole_server_zval_ptrPN6swoole6ServerE"]
    fn php_swoole_server_zval_ptr(server: *mut Server) -> *mut zval;
}

/// `php_swoole_server_get_and_check_server(object)`: `None` after the
/// `E_ERROR` of a server object never constructed.
pub(crate) fn server_of(object: *mut zval) -> Option<*mut Server> {
    // SAFETY: a `Swoole\Server` object.
    let server = unsafe { php_swoole_server_get_ptr(object) };
    if server.is_null() {
        // SAFETY: as above.
        let object = unsafe { obj_of(object) };
        // SAFETY: a live object.
        let name = unsafe { swoole_rs_zend::class_name(object) };
        let message = format!("Invalid instance of {}", name.to_string_lossy());
        host::set_last_error(host::PHP_FATAL_ERROR);
        let message = std::ffi::CString::new(message).unwrap_or_default();
        // SAFETY: an E_ERROR, raised once the handler's frames are done.
        bailout::defer(unsafe { swoole_rs_zend::php_error(swoole_rs_zend::E_ERROR, &message) });
        return None;
    }
    Some(server)
}

/// `Server::is_started()`.
pub(crate) fn started(server: *mut Server) -> bool {
    // SAFETY: a live server.
    unsafe { view::is_started(server) }
}

/// `conn->websocket_status = status` of the session's slot.
fn set_status(server: *mut Server, fd: zend_long, status: i64) {
    // SAFETY: a live server.
    if let Some(conn) = unsafe { view::by_session(server, fd) } {
        conn.websocket_status = u8::try_from(status).unwrap_or_default();
    }
}

/// `Server::send`; with `wait`, waiting in a coroutine while the buffer is full.
fn send(server: *mut Server, fd: zend_long, data: &[u8], wait: bool) -> bool {
    if wait {
        // SAFETY: a live server and the bytes.
        return unsafe { context::server_send(server, fd, data.as_ptr().cast(), data.len()) };
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Server::send's uint32_t length"
    )]
    // SAFETY: as above.
    unsafe {
        server_methods::send(server, fd, data.as_ptr().cast(), data.len() as u32)
    }
}

/// `Server::close(fd, reset)`.
fn close(server: *mut Server, fd: zend_long, reset: bool) -> bool {
    // SAFETY: a live server.
    unsafe { server_methods::close(server, fd, reset) }
}

/// The port's `open_websocket_{close,ping,pong}_frame` bits, or `None` when
/// the session is gone (`get_port_by_session_id`).
fn port_frames(server: *mut Server, fd: zend_long) -> Option<u8> {
    // SAFETY: a live server.
    let conn = unsafe { view::by_session(server, fd) }?;
    // SAFETY: as above.
    let port = unsafe { view::port_by_fd(server, conn.fd) };
    // SAFETY: a live port.
    (!port.is_null()).then(|| unsafe { view::port(port) }.websocket_frames)
}

/// The `websocket_subprotocol` of the port listening on `server_fd`.
fn port_protocol<'a>(server: *mut Server, server_fd: c_int) -> &'a [u8] {
    // SAFETY: a live server.
    let port = unsafe { view::port_by_server_fd(server, server_fd) };
    if port.is_null() {
        return &[];
    }
    // SAFETY: a live port, whose settings outlive the handshake.
    unsafe { view::port(port) }
        .websocket_protocol
        .as_deref()
        .map_or(&[], Vec::as_slice)
}

/// A boolean setting's value, when set and not null.
pub(crate) fn setting(table: *mut HashTable, key: &[u8]) -> Option<bool> {
    // SAFETY: the live settings table and the key's bytes.
    let found = unsafe { zend_hash_str_find(table, key.as_ptr().cast(), key.len()) };
    // SAFETY: a found entry is live.
    if found.is_null() || unsafe { type_of(found) } == IS_NULL {
        return None;
    }
    // SAFETY: as above.
    Some(unsafe { swr_zend_is_true(found) })
}

method_table! {
    static SERVER_METHODS = [
        c"push" => zim_swoole_websocket_server_push, &stubs::arginfo_class_Swoole_WebSocket_Server_push, ZEND_ACC_PUBLIC;
        c"disconnect" => zim_swoole_websocket_server_disconnect, &stubs::arginfo_class_Swoole_WebSocket_Server_disconnect, ZEND_ACC_PUBLIC;
        c"ping" => zim_swoole_websocket_server_ping, &stubs::arginfo_class_Swoole_WebSocket_Server_ping, ZEND_ACC_PUBLIC;
        c"isEstablished" => zim_swoole_websocket_server_isEstablished, &stubs::arginfo_class_Swoole_WebSocket_Server_isEstablished, ZEND_ACC_PUBLIC;
        c"pack" => zim_swoole_websocket_server_pack, &stubs::arginfo_class_Swoole_WebSocket_Server_pack, ZEND_ACC_PUBLIC | ZEND_ACC_STATIC;
        c"unpack" => zim_swoole_websocket_server_unpack, &stubs::arginfo_class_Swoole_WebSocket_Server_unpack, ZEND_ACC_PUBLIC | ZEND_ACC_STATIC;
    ];
}

method_table! {
    static FRAME_METHODS = [
        c"__toString" => zim_swoole_websocket_frame___toString, &stubs::arginfo_class_Swoole_WebSocket_Frame___toString, ZEND_ACC_PUBLIC;
        c"pack" => zim_swoole_websocket_server_pack, &stubs::arginfo_class_Swoole_WebSocket_Frame_pack, ZEND_ACC_PUBLIC | ZEND_ACC_STATIC;
        c"unpack" => zim_swoole_websocket_server_unpack, &stubs::arginfo_class_Swoole_WebSocket_Frame_unpack, ZEND_ACC_PUBLIC | ZEND_ACC_STATIC;
    ];
}

/// MINIT: the three classes and the constants, as
/// `php_swoole_websocket_server_minit` registered them.
/// # Safety
/// Called once, at MINIT, after `Swoole\Http\Server` is registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_websocket_minit(
    module_number: c_int,
    serializable: bool,
    zlib: bool,
) {
    ZLIB.store(zlib, Relaxed);
    // SAFETY: MINIT registered it first and nothing writes it now.
    let http_server = unsafe { crate::server::swoole_http_server_ce };
    // SAFETY: MINIT, with a static table and the registered parent. Its
    // objects are the parent's (no handlers of its own are installed).
    unsafe {
        SERVER.register_child(
            c"Swoole\\WebSocket\\Server",
            SERVER_METHODS.as_ptr(),
            http_server,
            |entry, _| {
                if !serializable {
                    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
                }
            },
        );
    }
    // SAFETY: as above.
    unsafe {
        FRAME.register(
            c"Swoole\\WebSocket\\Frame",
            FRAME_METHODS.as_ptr(),
            |_, _| {},
        );
    }
    let ce = FRAME.entry();
    // SAFETY: Stringable is registered by the engine before any extension.
    let stringable = unsafe { zend_ce_stringable };
    // SAFETY: MINIT, with the class just registered.
    unsafe { zend_class_implements(ce, 1, stringable) };
    for (name, value) in [
        (c"fd", 0),
        (c"opcode", i64::from(OPCODE_TEXT)),
        (c"flags", i64::from(FLAG_FIN)),
    ] {
        if name == c"opcode" {
            // SAFETY: as above; declared in upstream's order.
            unsafe { declare_string(ce, c"data", c"", ZEND_ACC_PUBLIC) };
        }
        // SAFETY: as above.
        unsafe { declare_long(ce, name, value, ZEND_ACC_PUBLIC) };
    }
    // SAFETY: as above.
    unsafe { declare_property_null(ce, c"finish", ZEND_ACC_PUBLIC) };
    // SAFETY: as above, extending Frame.
    unsafe {
        CLOSE_FRAME.register_child(c"Swoole\\WebSocket\\CloseFrame", ptr::null(), ce, |_, _| {});
    }
    let close = CLOSE_FRAME.entry();
    for (name, value) in [
        (c"opcode", i64::from(OPCODE_CLOSE)),
        (c"code", CLOSE_NORMAL),
    ] {
        // SAFETY: as above.
        unsafe { declare_long(close, name, value, ZEND_ACC_PUBLIC) };
    }
    // SAFETY: as above.
    unsafe { declare_string(close, c"reason", c"", ZEND_ACC_PUBLIC) };
    constants(module_number);
    // SAFETY: MINIT, before any thread reads it.
    unsafe { swoole_websocket_server_ce = SERVER.entry() };
}

/// The global constants, `SWOOLE_WEBSOCKET_*` then the BC `WEBSOCKET_*`, in
/// upstream's order.
fn constants(module: c_int) {
    let opcodes = [
        (c"CONTINUATION", OPCODE_CONTINUATION),
        (c"TEXT", OPCODE_TEXT),
        (c"BINARY", OPCODE_BINARY),
        (c"CLOSE", OPCODE_CLOSE),
        (c"PING", OPCODE_PING),
        (c"PONG", OPCODE_PONG),
    ];
    let flags = [
        (c"FIN", FLAG_FIN),
        (c"RSV1", FLAG_RSV1),
        (c"RSV2", FLAG_RSV2),
        (c"RSV3", FLAG_RSV3),
        (c"MASK", FLAG_MASK),
        (c"COMPRESS", FLAG_COMPRESS),
    ];
    let register = |prefix: &str, group: &str, name: &CStr, value: i64| {
        let name = format!("{prefix}{group}{}", name.to_string_lossy());
        let name = std::ffi::CString::new(name).unwrap_or_default();
        // SAFETY: MINIT, with the extension's module number; Zend copies the name.
        unsafe { register_long_constant(&name, value, module) };
    };
    for (prefix, bc) in [("SWOOLE_WEBSOCKET_", false), ("WEBSOCKET_", true)] {
        let mut statuses = vec![
            (c"CONNECTION", STATUS_CONNECTION),
            (c"HANDSHAKE", STATUS_HANDSHAKE),
        ];
        if bc {
            statuses.push((c"FRAME", STATUS_ACTIVE));
        }
        statuses.extend([(c"ACTIVE", STATUS_ACTIVE), (c"CLOSING", STATUS_CLOSING)]);
        if bc {
            statuses.push((c"HANDSHAKE_FAILED", STATUS_HANDSHAKE_FAILED));
        }
        for (name, value) in statuses {
            register(prefix, "STATUS_", name, value);
        }
        for (name, value) in opcodes {
            register(prefix, "OPCODE_", name, i64::from(value));
        }
        if !bc {
            for (name, value) in flags {
                register(prefix, "FLAG_", name, i64::from(value));
            }
        }
        for (name, value) in CLOSE_REASONS {
            register(prefix, "CLOSE_", name, value);
        }
    }
}

/// `swoole::websocket::FrameObject`: an outgoing frame (C++ shares the layout).
#[repr(C)]
pub struct FrameObject {
    opcode: u8,
    flags: u8,
    code: u16,
    data: *mut zval,
}

/// Truncate to a byte, as upstream's `uint8_t` fields do.
fn byte(value: i64) -> u8 {
    value.to_le_bytes()[0]
}

impl FrameObject {
    /// `FrameObject(zdata, opcode, flags, code)`: a `Frame` object's
    /// properties, or the arguments around any other value.
    fn new(data: *mut zval, opcode: i64, flags: i64, code: i64) -> Self {
        // SAFETY: a live zval.
        let object = unsafe { type_of(data) } == IS_OBJECT && {
            // SAFETY: an object zval.
            let object = unsafe { obj_of(data) };
            // SAFETY: a live object.
            let class = unsafe { (*object).ce };
            // SAFETY: two registered classes.
            unsafe { swr_zend_instanceof(class, FRAME.entry()) }
        };
        if !object {
            #[expect(clippy::cast_possible_truncation, reason = "upstream's uint16_t code")]
            #[expect(clippy::cast_sign_loss, reason = "upstream's uint16_t code")]
            let code = code as u16;
            return Self {
                opcode: byte(opcode),
                flags: byte(flags & FLAGS_ALL),
                code,
                data,
            };
        }
        // SAFETY: an object zval.
        let object = unsafe { obj_of(data) };
        let long = |name: &CStr| {
            // SAFETY: a live Frame, which declares the property.
            let slot = unsafe { swr_zend_property_slot(object, name.as_ptr(), name.count_bytes()) };
            // SAFETY: the live slot.
            unsafe { get_long(slot) }
        };
        let opcode = byte(long(c"opcode"));
        let mut code = 0;
        let data = if opcode == OPCODE_CLOSE {
            code = not_null(object, c"code").map_or(CLOSE_NORMAL, |code| {
                // SAFETY: the live property.
                unsafe { get_long(code) }
            });
            not_null(object, c"reason")
        } else {
            not_null(object, c"data")
        };
        let mut flags = byte(long(c"flags") & FLAGS_ALL);
        if let Some(finish) = not_null(object, c"finish") {
            // SAFETY: the live property.
            if unsafe { swr_zval_is_true(finish) } {
                flags |= FLAG_FIN;
            } else {
                flags &= !FLAG_FIN;
            }
        }
        #[expect(clippy::cast_possible_truncation, reason = "upstream's uint16_t code")]
        #[expect(clippy::cast_sign_loss, reason = "upstream's uint16_t code")]
        let code = code as u16;
        Self {
            opcode,
            flags,
            code,
            data: data.unwrap_or(ptr::null_mut()),
        }
    }
}

thread_local! {
    /// The `rv` a subclass's `__get` returns into, one per property a frame
    /// reads (upstream's is a dead stack slot by the time it is used).
    static RV: [std::cell::UnsafeCell<zval>; 4] = const {
        // SAFETY: a zval is plain data; all zeroes is IS_UNDEF.
        [const { std::cell::UnsafeCell::new(unsafe { std::mem::zeroed() }) }; 4]
    };
}

/// `sw_zend_read_property_not_null_ex(swoole_websocket_frame_ce, ..., 1)`.
fn not_null(object: *mut zend_object, name: &CStr) -> Option<*mut zval> {
    if bailout::pending() {
        return None;
    }
    let rv = RV.with(|rv| {
        match name.to_bytes() {
            b"code" => &rv[0],
            b"reason" => &rv[1],
            b"data" => &rv[2],
            _ => &rv[3],
        }
        .get()
    });
    let mut bailed = 0;
    // SAFETY: a live object of the class; the shim traps a bailout in __get.
    let found = unsafe {
        swr_zend_ws_property_not_null(
            FRAME.entry(),
            object,
            name.as_ptr(),
            name.count_bytes(),
            rv,
            &raw mut bailed,
        )
    };
    if bailed != 0 {
        bailout::Bailout.defer();
    }
    (!found.is_null()).then_some(found)
}

/// An owned `zend_string` reference, released on drop.
pub(crate) struct Str(*mut zend_string);

impl Str {
    pub(crate) fn bytes(&self) -> &[u8] {
        if self.0.is_null() {
            return &[];
        }
        // SAFETY: a live string this value holds a reference to.
        unsafe { bytes(self.0) }
    }

    /// The reference, which the caller now owns.
    fn take(mut self) -> *mut zend_string {
        std::mem::replace(&mut self.0, ptr::null_mut())
    }
}

impl Drop for Str {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the reference this value owns.
            unsafe { swr_zend_string_release(self.0) };
        }
    }
}

/// `"%s(): message"` from `swoole_warning` in `func`.
fn warning(func: &CStr, message: &str) {
    host::log(0, 0, func, message);
}

/// `FrameObject::pack`: the frame on the wire as a new string, or `None`.
fn pack(frame: &FrameObject) -> Option<Frame> {
    if frame.opcode > OPCODE_PONG {
        host::fatal("the maximum value of opcode is 10");
        return None;
    }
    // SAFETY: a live zval when set.
    let data = if frame.data.is_null() || unsafe { type_of(frame.data) } == IS_NULL {
        Str(ptr::null_mut())
    } else {
        // SAFETY: as above; the new reference is released on drop.
        Str(unsafe { get_string(frame.data) })
    };
    if bailout::pending() {
        return None;
    }
    if !ZLIB.load(Relaxed) {
        warning(
            c"FrameObject::pack",
            "Unable to compress websocket data frame, the `zlib` supports is required",
        );
        return None;
    }
    let policy = swr_v1_ws_pack_policy(frame.opcode, frame.flags, data.bytes().len());
    let mut flags = policy.to_le_bytes()[0];
    let compressed = if policy & 0x100 == 0 {
        None
    } else if let Some(compressed) = compress(data.bytes()) {
        flags |= FLAG_RSV1;
        Some(compressed)
    } else {
        flags &= !FLAG_RSV1;
        None
    };
    let payload = compressed.as_deref().unwrap_or(data.bytes());
    if frame.opcode == OPCODE_CLOSE {
        close_frame(i32::from(frame.code), payload, flags)
    } else {
        encode(payload, frame.opcode, flags)
    }
}

/// `websocket::message_compress(.., Z_DEFAULT_COMPRESSION)`, warning as it did.
fn compress(data: &[u8]) -> Option<Vec<u8>> {
    use swoole_rs_deflate::{MessageFailure, message};
    match message(data, -1, crate::header::zalloc, crate::header::zfree) {
        Ok(compressed) => Some(compressed),
        Err(MessageFailure::Init(status)) => {
            host::fatal(&format!("deflateInit2() failed, Error: [{status}]"));
            None
        }
        Err(MessageFailure::Deflate) => {
            host::fatal("Failed to compress outgoing frame");
            None
        }
    }
}

/// Frames up to this size encode on the stack: upstream encodes into a reused
/// buffer, so a small message costs no allocation there either.
const INLINE: usize = 128;

/// An encoded frame, on the stack when it fits.
pub(crate) enum Frame {
    Inline([MaybeUninit<u8>; INLINE], usize),
    Heap(Str),
}

impl Frame {
    pub(crate) fn bytes(&self) -> &[u8] {
        match self {
            Self::Inline(buffer, size) => buffer.get(..*size).map_or(&[], |bytes| {
                // SAFETY: `encoded` keeps an inline frame only once `size` bytes are written.
                unsafe { bytes.assume_init_ref() }
            }),
            Self::Heap(string) => string.bytes(),
        }
    }

    /// The frame as a new string the caller owns.
    fn into_string(self) -> *mut zend_string {
        match self {
            Self::Inline(..) => string::copy(self.bytes()),
            Self::Heap(string) => string.take(),
        }
    }
}

/// A frame of `size` bytes `write` fills; `None` unless it wrote exactly
/// `size`.
fn encoded(size: usize, write: impl FnOnce(&mut [MaybeUninit<u8>]) -> usize) -> Option<Frame> {
    if size == 0 {
        return None;
    }
    if size <= INLINE {
        let mut buffer = [MaybeUninit::uninit(); INLINE];
        return (write(&mut buffer) == size).then_some(Frame::Inline(buffer, size));
    }
    let out = string::alloc(size);
    // SAFETY: the fresh string's `size` writable bytes, which nothing else references.
    let bytes = unsafe { slice::from_raw_parts_mut(string::val(out).cast(), size) };
    if write(bytes) != size {
        // SAFETY: the fresh string nothing else references.
        unsafe { string::free(out) };
        return None;
    }
    // SAFETY: as above, now holding `size` bytes.
    unsafe { string::truncate(out, size) };
    Some(Frame::Heap(Str(out)))
}

/// Upstream's encoders fail a masked header-only frame after writing it.
fn header_only_mask(flags: u8) -> bool {
    flags & FLAG_MASK != 0 && flags & FLAG_ENCODE_HEADER_ONLY != 0
}

/// `websocket::encode`.
pub(crate) fn encode(data: &[u8], opcode: u8, flags: u8) -> Option<Frame> {
    if swr_v1_ws_control_invalid(opcode, flags, data.len()) {
        warning(
            c"websocket::encode",
            &format!(
                "invalid websocket control frame: opcode={opcode}, fin={}, payload_length={}",
                u8::from(flags & FLAG_FIN != 0),
                data.len()
            ),
        );
        return None;
    }
    let size = swr_v1_ws_encoded_size(data.len(), flags);
    let frame = encoded(size, |out| encode_into(data, opcode, flags, out))?;
    (!header_only_mask(flags)).then_some(frame)
}

/// `websocket::pack_close_frame`.
fn close_frame(code: i32, reason: &[u8], flags: u8) -> Option<Frame> {
    let func = c"websocket::pack_close_frame";
    if reason.len() > CLOSE_REASON_MAX {
        warning(func, "the max length of close reason is 125");
        return None;
    }
    if swr_v1_ws_control_invalid(OPCODE_CLOSE, FLAG_FIN, reason.len() + 2) {
        warning(
            func,
            &format!(
                "invalid websocket control frame: opcode=8, fin=1, payload_length={}",
                reason.len() + 2
            ),
        );
        return None;
    }
    let size = swr_v1_ws_encoded_size(reason.len() + 2, flags | FLAG_FIN);
    let frame = encoded(size, |out| encode_close_into(code, reason, flags, out))?;
    (!header_only_mask(flags)).then_some(frame)
}

/// `FrameObject::uncompress`: the inflated payload into `payload`.
fn uncompress(payload: *mut zval, data: &[u8]) -> bool {
    use swoole_rs_inflate::{MessageFailure, error, message};
    if !ZLIB.load(Relaxed) {
        warning(
            c"FrameObject::uncompress",
            "The compressed websocket data frame is received, the `zlib` supports is required",
        );
        return false;
    }
    match message(data) {
        Ok(plain) => {
            // SAFETY: the caller's empty slot takes the new string.
            unsafe { swr_zval_str(payload, string::copy(&plain)) };
            true
        }
        Err(MessageFailure::Init(status)) => {
            warning(
                c"websocket::message_uncompress",
                &format!(
                    "inflateInit2() failed by {}",
                    error(status).to_string_lossy()
                ),
            );
            false
        }
        Err(MessageFailure::Inflate(status)) => {
            host::fatal(&format!(
                "inflate() failed, Error: {}[{status}]",
                error(status).to_string_lossy()
            ));
            false
        }
    }
}

/// `websocket::construct_frame`: a `Frame`, or a `CloseFrame` with the close
/// payload's code and reason.
fn construct(frame: *mut zval, opcode: zend_long, payload: *mut zval, mut flags: u8) {
    let class = if opcode == zend_long::from(OPCODE_CLOSE) {
        CLOSE_FRAME.entry()
    } else {
        FRAME.entry()
    };
    // SAFETY: the caller's writable slot and a registered class.
    unsafe { init_object(frame, class) };
    // SAFETY: the object just created.
    let object = unsafe { obj_of(frame) };
    if class == CLOSE_FRAME.entry() {
        // SAFETY: a close payload is a string.
        let data = unsafe { value::str_of(payload) };
        // SAFETY: a live string.
        let data = unsafe { bytes(data) };
        if let [high, low, reason @ ..] = data {
            // `(payload[0] << 8) ^ (payload[1] & 0xFF)` over a signed char.
            let code = (i64::from(high.cast_signed()) << 8) ^ i64::from(*low);
            // SAFETY: a live CloseFrame.
            unsafe { value::update_long(class, object, c"code", code) };
            if !reason.is_empty() {
                let reason = string::copy(reason);
                let mut reason_zval = zval_of(reason);
                // SAFETY: as above; the property takes its own reference.
                unsafe {
                    zend_update_property(
                        class,
                        object,
                        c"reason".as_ptr(),
                        6,
                        &raw mut reason_zval,
                    );
                }
                // SAFETY: the reference made here.
                unsafe { zval_ptr_dtor(&raw mut reason_zval) };
            }
        }
    } else {
        // SAFETY: a live Frame and payload; the property takes its own reference.
        unsafe { zend_update_property(FRAME.entry(), object, c"data".as_ptr(), 4, payload) };
    }
    if flags & FLAG_RSV1 != 0 {
        flags |= FLAG_COMPRESS;
    }
    let ce = FRAME.entry();
    // SAFETY: a live Frame.
    unsafe { value::update_long(ce, object, c"opcode", opcode) };
    // SAFETY: as above.
    unsafe { value::update_long(ce, object, c"flags", i64::from(flags)) };
    // SAFETY: as above.
    unsafe { value::update_bool(ce, object, c"finish", flags & FLAG_FIN != 0) };
}

/// A zval holding `string`'s reference.
pub(crate) fn zval_of(string: *mut zend_string) -> zval {
    let mut out = new_zval();
    // SAFETY: an empty slot takes the reference.
    unsafe { swr_zval_str(&raw mut out, string) };
    out
}

/// `swoole_websocket_server_pack`'s body: the packed frame, or `""`.
fn pack_value(call: &Call, data: *mut zval, opcode: i64, flags: i64) {
    match pack(&FrameObject::new(data, opcode, flags, 0)) {
        // SAFETY: the return slot takes the new string.
        Some(packed) => unsafe { call.set_string(packed.into_string()) },
        None => call.set_empty_string(),
    }
}

fn pack_method(call: &Call) {
    let parsed = call.arity(1, 3).and_then(|()| {
        Ok((
            call.zval(1),
            call.long(2, i64::from(OPCODE_TEXT))?,
            call.long(3, i64::from(FLAG_FIN))?,
        ))
    });
    match parsed {
        Ok((data, opcode, flags)) => pack_value(call, data, opcode, flags),
        Err(_) => call.set_bool(false),
    }
}

fn to_string(call: &Call) {
    pack_value(call, call.this_zval(), 0, 0);
}

/// `websocket::decode` of a whole frame, warning as it does; the input is
/// unmasked in place.
fn decode(data: &mut [u8]) -> Option<DecodedFrame> {
    let func = c"websocket::decode";
    let (total, frame) = swoole_rs_websocket::decode_in_place(data);
    if total == INVALID_CONTROL {
        warning(
            func,
            &format!(
                "invalid compressed websocket control frame received: opcode={}",
                frame.opcode
            ),
        );
        return None;
    }
    if total <= 0 || usize::try_from(total).is_ok_and(|total| data.len() < total) {
        host::log(
            LOG_WARNING,
            WEBSOCKET_INCOMPLETE_PACKET,
            func,
            format!(
                "incomplete packet, expected length is {}, actual length is {}",
                total.cast_unsigned(),
                data.len()
            ),
        );
        return None;
    }
    Some(frame)
}

fn unpack(call: &Call) {
    let Ok(Some(string)) = call.arity(1, 1).and_then(|()| call.str(1)) else {
        call.set_bool(false);
        return;
    };
    // SAFETY: the argument's string; upstream decodes (unmasks) it in place.
    let length = unsafe { (*string).len };
    // SAFETY: the string holds `length` bytes at its value.
    let data = unsafe { std::slice::from_raw_parts_mut(string::val(string), length) };
    if data.len() < 2 {
        host::set_last_error(PROTOCOL_ERROR);
        call.set_bool(false);
        return;
    }
    let Some(frame) = decode(data) else {
        host::set_last_error(PROTOCOL_ERROR);
        call.set_bool(false);
        return;
    };
    let payload = data.get(frame.header_length..frame.header_length + frame.payload_length);
    let payload = payload.unwrap_or_default();
    let mut zpayload = new_zval();
    if frame.flags & FLAG_RSV1 != 0 {
        if !uncompress(&raw mut zpayload, payload) {
            host::set_last_error(PROTOCOL_ERROR);
            call.set_bool(false);
            return;
        }
    } else {
        zpayload = zval_of(string::copy(payload));
    }
    construct(
        call.return_value(),
        i64::from(frame.opcode),
        &raw mut zpayload,
        frame.flags,
    );
    // SAFETY: the payload reference made here.
    unsafe { zval_ptr_dtor(&raw mut zpayload) };
}

/// `php_swoole_server_get_and_check_server(ZEND_THIS)` and the running check.
fn server(call: &Call) -> Option<*mut Server> {
    let server = server_of(call.this_zval())?;
    if !started(server) {
        host::fatal("server is not running");
        return None;
    }
    Some(server)
}

/// `Server::get_connection_verify(fd) != nullptr`.
pub(crate) fn server_connection(server: *mut Server, fd: zend_long) -> bool {
    connection(server, fd, true).is_some()
}

fn connection(server: *mut Server, fd: zend_long, verify: bool) -> Option<Connection> {
    let conn = if verify {
        // SAFETY: a live server.
        unsafe { view::verified(server, fd) }
    } else {
        // SAFETY: as above.
        unsafe { view::by_session(server, fd) }
    }?;
    Some(Connection {
        status: conn.websocket_status,
        server_fd: conn.server_fd,
        compression: conn.websocket_compression != 0,
        closed: conn.closed.load(Relaxed) != 0,
    })
}

/// `swoole_websocket_server_push`: a packed frame to a WebSocket connection.
fn push_frame(server: *mut Server, fd: zend_long, frame: &[u8]) -> bool {
    if fd <= 0 {
        host::fatal(&format!("fd[{fd}] is invalid"));
        return false;
    }
    if connection(server, fd, false).is_none_or(|conn| i64::from(conn.status) < STATUS_HANDSHAKE) {
        host::set_last_error(WEBSOCKET_UNCONNECTED);
        host::fatal(&format!(
            "the connected client of connection[{fd}] is not a websocket client or closed"
        ));
        return false;
    }
    send(server, fd, frame, true)
}

fn push(call: &Call) {
    let run = || -> Option<bool> {
        let server = server(call)?;
        call.arity(2, 4).ok()?;
        let fd = call.long(1, 0).ok()?;
        let data = call.zval(2);
        let opcode = call.long(3, i64::from(OPCODE_TEXT)).ok()?;
        let flags = call.zval_or_null(4).map_or(i64::from(FLAG_FIN), |flags| {
            // SAFETY: a passed argument.
            unsafe { get_long(flags) }
        });
        let Some(conn) = connection(server, fd, true) else {
            host::set_last_error(SESSION_NOT_EXIST);
            host::fatal(&format!("session#{fd} does not exists"));
            return None;
        };
        let mut frame = FrameObject::new(data, opcode, flags, 0);
        if conn.compression {
            frame.flags |= FLAG_COMPRESS;
        }
        frame.flags &= !FLAG_MASK;
        let Some(packed) = pack(&frame) else {
            host::set_last_error(WEBSOCKET_PACK_FAILED);
            return None;
        };
        Some(push_frame(server, fd, packed.bytes()))
    };
    call.set_bool(run().unwrap_or(false));
}

fn disconnect(call: &Call) {
    let run = || -> Option<bool> {
        let server = server(call)?;
        call.arity(1, 3).ok()?;
        let fd = call.long(1, 0).ok()?;
        let code = call.long(2, CLOSE_NORMAL).ok()?;
        let reason = call.string(3, b"").ok()?;
        #[expect(clippy::cast_possible_truncation, reason = "upstream's int code")]
        let frame = close_frame(code as i32, reason, 0)?;
        if !push_frame(server, fd, frame.bytes()) {
            return Some(false);
        }
        connection(server, fd, false)?;
        // Mark it closing at once, so it is not closed twice.
        set_status(server, fd, STATUS_CLOSING);
        Some(close(server, fd, false))
    };
    call.set_bool(run().unwrap_or(false));
}

fn ping(call: &Call) {
    let run = || -> Option<bool> {
        let server = server(call)?;
        call.arity(1, 2).ok()?;
        let fd = call.long(1, 0).ok()?;
        let payload = call.str(2).ok()?;
        let mut data = zval_of(payload.unwrap_or_else(empty));
        let frame = FrameObject::new(
            &raw mut data,
            i64::from(OPCODE_PING),
            i64::from(FLAG_FIN),
            0,
        );
        let Some(packed) = pack(&frame) else {
            host::set_last_error(WEBSOCKET_PACK_FAILED);
            return None;
        };
        Some(push_frame(server, fd, packed.bytes()))
    };
    call.set_bool(run().unwrap_or(false));
}

/// `zend_empty_string`, borrowed.
fn empty() -> *mut zend_string {
    // SAFETY: Zend's interned empty string lives for the process.
    unsafe { swoole_rs_zend::sys::swr_zend_stream_empty_string() }
}

fn is_established(call: &Call) {
    let run = || -> Option<bool> {
        let server = server(call)?;
        let fd = call.arity(1, 1).and_then(|()| call.long(1, 0)).ok()?;
        Some(
            connection(server, fd, true)
                .is_some_and(|conn| !conn.closed && i64::from(conn.status) >= STATUS_ACTIVE),
        )
    };
    call.set_bool(run().unwrap_or(false));
}

handlers! {
    zim_swoole_websocket_server_push => push;
    zim_swoole_websocket_server_disconnect => disconnect;
    zim_swoole_websocket_server_ping => ping;
    zim_swoole_websocket_server_isEstablished => is_established;
    zim_swoole_websocket_server_pack => pack_method;
    zim_swoole_websocket_server_unpack => unpack;
    zim_swoole_websocket_frame___toString => to_string;
}

/// Run a server callback; `Err` if it bailed out. A failed call warns
/// `"Swoole\WebSocket\Server->{event} handler error"` and closes `fd`.
fn dispatch(
    server: *mut Server,
    server_fd: c_int,
    event: c_int,
    args: &mut [zval],
    fd: zend_long,
) -> Result<(), bailout::Bailout> {
    if crate::server::call_port(server, server_fd, event, args)? == Some(false) {
        let name = match event {
            ON_BEFORE_HANDSHAKE_RESPONSE => "onBeforeHandshakeResponse",
            ON_OPEN => "onOpen",
            _ => "onMessage",
        };
        host::notice(&format!("Swoole\\WebSocket\\Server->{name} handler error"));
        close(server, fd, false);
    }
    Ok(())
}

/// The bytes of a string zval (`php_swoole_get_recv_data`'s).
fn string_bytes(data: &zval) -> &[u8] {
    // SAFETY: a live string zval.
    let string = unsafe { value::str_of(data) };
    // SAFETY: a live string, borrowed for the zval's lifetime.
    unsafe { bytes(string) }
}

/// The server object, as the first callback argument (borrowed).
fn server_zval(server: *mut Server) -> zval {
    // SAFETY: the live server's object.
    let object = unsafe { php_swoole_server_zval_ptr(server) };
    // SAFETY: the server's live zval.
    unsafe { *object }
}

/// `swoole_websocket_onMessage`: a frame `Server::dispatch_frame` delivered,
/// answered by policy or handed to `onMessage` as a Frame. A bailout in the
/// handler is raised last, over this frame only. `server` and `request` are
/// the live server and its `RecvData`.
pub(crate) fn on_message(server: *mut c_void, request: *mut c_void, info: &DataHead) -> c_int {
    let (fd, server_fd, ext_flags) = (info.fd, c_int::from(info.server_fd), info.ext_flags);
    let result = message(server, request, fd, server_fd, ext_flags);
    let code = *result.as_ref().unwrap_or(&OK);
    bailout::raise(result.map(|_| ()));
    code
}

fn message(
    server: *mut Server,
    request: *mut c_void,
    fd: zend_long,
    server_fd: c_int,
    ext_flags: u16,
) -> Result<c_int, bailout::Bailout> {
    let Some(settings) = port_frames(server, fd) else {
        return Ok(ERR);
    };
    let mut data = new_zval();
    crate::server::recv_data(server, request, &raw mut data);
    let [opcode, flags] = ext_flags.to_be_bytes();
    let delivery = swr_v1_ws_message_policy(opcode, settings);
    if delivery != 0 {
        if delivery == 2 {
            let payload = string_bytes(&data);
            if let Some(pong) = encode(payload, OPCODE_PONG, FLAG_FIN) {
                send(server, fd, pong.bytes(), false);
            }
        }
        // SAFETY: the reference recv_data made.
        unsafe { zval_ptr_dtor(&raw mut data) };
        return Ok(OK);
    }
    // RFC 7692: the payload is deflated.
    if flags & FLAG_RSV1 != 0 {
        let mut plain = new_zval();
        let ok = uncompress(&raw mut plain, string_bytes(&data));
        // SAFETY: the reference recv_data made.
        unsafe { zval_ptr_dtor(&raw mut data) };
        if !ok {
            return Ok(OK);
        }
        data = plain;
    }
    let mut args = [server_zval(server), new_zval()];
    construct(
        &raw mut args[1],
        zend_long::from(opcode),
        &raw mut data,
        flags,
    );
    // SAFETY: the Frame just built.
    let frame = unsafe { obj_of(&raw const args[1]) };
    // SAFETY: as above.
    unsafe { value::update_long(FRAME.entry(), frame, c"fd", fd) };
    dispatch(server, server_fd, ON_MESSAGE, &mut args, fd)?;
    // SAFETY: the payload reference recv_data made.
    unsafe { zval_ptr_dtor(&raw mut data) };
    // SAFETY: the Frame built above.
    unsafe { zval_ptr_dtor(&raw mut args[1]) };
    Ok(OK)
}

/// `swoole_websocket_onHandshake`: the handshake, then `onOpen`, or a reset.
pub(crate) fn handshake_open(context: *mut c_void) -> Result<(), bailout::Bailout> {
    let server = send::state(context).private_data;
    let fd = host::fd(context);
    if handshake(context)? {
        on_open(server, context)
    } else {
        close(server, fd, true);
        Ok(())
    }
}

/// A handshake `onHandshake` answers: the connection is upgrading.
pub(crate) fn upgrading(server: *mut c_void, fd: zend_long) {
    set_status(server, fd, STATUS_HANDSHAKE);
}

/// Set a response header as `Context::set_header(..., false)`.
fn set_header(context: *mut Context, key: &[u8], value: &[u8]) -> Result<(), bailout::Bailout> {
    crate::response::set_header_bytes(context, key, value);
    if bailout::pending() {
        Err(bailout::Bailout)
    } else {
        Ok(())
    }
}

fn handshake(context: *mut Context) -> Result<bool, bailout::Bailout> {
    let header = send::state(context).request.zheader.ptr;
    let key = crate::parse::find(header, b"sec-websocket-key");
    let key = if key.is_null() {
        None
    } else {
        // SAFETY: a live header value; the reference is released on drop.
        let key = Str(unsafe { get_string(key) });
        if bailout::pending() {
            return Err(bailout::Bailout);
        }
        (key.bytes().len() == SEC_KEY_LENGTH).then_some(key)
    };
    let Some(key) = key else {
        send::state(context).response.status = 400;
        send::end(context, ptr::null_mut())?;
        return Ok(false);
    };
    let accept = swoole_rs_websocket::accept(key.bytes());
    drop(key);
    set_header(context, b"Upgrade", b"websocket")?;
    set_header(context, b"Connection", b"Upgrade")?;
    set_header(context, b"Sec-WebSocket-Accept", &accept)?;
    set_header(context, b"Sec-WebSocket-Version", b"13")?;
    let fd = host::fd(context);
    if host::flags(context) & CO_SOCKET == 0 {
        let server = send::state(context).private_data;
        let Some(conn) = connection(server, fd, false) else {
            closed(c"swoole_websocket_handshake", fd);
            return Ok(false);
        };
        set_status(server, fd, STATUS_ACTIVE);
        let protocol = port_protocol(server, conn.server_fd);
        if !protocol.is_empty() {
            set_header(context, b"Sec-WebSocket-Protocol", protocol)?;
        }
        let state = send::state(context);
        let mut args = [
            server_zval(server),
            // SAFETY: the context's live request and response objects.
            unsafe { *state.request.zobject.ptr },
            // SAFETY: as above.
            unsafe { *state.response.zobject.ptr },
        ];
        dispatch(
            server,
            conn.server_fd,
            ON_BEFORE_HANDSHAKE_RESPONSE,
            &mut args,
            fd,
        )?;
    } else {
        // SAFETY: a coroutine-socket context's live socket.
        unsafe { swoole_rs_phpclients::websocket_protocol(send::state(context).private_data) };
    }
    let state = send::state(context);
    state.response.status = 101;
    state.upgrade = 1;
    send::end(context, ptr::null_mut())
}

/// `swoole_error_log(SW_LOG_TRACE, SW_ERROR_SESSION_NOT_EXIST, ...)`.
fn closed(func: &CStr, fd: zend_long) {
    host::log(
        LOG_TRACE,
        SESSION_NOT_EXIST,
        func,
        format!("session[{fd}] is closed"),
    );
}

/// `swoole_websocket_onOpen`.
fn on_open(server: *mut Server, context: *mut Context) -> Result<(), bailout::Bailout> {
    let fd = host::fd(context);
    let Some(conn) = connection(server, fd, false) else {
        closed(c"swoole_websocket_onOpen", fd);
        return Ok(());
    };
    // SAFETY: the context's live request object.
    let request = unsafe { *send::state(context).request.zobject.ptr };
    let mut args = [server_zval(server), request];
    dispatch(server, conn.server_fd, ON_OPEN, &mut args, fd)
}

/// `swoole_websocket_onRequest`, the default `onRequest`: 400 and close.
pub(crate) fn request(context: *mut c_void) {
    const BAD_REQUEST: &[u8] = b"HTTP/1.1 400 Bad Request\r\n\
        Connection: close\r\n\
        Content-Type: text/html; charset=UTF-8\r\n\
        Cache-Control: must-revalidate,no-cache,no-store\r\n\
        Content-Length: 83\r\n\
        Server: swoole-http-server\r\n\r\n\
        <html><body><h2>HTTP 400 Bad Request</h2><hr><i>Powered by Swoole</i></body></html>";
    context::send(context, BAD_REQUEST);
    send::state(context).end = 1;
    context::close(context);
}

/// The coroutine `Response::push`: a frame over the context's socket.
pub(crate) fn response_push(
    context: *mut Context,
    data: *mut zval,
    opcode: i64,
    flags: Option<*mut zval>,
) -> bool {
    let flags = flags.map_or(i64::from(FLAG_FIN), |flags| {
        // SAFETY: a passed argument.
        unsafe { get_long(flags) }
    });
    let mut frame = FrameObject::new(data, opcode, flags, 0);
    frame.flags &= !FLAG_MASK;
    if send::state(context).websocket_compression != 0 {
        frame.flags |= FLAG_COMPRESS;
    }
    send_frame(context, &frame).unwrap_or(false)
}

/// `http_response_send_frame`: `None` when the frame cannot be packed.
fn send_frame(context: *mut Context, frame: &FrameObject) -> Option<bool> {
    let Some(packed) = pack(frame) else {
        host::set_last_error(WEBSOCKET_PACK_FAILED);
        return None;
    };
    let packed = packed.bytes();
    Some(context::send(context, packed))
}

/// The coroutine `Response::disconnect`: a close frame, then close.
pub(crate) fn response_disconnect(
    context: *mut Context,
    code: i64,
    reason: *mut zend_string,
) -> bool {
    let mut data = zval_of(if reason.is_null() { empty() } else { reason });
    let frame = FrameObject::new(
        &raw mut data,
        i64::from(OPCODE_CLOSE),
        i64::from(FLAG_FIN),
        code,
    );
    let Some(sent) = send_frame(context, &frame) else {
        return false;
    };
    context::close(context);
    sent
}

/// The coroutine `Response::upgrade`'s handshake.
/// The handshake response's `permessage-deflate` answer, when built with zlib
/// and offered: the request asked for it, so the response says so and the
/// connection compresses.
pub(crate) fn accept(context: *mut Context) {
    if !ZLIB.load(Relaxed) {
        return;
    }
    let state = send::state(context);
    let offered = if context::co_socket(state) {
        state.websocket_settings & SETTING_COMPRESSION != 0
    } else {
        // SAFETY: the context's live server.
        unsafe { view::config(state.private_data) }.websocket_compression
    };
    let mut accepted = false;
    if offered && !state.request.zobject.ptr.is_null() {
        let name = b"sec-websocket-extensions";
        // SAFETY: the request's live `header` array.
        let table = unsafe { value::table(state.request.zheader.ptr) };
        // SAFETY: a live table and the name's bytes.
        let value = unsafe {
            swoole_rs_zend::sys::zend_hash_str_find(table, name.as_ptr().cast(), name.len())
        };
        // SAFETY: a found slot is a live zval.
        if !value.is_null() && unsafe { type_of(value) } == IS_STRING {
            // SAFETY: a string zval.
            let text = unsafe { value::str_of(value) };
            // SAFETY: a live string.
            let text = unsafe { bytes(text) };
            let first = text.split(|&byte| byte == b';').next().unwrap_or_default();
            if first == b"permessage-deflate" {
                accepted = true;
                let mut offer = zval_of(string::copy(
                    b"permessage-deflate; client_no_context_takeover; server_no_context_takeover",
                ));
                crate::response::set_header(
                    context,
                    b"Sec-Websocket-Extensions",
                    &raw mut offer,
                    false,
                );
                // SAFETY: the reference made here; the header array took its own.
                unsafe { zval_ptr_dtor(&raw mut offer) };
            }
        }
    }
    let state = send::state(context);
    state.websocket_compression = u8::from(accepted);
    if !crate::context::co_socket(state)
        // SAFETY: the server the context is bound to.
        && let Some(conn) = unsafe { view::verified(state.private_data, state.fd) }
    {
        conn.websocket_compression = u8::from(accepted);
    }
}

/// After a server handshake response: the connection is a WebSocket on 101,
/// otherwise the handshake failed and the connection closes.
pub(crate) fn handshaked(context: *mut Context) {
    let state = send::state(context);
    let (server, fd) = (state.private_data, state.fd);
    let Some(conn) = connection(server, fd, true) else {
        return;
    };
    if i64::from(conn.status) != STATUS_HANDSHAKE {
        return;
    }
    let state = send::state(context);
    let status = if state.response.status == 101 {
        STATUS_ACTIVE
    } else {
        state.keepalive = 0;
        STATUS_HANDSHAKE_FAILED
    };
    set_status(server, fd, status);
}

pub(crate) fn upgrade(context: *mut Context) -> Result<bool, bailout::Bailout> {
    handshake(context)
}

/// `Coroutine\Http\Client`'s `WebSocketSettings` and the message it is
/// assembling.
struct ClientWebSocket {
    /// `OPEN_*_FRAME` bits for [`recv`].
    frames: u32,
    mask: bool,
    compression: bool,
    message: Assembly,
}

/// A client's WebSocket state, as `WebSocketSettings` starts.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_http_client_coro_ws_new() -> *mut c_void {
    let state = ClientWebSocket {
        frames: 0,
        mask: true,
        compression: false,
        message: None,
    };
    Box::into_raw(Box::new(state)).cast()
}

/// # Safety
/// `ws` is live from `swoole_http_client_coro_ws_new`, freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_http_client_coro_ws_free(ws: *mut c_void) {
    // SAFETY: per the caller.
    drop(unsafe { Box::from_raw(ws.cast::<ClientWebSocket>()) });
}

/// # Safety
/// As [`swoole_http_client_coro_ws_free`], but live.
unsafe fn client<'a>(ws: *mut c_void) -> &'a mut ClientWebSocket {
    // SAFETY: per the caller.
    unsafe { &mut *ws.cast::<ClientWebSocket>() }
}

/// `WebSocket::apply_setting(settings, vht, false)`.
/// # Safety
/// `ws` is a live client state and `settings` an array zval.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_http_client_coro_ws_apply(ws: *mut c_void, settings: *const zval) {
    // SAFETY: per the caller.
    let (state, table) = (unsafe { client(ws) }, unsafe { value::table(settings) });
    state.mask = setting(table, b"websocket_mask").unwrap_or(true);
    if ZLIB.load(Relaxed)
        && let Some(on) = setting(table, b"websocket_compression")
    {
        state.compression = on;
    }
    for (key, bit) in [
        (&b"open_websocket_close_frame"[..], OPEN_CLOSE_FRAME),
        (b"open_websocket_ping_frame", OPEN_PING_FRAME),
        (b"open_websocket_pong_frame", OPEN_PONG_FRAME),
    ] {
        if let Some(on) = setting(table, key) {
            state.frames = if on {
                state.frames | bit
            } else {
                state.frames & !bit
            };
        }
    }
}

/// # Safety
/// `ws` is a live client state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_http_client_coro_ws_compression(ws: *mut c_void) -> bool {
    // SAFETY: per the caller.
    unsafe { client(ws) }.compression
}

/// Pack a frame and send it: 0, 1 if packing failed, 2 if sending did.
/// # Safety
/// `socket` and `ws` are live; `data` is a live zval.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_http_client_coro_ws_push(
    socket: *mut c_void,
    ws: *mut c_void,
    data: *mut zval,
    opcode: i64,
    flags: c_int,
    code: i64,
    compress: bool,
) -> c_int {
    let mut frame = FrameObject::new(data, opcode, i64::from(byte(i64::from(flags))), code);
    // SAFETY: per the caller.
    if unsafe { client(ws) }.mask {
        frame.flags |= FLAG_MASK;
    }
    if compress {
        frame.flags |= FLAG_COMPRESS;
    }
    let sent = pack(&frame).map_or(1, |packed| {
        let packed = packed.bytes();
        // SAFETY: the live socket and the packed bytes.
        let written = unsafe { co::send_all(socket, packed.as_ptr().cast(), packed.len()) };
        if usize::try_from(written).is_ok_and(|written| written == packed.len()) {
            0
        } else {
            2
        }
    });
    bailout::finish();
    sent
}

/// `websocket::recv_frame` on the client's socket.
/// # Safety
/// `socket` and `ws` are live; `return_value` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_http_client_coro_ws_recv(
    socket: *mut c_void,
    ws: *mut c_void,
    return_value: *mut zval,
    timeout: f64,
) {
    // SAFETY: per the caller.
    let state = unsafe { client(ws) };
    recv(
        state.frames,
        &mut state.message,
        socket,
        return_value,
        timeout,
    );
    bailout::finish();
}

/// `WebSocketSettings`' bits for [`recv`], and with `compression` a
/// coroutine server's `websocket_settings`.
pub(crate) const IN_SERVER: u32 = 1;
pub(crate) const OPEN_PING_FRAME: u32 = 1 << 1;
pub(crate) const OPEN_PONG_FRAME: u32 = 1 << 2;
pub(crate) const OPEN_CLOSE_FRAME: u32 = 1 << 3;
pub(crate) const SETTING_COMPRESSION: u8 = 1 << 4;
/// `SW_WEBSOCKET_DEFAULT_BUFFER`.
const DEFAULT_BUFFER: usize = 4096;

/// `sock->recv_packet(timeout)` on a coroutine socket, with the packet: its
/// read buffer's bytes.
fn sock_recv(sock: *mut c_void, timeout: f64) -> (isize, *mut u8, usize) {
    // SAFETY: a live `SocketImpl *`, in a coroutine.
    let received = unsafe { co::recv_packet(sock, timeout) };
    if received <= 0 {
        return (received, ptr::null_mut(), 0);
    }
    // SAFETY: as above.
    let buffer = unsafe { co::record(sock) }.read_buffer.cast::<CppString>();
    // SAFETY: a packet read leaves it in the socket's read buffer.
    let buffer = unsafe { &*buffer };
    (received, buffer.str, buffer.length)
}

/// `sock->get_fd()`.
pub(crate) fn sock_fd(sock: *mut c_void) -> c_int {
    // SAFETY: a live `SocketImpl *`.
    unsafe { co::record(sock) }.sock_fd
}

/// A message being assembled from fragments: its payload so far, and the
/// first frame's opcode and flags (`(opcode << 8) | flags`).
pub(crate) struct Message {
    data: Vec<u8>,
    ext_flags: u16,
}

/// Where a message being assembled lives: a context's `ws_buffer`, or a
/// client's `continue_frame_buffer` (null when none).
pub(crate) type Assembly = Option<Box<Message>>;

/// `websocket::send_frame`: answer a control frame the user did not ask for.
fn reply(settings: u32, sock: *mut c_void, opcode: u8, payload: &[u8]) {
    let flags = if settings & IN_SERVER != 0 {
        FLAG_FIN
    } else {
        FLAG_FIN | FLAG_MASK
    };
    if let Some(frame) = encode(payload, opcode, flags) {
        let frame = frame.bytes();
        // SAFETY: the live socket and the encoded frame.
        unsafe { co::send(sock, frame.as_ptr().cast(), frame.len()) };
    }
}

/// A frame object for `payload` in `return_value`, with the socket's fd.
fn deliver(return_value: *mut zval, sock: *mut c_void, opcode: u8, payload: &mut zval, flags: u8) {
    construct(return_value, zend_long::from(opcode), payload, flags);
    // SAFETY: the frame took its own reference.
    unsafe { zval_ptr_dtor(payload) };
    let fd = sock_fd(sock);
    // SAFETY: the frame just created.
    let object = unsafe { obj_of(return_value) };
    // SAFETY: a live Frame, which declares `fd`.
    unsafe { value::update_long(FRAME.entry(), object, c"fd", zend_long::from(fd)) };
}

fn bytes_of<'a>(data: *const u8, length: usize) -> &'a [u8] {
    if length == 0 || data.is_null() {
        return &[];
    }
    // SAFETY: per every caller, `length` readable bytes.
    unsafe { std::slice::from_raw_parts(data, length) }
}

fn set_type(target: *mut zval, kind: u32) {
    // SAFETY: a writable zval; these types carry no value.
    unsafe { (*target).u1 = _zval_struct__bindgen_ty_1 { type_info: kind } };
}

fn protocol_error(return_value: *mut zval) {
    host::set_last_error(PROTOCOL_ERROR);
    set_type(return_value, IS_NULL);
}

/// The payload as a string zval, inflated when `compressed`.
fn payload_zval(payload: &[u8], compressed: bool) -> Option<zval> {
    if !compressed {
        return Some(zval_of(string::copy(payload)));
    }
    let mut out = new_zval();
    uncompress(&raw mut out, payload).then_some(out)
}

/// `websocket::recv_frame`: the next whole message from a coroutine socket as
/// a frame object, answering pings and closes the settings leave to it.
/// `false` when the socket read failed, `""` when it closed, `null` on any
/// other error.
pub(crate) fn recv(
    settings: u32,
    slot: &mut Assembly,
    sock: *mut c_void,
    return_value: *mut zval,
    timeout: f64,
) {
    let func = c"websocket::recv_frame";
    loop {
        let (received, data, length) = sock_recv(sock, timeout);
        if received < 0 {
            set_type(return_value, IS_FALSE);
            return;
        }
        if received == 0 {
            // SAFETY: the empty interned string.
            unsafe { swr_zval_str(return_value, empty()) };
            return;
        }
        if length < 2 {
            protocol_error(return_value);
            return;
        }
        // SAFETY: the socket's read buffer, `length` bytes, unmasked in place.
        let buffer = unsafe { std::slice::from_raw_parts_mut(data, length) };
        let Some(frame) = decode(buffer) else {
            protocol_error(return_value);
            return;
        };
        let payload = bytes_of(
            buffer.as_ptr().wrapping_add(frame.header_length),
            frame.payload_length,
        );
        let opcode = frame.opcode;
        let respond = match opcode {
            OPCODE_PING if settings & OPEN_PING_FRAME == 0 => {
                reply(settings, sock, OPCODE_PONG, payload);
                continue;
            }
            OPCODE_PONG if settings & OPEN_PONG_FRAME == 0 => continue,
            OPCODE_CLOSE if settings & OPEN_CLOSE_FRAME == 0 => {
                reply(settings, sock, OPCODE_CLOSE, payload);
                continue;
            }
            OPCODE_PING | OPCODE_PONG | OPCODE_CLOSE => true,
            _ => false,
        };
        if respond {
            let mut zpayload = zval_of(string::copy(payload));
            deliver(return_value, sock, opcode, &mut zpayload, FLAG_FIN);
            return;
        }
        if opcode == OPCODE_CONTINUATION {
            let Some(message) = slot.as_mut() else {
                warning(
                    func,
                    "A continuation frame cannot stand alone and MUST be preceded by an initial frame whose \
                     opcode indicates either text or binary data.",
                );
                set_type(return_value, IS_NULL);
                return;
            };
            message.data.extend_from_slice(payload);
            if frame.flags & FLAG_FIN == 0 {
                continue;
            }
            // The whole message is assembled: release its memory once delivered.
            let Some(message) = slot.take() else { return };
            let [complete_opcode, complete_flags] = message.ext_flags.to_be_bytes();
            let Some(mut zpayload) = payload_zval(&message.data, complete_flags & FLAG_RSV1 != 0)
            else {
                protocol_error(return_value);
                return;
            };
            deliver(
                return_value,
                sock,
                complete_opcode,
                &mut zpayload,
                complete_flags | FLAG_FIN,
            );
            return;
        }
        if slot.is_some() {
            warning(
                func,
                "All fragments of a message, except for the initial frame, must use the continuation \
                 frame opcode(0).",
            );
            set_type(return_value, IS_NULL);
            return;
        }
        if frame.flags & FLAG_FIN != 0 {
            let Some(mut zpayload) = payload_zval(payload, frame.flags & FLAG_RSV1 != 0) else {
                protocol_error(return_value);
                return;
            };
            deliver(return_value, sock, opcode, &mut zpayload, frame.flags);
            return;
        }
        let size = if payload.is_empty() {
            DEFAULT_BUFFER
        } else {
            payload.len()
        };
        let mut data = Vec::with_capacity(size);
        data.extend_from_slice(payload);
        *slot = Some(Box::new(Message {
            data,
            ext_flags: u16::from_be_bytes([opcode, frame.flags]),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_object_matches_the_cxx_layout() {
        // `struct FrameObject { uint8_t opcode; uint8_t flags; uint16_t code; zval *data; }`
        assert_eq!(size_of::<FrameObject>(), 16, "size");
        assert_eq!(std::mem::offset_of!(FrameObject, data), 8, "data");
    }

    #[test]
    fn bytes_truncate_like_uint8_t() {
        assert_eq!(byte(0x1_09), 9, "low byte");
        assert_eq!(byte(-1), 255, "two's complement");
    }

    #[test]
    fn a_masked_header_only_frame_fails() {
        assert!(
            header_only_mask(FLAG_MASK | FLAG_ENCODE_HEADER_ONLY),
            "both"
        );
        assert!(!header_only_mask(FLAG_MASK), "mask only");
    }
}
