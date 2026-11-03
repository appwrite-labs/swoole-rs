//! `Swoole\WebSocket\Server`, `Frame` and `CloseFrame`: registration and
//! constants, frames (packing, unpacking, permessage-deflate), the server's
//! push family, the handshake and the `onOpen`/`onMessage` dispatch. C++ keeps
//! the Server and its connections and answers through `swr_ws_host_*`
//! (`ext-src/swoole_websocket_server.cc`); PHP callbacks run there, inside
//! `zend_try` (docs/zend.md).
use crate::{
    host::{self, CO_SOCKET, Context, swr_http_host_close, swr_http_host_send},
    send,
};
use std::{
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicBool, Ordering::Relaxed},
};
use swoole_rs_phpstubs::websocket as stubs;
use swoole_rs_websocket::{
    DecodedFrame, INVALID_CONTROL, swr_v1_ws_control_invalid, swr_v1_ws_decode, swr_v1_ws_encode,
    swr_v1_ws_encode_close, swr_v1_ws_encoded_size, swr_v1_ws_message_policy,
    swr_v1_ws_pack_policy,
};
use swoole_rs_zend::{
    Call, Class, bailout, declare_long, declare_property_null, declare_string, handlers,
    init_object, method_table, register_long_constant,
    string::{self, bytes},
    sys::{
        IS_NULL, IS_OBJECT, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, ZEND_ACC_STATIC,
        swr_zend_instanceof, swr_zend_property_slot, swr_zend_string_release,
        swr_zend_ws_property_not_null, swr_zval_is_true, swr_zval_str, zend_ce_stringable,
        zend_class_entry, zend_class_implements, zend_long, zend_object, zend_string,
        zend_update_property, zval, zval_ptr_dtor,
    },
    type_of,
    value::{self, get_long, get_string, obj_of},
};

static SERVER: Class = Class::new();
static FRAME: Class = Class::new();
static CLOSE_FRAME: Class = Class::new();
/// `SW_HAVE_ZLIB`.
static ZLIB: AtomicBool = AtomicBool::new(false);

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

/// `swr_ws_host_call`'s events and results.
const ON_BEFORE_HANDSHAKE_RESPONSE: c_int = 0;
const ON_OPEN: c_int = 1;
const CALL_FAILED: c_int = 1;
const CALL_BAILED: c_int = -1;

/// `SW_OK`/`SW_ERR`.
const OK: c_int = 0;
/// `swr_v1_phphttp_websocket_message`: native code calls `onMessage`.
const MESSAGE_DISPATCH: c_int = 1;
const ERR: c_int = -1;

/// `swoole::Server`, opaque.
type Server = c_void;

/// `swr_ws_connection`: what the methods read of a `swoole::Connection`.
#[repr(C)]
#[derive(Default)]
struct Connection {
    status: c_int,
    server_fd: c_int,
    compression: bool,
    closed: bool,
}

unsafe extern "C" {
    pub(crate) fn swr_ws_host_server(object: *mut zval, server: *mut *mut Server) -> bool;
    pub(crate) fn swr_ws_host_started(server: *mut Server) -> bool;
    fn swr_ws_host_connection(
        server: *mut Server,
        fd: zend_long,
        verify: bool,
        out: *mut Connection,
    ) -> bool;
    fn swr_ws_host_set_status(server: *mut Server, fd: zend_long, status: i64);
    fn swr_ws_host_send(
        server: *mut Server,
        fd: zend_long,
        data: *const c_char,
        length: usize,
        wait: bool,
    ) -> bool;
    fn swr_ws_host_close(server: *mut Server, fd: zend_long, reset: bool) -> bool;
    fn swr_ws_host_port_settings(server: *mut Server, fd: zend_long) -> c_int;
    fn swr_ws_host_protocol(
        server: *mut Server,
        server_fd: c_int,
        length: *mut usize,
    ) -> *const c_char;
    fn swr_ws_host_recv_data(server: *mut Server, request: *mut c_void, data: *mut zval);
    fn swr_ws_host_call(
        server: *mut Server,
        server_fd: c_int,
        event: c_int,
        argc: u32,
        args: *mut zval,
    ) -> c_int;
    fn swr_ws_host_server_zval(server: *mut Server) -> *mut zval;
    fn swr_ws_host_context_server(context: *mut Context) -> *mut Server;
    fn swr_ws_host_context_compression(context: *mut Context) -> bool;
    fn swr_ws_host_co_protocol(context: *mut Context);
    fn swr_ws_host_set_header(
        context: *mut Context,
        key: *const c_char,
        key_length: usize,
        value: *const c_char,
        value_length: usize,
    ) -> bool;
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
/// `php_swoole_websocket_server_minit` registered them; the class entries go
/// back to C++'s globals.
/// # Safety
/// Called once, at MINIT, after `Swoole\Http\Server` is registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_websocket_minit(
    module_number: c_int,
    http_server: *mut zend_class_entry,
    serializable: bool,
    zlib: bool,
    frame: *mut *mut zend_class_entry,
) -> *mut zend_class_entry {
    ZLIB.store(zlib, Relaxed);
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
    // SAFETY: the caller's slot for the Frame class.
    unsafe { *frame = ce };
    SERVER.entry()
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
    let slot = match name.to_bytes() {
        b"code" => 0,
        b"reason" => 1,
        b"data" => 2,
        _ => 3,
    };
    let rv = RV.with(|rv| rv[slot].get());
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
fn pack(frame: &FrameObject) -> Option<Str> {
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

/// A string of `size` bytes `write` fills; `None` (and freed) unless it
/// wrote exactly `size`.
fn encoded(size: usize, write: impl FnOnce(*mut u8) -> usize) -> Option<Str> {
    if size == 0 {
        return None;
    }
    let out = string::alloc(size);
    if write(string::val(out)) != size {
        // SAFETY: the fresh string nothing else references.
        unsafe { string::free(out) };
        return None;
    }
    // SAFETY: as above, now holding `size` bytes.
    unsafe { string::truncate(out, size) };
    Some(Str(out))
}

/// Upstream's encoders fail a masked header-only frame after writing it.
fn header_only_mask(flags: u8) -> bool {
    flags & FLAG_MASK != 0 && flags & FLAG_ENCODE_HEADER_ONLY != 0
}

/// `websocket::encode`.
pub(crate) fn encode(data: &[u8], opcode: u8, flags: u8) -> Option<Str> {
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
    let frame = encoded(size, |out| {
        // SAFETY: `data` and a fresh `size`-byte output.
        unsafe { swr_v1_ws_encode(data.as_ptr(), data.len(), opcode, flags, out, size) }
    })?;
    (!header_only_mask(flags)).then_some(frame)
}

/// `websocket::pack_close_frame`.
fn close_frame(code: i32, reason: &[u8], flags: u8) -> Option<Str> {
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
    let frame = encoded(size, |out| {
        // SAFETY: the reason and a fresh `size`-byte output.
        unsafe { swr_v1_ws_encode_close(code, reason.as_ptr(), reason.len(), flags, out, size) }
    })?;
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
        if data.len() >= 2 {
            // `(payload[0] << 8) ^ (payload[1] & 0xFF)` over a signed char.
            let code = (i64::from(data[0].cast_signed()) << 8) ^ i64::from(data[1]);
            // SAFETY: a live CloseFrame.
            unsafe { value::update_long(class, object, c"code", code) };
            if data.len() > 2 {
                let reason = string::copy(&data[2..]);
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
fn zval_of(string: *mut zend_string) -> zval {
    // SAFETY: a zval is plain data.
    let mut out: zval = unsafe { std::mem::zeroed() };
    // SAFETY: an empty slot takes the reference.
    unsafe { swr_zval_str(&raw mut out, string) };
    out
}

/// `swoole_websocket_server_pack`'s body: the packed frame, or `""`.
fn pack_value(call: &Call, data: *mut zval, opcode: i64, flags: i64) {
    match pack(&FrameObject::new(data, opcode, flags, 0)) {
        // SAFETY: the return slot takes the new string.
        Some(packed) => unsafe { call.set_string(packed.take()) },
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
    let mut frame = DecodedFrame::default();
    // SAFETY: the caller's writable bytes and a local output.
    let total = unsafe { swr_v1_ws_decode(data.as_mut_ptr(), data.len(), &raw mut frame) };
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
    let payload = &data[frame.header_length..frame.header_length + frame.payload_length];
    // SAFETY: a zval is plain data.
    let mut zpayload: zval = unsafe { std::mem::zeroed() };
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
    let mut server = ptr::null_mut();
    // SAFETY: `$this` is a server object; C++ traps the E_ERROR of a dead one.
    if unsafe { swr_ws_host_server(call.this_zval(), &raw mut server) } {
        bailout::Bailout.defer();
        return None;
    }
    // SAFETY: a live server.
    if !unsafe { swr_ws_host_started(server) } {
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
    let mut out = Connection::default();
    // SAFETY: a live server and a local output.
    unsafe { swr_ws_host_connection(server, fd, verify, &raw mut out) }.then_some(out)
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
    // SAFETY: a live server and the frame's bytes.
    unsafe { swr_ws_host_send(server, fd, frame.as_ptr().cast(), frame.len(), true) }
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
        // SAFETY: a live server.
        unsafe { swr_ws_host_set_status(server, fd, STATUS_CLOSING) };
        // SAFETY: as above.
        Some(unsafe { swr_ws_host_close(server, fd, false) })
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
    // SAFETY: a live server and the arguments.
    let called = unsafe {
        swr_ws_host_call(
            server,
            server_fd,
            event,
            u32::try_from(args.len()).unwrap_or(0),
            args.as_mut_ptr(),
        )
    };
    match called {
        CALL_BAILED => return Err(bailout::Bailout),
        CALL_FAILED => {
            let name = match event {
                ON_BEFORE_HANDSHAKE_RESPONSE => "onBeforeHandshakeResponse",
                ON_OPEN => "onOpen",
                _ => "onMessage",
            };
            host::notice(&format!("Swoole\\WebSocket\\Server->{name} handler error"));
            // SAFETY: a live server.
            unsafe { swr_ws_host_close(server, fd, false) };
        }
        _ => {}
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
    let object = unsafe { swr_ws_host_server_zval(server) };
    // SAFETY: the server's live zval.
    unsafe { *object }
}

/// `swoole_websocket_onMessage` up to the `onMessage` call: a frame
/// `Server::dispatch_frame` delivered. Returns `MESSAGE_DISPATCH` with the server
/// and the Frame in `args` and the payload reference in `data`; native code then
/// calls `onMessage` itself, so a bailout in it crosses no Rust frame, and hands
/// the outcome to [`swr_v1_phphttp_websocket_message_done`]. Otherwise the
/// handler's `SW_OK`/`SW_ERR`.
/// # Safety
/// `server` and `request` are the live server and its `RecvData`; `args` has room
/// for two zvals and `data` for one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_websocket_message(
    server: *mut c_void,
    request: *mut c_void,
    fd: i64,
    ext_flags: u16,
    args: *mut zval,
    data: *mut zval,
) -> c_int {
    let result = message(server, request, fd, ext_flags, args, data);
    bailout::finish();
    result
}

/// After `onMessage`: a failed call warns and closes `fd`; then the references
/// [`swr_v1_phphttp_websocket_message`] made are released.
/// # Safety
/// `server` is live; `args` and `data` are what `swr_v1_phphttp_websocket_message`
/// filled for this frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_websocket_message_done(
    server: *mut c_void,
    fd: i64,
    failed: bool,
    args: *mut zval,
    data: *mut zval,
) {
    if failed {
        host::notice("Swoole\\WebSocket\\Server->onMessage handler error");
        // SAFETY: a live server.
        unsafe { swr_ws_host_close(server.cast(), fd, false) };
    }
    // SAFETY: the payload reference `message` made.
    unsafe { zval_ptr_dtor(data) };
    // SAFETY: `args` holds two zvals.
    let frame = unsafe { args.add(1) };
    // SAFETY: the Frame `message` made.
    unsafe { zval_ptr_dtor(frame) };
}

fn message(
    server: *mut Server,
    request: *mut c_void,
    fd: zend_long,
    ext_flags: u16,
    args: *mut zval,
    out: *mut zval,
) -> c_int {
    // SAFETY: a live server.
    let Ok(settings) = u8::try_from(unsafe { swr_ws_host_port_settings(server, fd) }) else {
        return ERR;
    };
    // SAFETY: a zval is plain data.
    let mut data: zval = unsafe { std::mem::zeroed() };
    // SAFETY: the live request and an empty slot.
    unsafe { swr_ws_host_recv_data(server, request, &raw mut data) };
    let [opcode, flags] = ext_flags.to_be_bytes();
    let delivery = swr_v1_ws_message_policy(opcode, settings);
    if delivery != 0 {
        if delivery == 2 {
            let payload = string_bytes(&data);
            if let Some(pong) = encode(payload, OPCODE_PONG, FLAG_FIN) {
                let pong = pong.bytes();
                // SAFETY: a live server and the frame's bytes.
                unsafe { swr_ws_host_send(server, fd, pong.as_ptr().cast(), pong.len(), false) };
            }
        }
        // SAFETY: the reference recv_data made.
        unsafe { zval_ptr_dtor(&raw mut data) };
        return OK;
    }
    // RFC 7692: the payload is deflated.
    if flags & FLAG_RSV1 != 0 {
        // SAFETY: a zval is plain data.
        let mut plain: zval = unsafe { std::mem::zeroed() };
        let ok = uncompress(&raw mut plain, string_bytes(&data));
        // SAFETY: the reference recv_data made.
        unsafe { zval_ptr_dtor(&raw mut data) };
        if !ok {
            return OK;
        }
        data = plain;
    }
    // SAFETY: the caller's room for two zvals.
    unsafe { args.write(server_zval(server)) };
    // SAFETY: as above, the second.
    let frame_zval = unsafe { args.add(1) };
    construct(frame_zval, zend_long::from(opcode), &raw mut data, flags);
    // SAFETY: the Frame just built.
    let frame = unsafe { obj_of(frame_zval) };
    // SAFETY: as above.
    unsafe { value::update_long(FRAME.entry(), frame, c"fd", fd) };
    // SAFETY: the caller's room for one zval; it now owns the payload reference.
    unsafe { out.write(data) };
    MESSAGE_DISPATCH
}

/// `swoole_websocket_handshake`, and for the async server (`open`) what
/// `swoole_websocket_onHandshake` does after it: `onOpen`, or a reset.
/// # Safety
/// `context` is a live `swoole::http::Context`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_websocket_handshake(
    context: *mut c_void,
    open: bool,
) -> bool {
    let result = handshake(context).and_then(|done| {
        if open {
            // SAFETY: a live server context.
            let server = unsafe { swr_ws_host_context_server(context) };
            let fd = host::fd(context);
            if done {
                on_open(server, context)?;
            } else {
                // SAFETY: a live server.
                unsafe { swr_ws_host_close(server, fd, true) };
            }
        }
        Ok(done)
    });
    let done = matches!(result, Ok(true));
    bailout::raise(result.map(|_| ()));
    done
}

/// Set a response header as `Context::set_header(..., false)`.
fn set_header(context: *mut Context, key: &[u8], value: &[u8]) -> Result<(), bailout::Bailout> {
    // SAFETY: a live context; C++ copies both and traps a bailout.
    let bailed = unsafe {
        swr_ws_host_set_header(
            context,
            key.as_ptr().cast(),
            key.len(),
            value.as_ptr().cast(),
            value.len(),
        )
    };
    if bailed {
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
        // SAFETY: a live server context.
        let server = unsafe { swr_ws_host_context_server(context) };
        let Some(conn) = connection(server, fd, false) else {
            closed(c"swoole_websocket_handshake", fd);
            return Ok(false);
        };
        // SAFETY: a live server.
        unsafe { swr_ws_host_set_status(server, fd, STATUS_ACTIVE) };
        let mut length = 0;
        // SAFETY: as above.
        let protocol = unsafe { swr_ws_host_protocol(server, conn.server_fd, &raw mut length) };
        if !protocol.is_null() && length > 0 {
            // SAFETY: the port's protocol setting, `length` bytes.
            let protocol = unsafe { std::slice::from_raw_parts(protocol.cast::<u8>(), length) };
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
        // SAFETY: a live coroutine-socket context.
        unsafe { swr_ws_host_co_protocol(context) };
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
/// # Safety
/// `context` is a live server context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_websocket_request(context: *mut c_void) {
    const BAD_REQUEST: &[u8] = b"HTTP/1.1 400 Bad Request\r\n\
        Connection: close\r\n\
        Content-Type: text/html; charset=UTF-8\r\n\
        Cache-Control: must-revalidate,no-cache,no-store\r\n\
        Content-Length: 83\r\n\
        Server: swoole-http-server\r\n\r\n\
        <html><body><h2>HTTP 400 Bad Request</h2><hr><i>Powered by Swoole</i></body></html>";
    // SAFETY: a live context and the response's bytes.
    unsafe { swr_http_host_send(context, BAD_REQUEST.as_ptr().cast(), BAD_REQUEST.len()) };
    send::state(context).end = 1;
    // SAFETY: a live context.
    unsafe { swr_http_host_close(context) };
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
    // SAFETY: a live context.
    if unsafe { swr_ws_host_context_compression(context) } {
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
    // SAFETY: a live context and the frame's bytes.
    Some(unsafe { swr_http_host_send(context, packed.as_ptr().cast(), packed.len()) })
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
    // SAFETY: a live context.
    unsafe { swr_http_host_close(context) };
    sent
}

/// The coroutine `Response::upgrade`'s handshake.
pub(crate) fn upgrade(context: *mut Context) -> Result<bool, bailout::Bailout> {
    handshake(context)
}

/// `websocket::construct_frame` for C++ (the coroutine `recv`).
/// # Safety
/// `frame` is a writable slot and `payload` a live string zval.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_websocket_frame(
    frame: *mut zval,
    opcode: i64,
    payload: *mut zval,
    flags: u8,
) {
    construct(frame, opcode, payload, flags);
}

/// `FrameObject`'s constructor for C++.
/// # Safety
/// `object` is writable and `data` a live zval.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_websocket_frame_object(
    object: *mut FrameObject,
    data: *mut zval,
    opcode: i64,
    flags: i64,
    code: i64,
) {
    let frame = FrameObject::new(data, opcode, flags, code);
    // SAFETY: per the caller.
    unsafe { object.write(frame) };
    bailout::finish();
}

/// `FrameObject::pack` for C++: the frame as a new string, or null.
/// # Safety
/// `object` is a frame `swr_v1_phphttp_websocket_frame_object` built.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_websocket_pack(
    object: *const FrameObject,
) -> *mut zend_string {
    // SAFETY: per the caller.
    let packed = pack(unsafe { &*object }).map_or(ptr::null_mut(), Str::take);
    bailout::finish();
    packed
}

/// `FrameObject::uncompress` for C++.
/// # Safety
/// `payload` is an empty slot and `data` covers `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_websocket_uncompress(
    payload: *mut zval,
    data: *const c_char,
    length: usize,
) -> bool {
    let data = if length == 0 {
        &[][..]
    } else {
        // SAFETY: per the caller.
        unsafe { std::slice::from_raw_parts(data.cast(), length) }
    };
    let done = uncompress(payload, data);
    bailout::finish();
    done
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
