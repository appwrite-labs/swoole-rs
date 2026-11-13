//! `Swoole\WebSocket\Server`, `Frame` and `CloseFrame`: registration and
//! constants, frames (packing, unpacking, permessage-deflate), the server's
//! push family, the handshake and the `onOpen`/`onMessage` dispatch, over the
//! server's Rust-owned connections and ports (crates/server), and
//! `Coroutine\Http\Client`'s WebSocket state.
use crate::server::new_zval;
use crate::{
    context::{self, Ctx},
    host::{self, CO_SOCKET},
    send,
};
use std::{
    borrow::Cow,
    cell::Cell,
    ffi::{CStr, c_int},
    mem::MaybeUninit,
    ptr,
    sync::Arc,
    sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed},
};
use swoole_rs_corebase::CCast;
use swoole_rs_phpruntime::co;
use swoole_rs_phpserver::{RecvData, php_swoole_get_recv_data, php_swoole_server_of, zserv};
use swoole_rs_phpstubs::websocket as stubs;
use swoole_rs_phpswoole::MinitArgs;
use swoole_rs_server::{server_methods, view};
use swoole_rs_websocket::{
    DecodedFrame, Delivery, INVALID_CONTROL, control_invalid, encode_close_into, encode_into,
    encoded_size, header, message_policy, pack_policy,
};
use swoole_rs_zend::{
    Array, Call, Class, Minit, Owned, Value, ZStr, ZString, bailout, handlers, method_table,
    string::{self},
    sys::{
        _zval_struct__bindgen_ty_1, IS_FALSE, IS_LONG, IS_NULL, IS_TRUE, ZEND_ACC_NOT_SERIALIZABLE,
        ZEND_ACC_PUBLIC, ZEND_ACC_STATIC, zend_long, zend_object, zend_value, zval, zval_ptr_dtor,
    },
};

static SERVER: Class = Class::new();
static FRAME: Class = Class::new();
static CLOSE_FRAME: Class = Class::new();
/// Frame's declared `fd`, `data`, `opcode`, `flags` and `finish`, as byte offsets
/// into a `Frame` or `CloseFrame` (which keeps its parent's slots), set in MINIT.
static SLOTS: [AtomicU32; 5] = [const { AtomicU32::new(0) }; 5];
const SLOT_NAMES: [&CStr; 5] = [c"fd", c"data", c"opcode", c"flags", c"finish"];
const FD: usize = 0;
const DATA: usize = 1;
const OPCODE: usize = 2;
const FLAGS: usize = 3;
const FINISH: usize = 4;

/// Store `value` in the `which` declared property of a Frame just created, as
/// `zend_update_property` does once it has found the slot by name: Frame's
/// properties are untyped and it has no write handler. The default it replaces
/// (a number, null or the interned "") needs no release.
fn set(frame: *mut zend_object, which: usize, value: zval) {
    let offset = SLOTS.get(which).map_or(0, |slot| slot.load(Relaxed));
    let slot = frame.wrapping_byte_add(offset as usize).cast::<zval>();
    // SAFETY: every caller's fresh Frame, whose declared slot this is.
    unsafe { slot.write(value) };
}

/// A long (`IS_LONG`), `true` or `false` zval.
fn scalar(type_info: u32, lval: zend_long) -> zval {
    let mut value = new_zval();
    value.value = zend_value { lval };
    value.u1 = _zval_struct__bindgen_ty_1 { type_info };
    value
}

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
const LOG_TRACE: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_TRACE as c_int;
const LOG_WARNING: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as c_int;

/// The callbacks the handshake and frames call (`SW_SERVER_CB_*`).
const ON_BEFORE_HANDSHAKE_RESPONSE: c_int = 6;
const ON_OPEN: c_int = 7;
const ON_MESSAGE: c_int = 8;

/// `SW_OK`/`SW_ERR`.
const OK: c_int = swoole_rs_corebase::constants::ResultCode::SW_OK as c_int;
const ERR: c_int = swoole_rs_corebase::constants::ResultCode::SW_ERR as c_int;

/// `Swoole\WebSocket\Server`, once MINIT registered it; crates/phpserver
/// tells a WebSocket server by it.
#[must_use]
pub fn websocket_class() -> &'static Class {
    &SERVER
}

/// What the methods read of a `swoole::Connection`.
#[derive(Clone, Copy)]
struct Connection {
    status: u8,
    server_fd: c_int,
    compression: bool,
    closed: bool,
}

/// `php_swoole_server_get_and_check_server(object)`: `None` after the
/// `E_ERROR` of a server object never constructed. The view lives while
/// `_held`, which holds the object (the method's frame, or the value), is
/// borrowed.
pub(crate) fn server_of(object: &Value) -> Option<view::Server<'_>> {
    let server = php_swoole_server_of(object);
    if server.is_none() {
        let name = object.class_name().map(CStr::to_string_lossy);
        let name = name.unwrap_or_default();
        let message = format!("Invalid instance of {name}");
        host::set_last_error(host::PHP_FATAL_ERROR);
        let message = std::ffi::CString::new(message).unwrap_or_default();
        bailout::defer(swoole_rs_zend::report(swoole_rs_zend::E_ERROR, &message));
    }
    server
}

/// `conn->websocket_status = status` of the session's slot.
fn set_status(server: view::Server<'_>, fd: zend_long, status: i64) {
    if let Some(conn) = server.by_session(fd) {
        conn.set_websocket_status(u8::try_from(status).unwrap_or_default());
    }
}

/// `Server::close(fd, reset)`.
fn close(server: view::Server<'_>, fd: zend_long, reset: bool) -> bool {
    server_methods::close(server, fd, reset)
}

/// The port's `open_websocket_{close,ping,pong}_frame` bits, or `None` when
/// the session is gone (`get_port_by_session_id`).
fn port_frames(server: view::Server<'_>, fd: zend_long) -> Option<u8> {
    let conn = server.by_session(fd)?;
    server
        .port_of_fd(conn.fd())
        .map(|port| port.websocket_frames.get())
}

/// The `websocket_subprotocol` of the port listening on `server_fd`.
fn port_protocol(server: view::Server<'_>, server_fd: c_int) -> Option<Arc<Vec<u8>>> {
    server
        .port_of_server_fd(server_fd)
        .and_then(|port| port.websocket_protocol.get())
}

/// A boolean setting's value, when set and not null.
pub(crate) fn setting(table: &Array, key: &[u8]) -> Option<bool> {
    table.get(key).map(Value::truthy)
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
/// After `Swoole\Http\Server` is registered.
pub fn websocket_minit(args: &MinitArgs) {
    let serializable = args.thread;
    let minit = args.minit;
    ZLIB.store(args.zlib, Relaxed);
    swoole_rs_phpclients::register_websocket(client_websocket);
    // Its objects are the parent's (no handlers of its own are installed).
    let http_server = minit.parent(&crate::server::SERVER);
    minit.register_child(
        &SERVER,
        c"Swoole\\WebSocket\\Server",
        &SERVER_METHODS.0,
        http_server,
        |entry, _| {
            if !serializable {
                entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
            }
        },
    );
    minit.register(
        &FRAME,
        c"Swoole\\WebSocket\\Frame",
        &FRAME_METHODS.0,
        |_, _| {},
    );
    minit.implements(&FRAME, minit.stringable());
    for (name, value) in [
        (c"fd", 0),
        (c"opcode", i64::from(OPCODE_TEXT)),
        (c"flags", i64::from(FLAG_FIN)),
    ] {
        if name == c"opcode" {
            // Declared in upstream's order.
            minit.property_string(&FRAME, c"data", c"", ZEND_ACC_PUBLIC);
        }
        minit.property_long(&FRAME, name, value, ZEND_ACC_PUBLIC);
    }
    minit.property_null(&FRAME, c"finish", ZEND_ACC_PUBLIC);
    for (slot, name) in SLOTS.iter().zip(SLOT_NAMES) {
        slot.store(crate::parse::property_offset(&FRAME, name), Relaxed);
    }
    let frame = minit.parent(&FRAME);
    minit.register_child(
        &CLOSE_FRAME,
        c"Swoole\\WebSocket\\CloseFrame",
        &[],
        frame,
        |_, _| {},
    );
    for (name, value) in [
        (c"opcode", i64::from(OPCODE_CLOSE)),
        (c"code", CLOSE_NORMAL),
    ] {
        minit.property_long(&CLOSE_FRAME, name, value, ZEND_ACC_PUBLIC);
    }
    minit.property_string(&CLOSE_FRAME, c"reason", c"", ZEND_ACC_PUBLIC);
    constants(minit);
}

/// The global constants, `SWOOLE_WEBSOCKET_*` then the BC `WEBSOCKET_*`, in
/// upstream's order.
fn constants(minit: &Minit) {
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
        minit.long_constant(&name, value);
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

/// `swoole::websocket::FrameObject`: an outgoing frame.
pub struct FrameObject<'a> {
    opcode: u8,
    flags: u8,
    code: u16,
    data: Option<Cow<'a, Value>>,
}

/// Truncate to a byte, as upstream's `uint8_t` fields do.
fn byte(value: i64) -> u8 {
    value.to_le_bytes()[0]
}

impl<'a> FrameObject<'a> {
    /// `FrameObject(zdata, opcode, flags, code)`: a `Frame` object's
    /// properties, or the arguments around any other value.
    fn new(data: Option<&'a Value>, opcode: i64, flags: i64, code: i64) -> Self {
        let Some(frame) = data.filter(|value| value.instance_of(&FRAME)) else {
            let code = code.c_cast::<u16>();
            return Self {
                opcode: byte(opcode),
                flags: byte(flags & FLAGS_ALL),
                code,
                data: data.map(Cow::Borrowed),
            };
        };
        // The Frame class declares both properties read here.
        let long = |name: &CStr| frame.property(name).map_or(0, Value::long);
        let opcode = byte(long(c"opcode"));
        let mut code = 0;
        let data = if opcode == OPCODE_CLOSE {
            code = not_null(frame, c"code").map_or(CLOSE_NORMAL, |code| code.long());
            not_null(frame, c"reason")
        } else {
            not_null(frame, c"data")
        }
        .map(Cow::Owned);
        let mut flags = byte(long(c"flags") & FLAGS_ALL);
        if let Some(finish) = not_null(frame, c"finish") {
            if finish.truthy() {
                flags |= FLAG_FIN;
            } else {
                flags &= !FLAG_FIN;
            }
        }
        let code = code.c_cast::<u16>();
        Self {
            opcode,
            flags,
            code,
            data,
        }
    }
}

/// `sw_zend_read_property_not_null_ex(swoole_websocket_frame_ce, ..., 1)` of
/// a Frame object: a copy of the property, or what a subclass's `__get`
/// returned, unless it is null or undefined.
fn not_null(frame: &Value, name: &CStr) -> Option<Owned> {
    if bailout::pending() {
        return None;
    }
    match frame.read_property(Some(&FRAME), name) {
        Ok(value) => value.filter(|value| value.kind() > IS_NULL),
        Err(bailout) => {
            bailout.defer();
            None
        }
    }
}

/// `"%s(): message"` from `swoole_warning` in `func`.
fn warning(func: &CStr, message: &str) {
    host::log(0, 0, func, message);
}

/// `FrameObject::pack`: the frame on the wire as a new string, or `None`.
fn pack(frame: &FrameObject) -> Option<Frame> {
    packed(frame, |payload, flags| whole(frame, payload, flags))
}

/// `frame` around its packed `payload`, in one piece.
fn whole(frame: &FrameObject, payload: &[u8], flags: u8) -> Option<Frame> {
    if frame.opcode == OPCODE_CLOSE {
        close_frame(i32::from(frame.code), payload, flags)
    } else {
        encode(payload, frame.opcode, flags)
    }
}

/// What `FrameObject::pack` puts on the wire, to `emit`: the payload, deflated when
/// the flags ask and zlib can, and the flags that leaves. `None` for a frame that
/// cannot be packed.
fn packed<T>(frame: &FrameObject, emit: impl FnOnce(&[u8], u8) -> Option<T>) -> Option<T> {
    if frame.opcode > OPCODE_PONG {
        host::fatal("the maximum value of opcode is 10");
        return None;
    }
    let data = frame.data.as_deref().filter(|data| !data.is_null());
    let data = data.map(Value::string);
    let data = data.as_deref().map_or(&[][..], ZStr::bytes);
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
    let (mut flags, deflate) = pack_policy(frame.opcode, frame.flags, data.len());
    let compressed = if !deflate {
        None
    } else if let Some(compressed) = compress(data) {
        flags |= FLAG_RSV1;
        Some(compressed)
    } else {
        flags &= !FLAG_RSV1;
        None
    };
    emit(compressed.as_deref().unwrap_or(data), flags)
}

/// `websocket::message_compress(.., Z_DEFAULT_COMPRESSION)`, warning as it did.
fn compress(data: &[u8]) -> Option<Vec<u8>> {
    use swoole_rs_deflate::{MessageFailure, message};
    match message(data, -1) {
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
    Heap(ZString),
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

    /// The frame as a string of its own.
    fn into_string(self) -> ZString {
        match self {
            Self::Inline(..) => ZString::new(self.bytes()),
            Self::Heap(string) => string,
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
    let mut out = string::ZBuf::with_capacity(size);
    // The fresh string's `size` unwritten bytes.
    if write(out.spare_capacity_mut()) != size {
        // Dropped: the string is freed.
        return None;
    }
    // SAFETY: `write` filled all `size` bytes of the capacity.
    unsafe { out.set_len(size) };
    Some(Frame::Heap(out.into_zstring()))
}

/// Upstream's encoders fail a masked header-only frame after writing it.
fn header_only_mask(flags: u8) -> bool {
    flags & FLAG_MASK != 0 && flags & FLAG_ENCODE_HEADER_ONLY != 0
}

/// `websocket::encode`.
pub(crate) fn encode(data: &[u8], opcode: u8, flags: u8) -> Option<Frame> {
    if control_invalid(opcode, flags, data.len()) {
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
    let size = encoded_size(data.len(), flags);
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
    if control_invalid(OPCODE_CLOSE, FLAG_FIN, reason.len() + 2) {
        warning(
            func,
            &format!(
                "invalid websocket control frame: opcode=8, fin=1, payload_length={}",
                reason.len() + 2
            ),
        );
        return None;
    }
    let size = encoded_size(reason.len() + 2, flags | FLAG_FIN);
    let frame = encoded(size, |out| encode_close_into(code, reason, flags, out))?;
    (!header_only_mask(flags)).then_some(frame)
}

/// `FrameObject::uncompress`: the inflated payload, as a string.
fn uncompress(data: &[u8]) -> Option<Owned> {
    use swoole_rs_inflate::{MessageFailure, error, message};
    if !ZLIB.load(Relaxed) {
        warning(
            c"FrameObject::uncompress",
            "The compressed websocket data frame is received, the `zlib` supports is required",
        );
        return None;
    }
    match message(data) {
        Ok(plain) => Some(Owned::string(ZString::new(&plain))),
        Err(MessageFailure::Init(status)) => {
            warning(
                c"websocket::message_uncompress",
                &format!(
                    "inflateInit2() failed by {}",
                    error(status).to_string_lossy()
                ),
            );
            None
        }
        Err(MessageFailure::Inflate(status)) => {
            host::fatal(&format!(
                "inflate() failed, Error: {}[{status}]",
                error(status).to_string_lossy()
            ));
            None
        }
    }
}

/// `websocket::construct_frame`: a new `Frame`, or a `CloseFrame` with the
/// close payload's code and reason.
fn construct(opcode: zend_long, payload: &Value, mut flags: u8) -> Owned {
    let close = opcode == zend_long::from(OPCODE_CLOSE);
    let class = if close { &CLOSE_FRAME } else { &FRAME };
    let frame = class.create_object();
    let Some(object) = frame.object() else {
        return frame;
    };
    if close {
        // A close payload is a string.
        if let [high, low, reason @ ..] = payload.bytes().unwrap_or_default()
            && let Some(props) = frame.props(Some(class))
        {
            // `(payload[0] << 8) ^ (payload[1] & 0xFF)` over a signed char.
            let code = (i64::from(high.cast_signed()) << 8) ^ i64::from(*low);
            props.long(c"code", code);
            if !reason.is_empty() {
                props.bytes(c"reason", reason);
            }
        }
    } else {
        // The Frame takes its own reference.
        set(object, DATA, payload.copy().into_zval());
    }
    if flags & FLAG_RSV1 != 0 {
        flags |= FLAG_COMPRESS;
    }
    set(object, OPCODE, scalar(IS_LONG, opcode));
    set(object, FLAGS, scalar(IS_LONG, i64::from(flags)));
    let finish = if flags & FLAG_FIN != 0 {
        IS_TRUE
    } else {
        IS_FALSE
    };
    set(object, FINISH, scalar(finish, 0));
    frame
}

/// A value holding `string`, or the empty string for none.
fn string_or_empty(string: Option<&ZStr>) -> Owned {
    Owned::string(string.map_or_else(ZString::empty, ZStr::to_owned))
}

/// `swoole_websocket_server_pack`'s body: the packed frame, or `""`.
fn pack_value(call: &Call, data: Option<&Value>, opcode: i64, flags: i64) {
    match pack(&FrameObject::new(data, opcode, flags, 0)) {
        Some(packed) => call.set_zstring(packed.into_string()),
        None => call.set_empty_string(),
    }
}

fn pack_method(call: &Call) {
    let parsed = call.arity(1, 3).and_then(|()| {
        Ok((
            call.value(1),
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
    pack_value(call, call.this_value(), 0, 0);
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
    let Ok(Some(string)) = call.arity(1, 1).and_then(|()| call.zstr(1)) else {
        call.set_bool(false);
        return;
    };
    let length = string.bytes().len();
    // SAFETY: the argument's string holds `length` bytes at its value; upstream
    // decodes (unmasks) it in place.
    let data = unsafe { std::slice::from_raw_parts_mut(string::val(string.as_ptr()), length) };
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
    let zpayload = if frame.flags & FLAG_RSV1 != 0 {
        let Some(plain) = uncompress(payload) else {
            host::set_last_error(PROTOCOL_ERROR);
            call.set_bool(false);
            return;
        };
        plain
    } else {
        Owned::string(ZString::new(payload))
    };
    // The Frame takes its own reference to the payload.
    call.set_owned(construct(i64::from(frame.opcode), &zpayload, frame.flags));
}

/// `php_swoole_server_get_and_check_server(ZEND_THIS)` and the running check.
fn server(call: &Call) -> Option<view::Server<'_>> {
    let server = server_of(call.this_value()?)?;
    if !server.is_started() {
        host::fatal("server is not running");
        return None;
    }
    Some(server)
}

/// `Server::get_connection_verify(fd) != nullptr`.
pub(crate) fn server_connection(server: view::Server<'_>, fd: zend_long) -> bool {
    connection(server, fd, true).is_some()
}

fn connection(server: view::Server<'_>, fd: zend_long, verify: bool) -> Option<Connection> {
    let conn = if verify {
        server.verified(fd)
    } else {
        server.by_session(fd)
    }?;
    Some(Connection {
        status: conn.websocket_status(),
        server_fd: conn.server_fd(),
        compression: conn.websocket_compression(),
        closed: conn.closed().load(Relaxed) != 0,
    })
}

/// `swoole_websocket_server_push`: a packed frame to a WebSocket connection, as its
/// `head` and the `payload` that follows it (all of it in `head`, for a small one).
fn push_frame(server: view::Server<'_>, fd: zend_long, head: &[u8], payload: &[u8]) -> bool {
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
    context::server_send_parts(server, fd, head, payload)
}

fn push(call: &Call) {
    let run = || -> Option<bool> {
        let server = server(call)?;
        call.arity(2, 4).ok()?;
        let fd = call.long(1, 0).ok()?;
        let data = call.value(2);
        let opcode = call.long(3, i64::from(OPCODE_TEXT)).ok()?;
        let flags = call
            .zval_or_null(4)
            .map_or(i64::from(FLAG_FIN), Value::long);
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
        let sent = packed(&frame, |payload, flags| {
            // A large data frame's payload is sent from where it is, behind its header.
            if payload.len() > INLINE
                && frame.opcode < OPCODE_CLOSE
                && flags & FLAG_ENCODE_HEADER_ONLY == 0
            {
                let (head, length) = header(payload.len() as u64, frame.opcode, flags);
                let head = head.get(..length).unwrap_or_default();
                return Some(push_frame(server, fd, head, payload));
            }
            let frame = whole(&frame, payload, flags)?;
            Some(push_frame(server, fd, frame.bytes(), &[]))
        });
        if sent.is_none() {
            host::set_last_error(WEBSOCKET_PACK_FAILED);
        }
        sent
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
        let frame = close_frame(code.c_cast::<i32>(), reason, 0)?;
        if !push_frame(server, fd, frame.bytes(), &[]) {
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
        let data = string_or_empty(call.zstr(2).ok()?);
        let frame = FrameObject::new(Some(&data), i64::from(OPCODE_PING), i64::from(FLAG_FIN), 0);
        let Some(packed) = pack(&frame) else {
            host::set_last_error(WEBSOCKET_PACK_FAILED);
            return None;
        };
        Some(push_frame(server, fd, packed.bytes(), &[]))
    };
    call.set_bool(run().unwrap_or(false));
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
    server: view::Server<'_>,
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

/// `swoole_websocket_onMessage`: a frame `Server::dispatch_frame` delivered,
/// answered by policy or handed to `onMessage` as a Frame. A bailout in the
/// handler is raised last, over this frame only.
pub(crate) fn on_message(server: view::Server<'_>, request: &RecvData) -> c_int {
    let result = message(server, request);
    let code = *result.as_ref().unwrap_or(&OK);
    bailout::raise(result.map(|_| ()));
    code
}

fn message(view: view::Server<'_>, request: &RecvData) -> Result<c_int, bailout::Bailout> {
    let info = request.info();
    let (fd, server_fd, ext_flags) = (info.fd, c_int::from(info.server_fd), info.ext_flags);
    let server = view;
    let Some(settings) = port_frames(server, fd) else {
        return Ok(ERR);
    };
    let mut data = php_swoole_get_recv_data(view, request);
    let [opcode, flags] = ext_flags.to_be_bytes();
    let delivery = message_policy(opcode, settings);
    if delivery != Delivery::Deliver {
        if delivery == Delivery::Pong
            && let Some(pong) = encode(data.bytes().unwrap_or_default(), OPCODE_PONG, FLAG_FIN)
        {
            server_methods::send(server, fd, pong.bytes());
        }
        return Ok(OK);
    }
    // RFC 7692: the payload is deflated.
    if flags & FLAG_RSV1 != 0 {
        let Some(plain) = uncompress(data.bytes().unwrap_or_default()) else {
            return Ok(OK);
        };
        data = plain;
    }
    let frame = construct(zend_long::from(opcode), &data, flags);
    if let Some(object) = frame.object() {
        set(object, FD, scalar(IS_LONG, fd));
    }
    let mut args = [zserv(), frame.into_zval()];
    if let Err(bailout) = dispatch(server, server_fd, ON_MESSAGE, &mut args, fd) {
        // As upstream, whose bailout jumps past the releases.
        let _kept = data.into_zval();
        return Err(bailout);
    }
    // SAFETY: the Frame built above.
    unsafe { zval_ptr_dtor(&raw mut args[1]) };
    Ok(OK)
}

/// `swoole_websocket_onHandshake`: the handshake, then `onOpen`, or a reset.
pub(crate) fn handshake_open(mut context: Ctx) -> Result<(), bailout::Bailout> {
    let server = context::server(context.state());
    let fd = context.fd();
    if handshake(context)? {
        on_open(server, context)
    } else {
        close(server, fd, true);
        Ok(())
    }
}

/// A handshake `onHandshake` answers: the connection is upgrading.
pub(crate) fn upgrading(server: view::Server<'_>, fd: zend_long) {
    set_status(server, fd, STATUS_HANDSHAKE);
}

/// Set a response header as `Context::set_header(..., false)`.
fn set_header(context: Ctx, key: &[u8], value: &[u8]) -> Result<(), bailout::Bailout> {
    crate::response::set_header_bytes(context, key, value);
    if bailout::pending() {
        Err(bailout::Bailout)
    } else {
        Ok(())
    }
}

fn handshake(mut context: Ctx) -> Result<bool, bailout::Bailout> {
    let header = context.state().request.zheader.get();
    let key = header
        .and_then(Value::array)
        .and_then(|header| header.find(b"sec-websocket-key"));
    let key = key.map(Value::string);
    if key.is_some() && bailout::pending() {
        return Err(bailout::Bailout);
    }
    let key = key.filter(|key| key.bytes().len() == SEC_KEY_LENGTH);
    let Some(key) = key else {
        context.state().response.status = 400;
        send::end(context, None)?;
        return Ok(false);
    };
    let accept = swoole_rs_websocket::accept(key.bytes());
    drop(key);
    set_header(context, b"Upgrade", b"websocket")?;
    set_header(context, b"Connection", b"Upgrade")?;
    set_header(context, b"Sec-WebSocket-Accept", &accept)?;
    set_header(context, b"Sec-WebSocket-Version", b"13")?;
    let fd = context.fd();
    if context.flags() & CO_SOCKET == 0 {
        let server = context::server(context.state());
        let Some(conn) = connection(server, fd, false) else {
            closed(c"swoole_websocket_handshake", fd);
            return Ok(false);
        };
        set_status(server, fd, STATUS_ACTIVE);
        if let Some(protocol) = port_protocol(server, conn.server_fd)
            && !protocol.is_empty()
        {
            set_header(context, b"Sec-WebSocket-Protocol", &protocol)?;
        }
        let state = context.state();
        let mut args = [
            zserv(),
            state.request.zobject.value,
            state.response.zobject.value,
        ];
        dispatch(
            server,
            conn.server_fd,
            ON_BEFORE_HANDSHAKE_RESPONSE,
            &mut args,
            fd,
        )?;
    } else {
        swoole_rs_phpclients::websocket_protocol(&context.state().zsocket);
    }
    let state = context.state();
    state.response.status = 101;
    state.upgrade = true;
    send::end(context, None)
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
fn on_open(server: view::Server<'_>, mut context: Ctx) -> Result<(), bailout::Bailout> {
    let fd = context.fd();
    let Some(conn) = connection(server, fd, false) else {
        closed(c"swoole_websocket_onOpen", fd);
        return Ok(());
    };
    let mut args = [zserv(), context.state().request.zobject.value];
    dispatch(server, conn.server_fd, ON_OPEN, &mut args, fd)
}

/// `swoole_websocket_onRequest`, the default `onRequest`: 400 and close.
pub(crate) fn request(mut context: Ctx) {
    const BAD_REQUEST: &[u8] = b"HTTP/1.1 400 Bad Request\r\n\
        Connection: close\r\n\
        Content-Type: text/html; charset=UTF-8\r\n\
        Cache-Control: must-revalidate,no-cache,no-store\r\n\
        Content-Length: 83\r\n\
        Server: swoole-http-server\r\n\r\n\
        <html><body><h2>HTTP 400 Bad Request</h2><hr><i>Powered by Swoole</i></body></html>";
    context::send(context, BAD_REQUEST);
    context.state().end = true;
    context::close(context);
}

/// The coroutine `Response::push`: a frame over the context's socket.
pub(crate) fn response_push(
    mut context: Ctx,
    data: &Value,
    opcode: i64,
    flags: Option<&Value>,
) -> bool {
    let flags = flags.map_or(i64::from(FLAG_FIN), Value::long);
    let mut frame = FrameObject::new(Some(data), opcode, flags, 0);
    frame.flags &= !FLAG_MASK;
    if context.state().websocket_compression {
        frame.flags |= FLAG_COMPRESS;
    }
    send_frame(context, &frame).unwrap_or(false)
}

/// `http_response_send_frame`: `None` when the frame cannot be packed.
fn send_frame(context: Ctx, frame: &FrameObject) -> Option<bool> {
    let Some(packed) = pack(frame) else {
        host::set_last_error(WEBSOCKET_PACK_FAILED);
        return None;
    };
    let packed = packed.bytes();
    Some(context::send(context, packed))
}

/// The coroutine `Response::disconnect`: a close frame, then close.
pub(crate) fn response_disconnect(context: Ctx, code: i64, reason: Option<&ZStr>) -> bool {
    let data = string_or_empty(reason);
    let frame = FrameObject::new(
        Some(&data),
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
pub(crate) fn accept(mut context: Ctx) {
    if !ZLIB.load(Relaxed) {
        return;
    }
    let state = context.state();
    let offered = if context::co_socket(state) {
        state.websocket_settings & SETTING_COMPRESSION != 0
    } else {
        crate::context::server(state)
            .config()
            .websocket_compression
            .get()
    };
    let mut accepted = false;
    if offered && !state.request.zobject.ptr.is_null() {
        let text = state
            .request
            .zheader
            .get()
            .and_then(Value::array)
            .and_then(|header| header.find(b"sec-websocket-extensions"))
            .and_then(Value::bytes);
        if let Some(text) = text
            && text.split(|&byte| byte == b';').next() == Some(b"permessage-deflate")
        {
            accepted = true;
            crate::response::set_header_bytes(
                context,
                b"Sec-Websocket-Extensions",
                b"permessage-deflate; client_no_context_takeover; server_no_context_takeover",
            );
        }
    }
    let state = context.state();
    state.websocket_compression = accepted;
    if !crate::context::co_socket(state)
        && let Some(conn) = crate::context::server(state).verified(state.fd)
    {
        conn.set_websocket_compression(accepted);
    }
}

/// After a server handshake response: the connection is a WebSocket on 101,
/// otherwise the handshake failed and the connection closes.
pub(crate) fn handshaked(mut context: Ctx) {
    let state = context.state();
    let (server, fd) = (crate::context::server(state), state.fd);
    let Some(conn) = connection(server, fd, true) else {
        return;
    };
    if i64::from(conn.status) != STATUS_HANDSHAKE {
        return;
    }
    let state = context.state();
    let status = if state.response.status == 101 {
        STATUS_ACTIVE
    } else {
        state.keepalive = false;
        STATUS_HANDSHAKE_FAILED
    };
    set_status(server, fd, status);
}

pub(crate) fn upgrade(context: Ctx) -> Result<bool, bailout::Bailout> {
    handshake(context)
}

/// `Coroutine\Http\Client`'s `WebSocketSettings` and the message it is
/// assembling, in cells: a `recv` yields, and other coroutines push or apply
/// settings meanwhile.
struct ClientWebSocket {
    /// `OPEN_*_FRAME` bits for [`recv`].
    frames: Cell<u32>,
    mask: Cell<bool>,
    compression: Cell<bool>,
    /// Out of the cell while a `recv` assembles it.
    message: Cell<Assembly>,
}

/// A client's WebSocket state, as `WebSocketSettings` starts; registered
/// with crates/phpclients at MINIT.
fn client_websocket() -> Box<dyn swoole_rs_phpclients::WebSocket> {
    Box::new(ClientWebSocket {
        frames: Cell::new(0),
        mask: Cell::new(true),
        compression: Cell::new(false),
        message: Cell::new(None),
    })
}

impl swoole_rs_phpclients::WebSocket for ClientWebSocket {
    fn apply(&self, table: &Array) {
        self.mask
            .set(setting(table, b"websocket_mask").unwrap_or(true));
        if ZLIB.load(Relaxed)
            && let Some(on) = setting(table, b"websocket_compression")
        {
            self.compression.set(on);
        }
        for (key, bit) in [
            (&b"open_websocket_close_frame"[..], OPEN_CLOSE_FRAME),
            (b"open_websocket_ping_frame", OPEN_PING_FRAME),
            (b"open_websocket_pong_frame", OPEN_PONG_FRAME),
        ] {
            if let Some(on) = setting(table, key) {
                let frames = self.frames.get();
                self.frames
                    .set(if on { frames | bit } else { frames & !bit });
            }
        }
    }

    fn compression(&self) -> bool {
        self.compression.get()
    }

    fn push(
        &self,
        socket: swoole_rs_phpclients::Co,
        data: &Value,
        opcode: i64,
        flags: c_int,
        code: i64,
        compress: bool,
    ) -> c_int {
        let mut frame =
            FrameObject::new(Some(data), opcode, i64::from(byte(i64::from(flags))), code);
        if self.mask.get() {
            frame.flags |= FLAG_MASK;
        }
        if compress {
            frame.flags |= FLAG_COMPRESS;
        }
        pack(&frame).map_or(1, |packed| {
            let packed = packed.bytes();
            let written = socket.handle().send_all(packed);
            if usize::try_from(written).is_ok_and(|written| written == packed.len()) {
                0
            } else {
                2
            }
        })
    }

    fn recv(&self, socket: swoole_rs_phpclients::Co, timeout: f64) -> Owned {
        let mut message = self.message.take();
        let frame = recv(self.frames.get(), &mut message, socket.handle(), timeout);
        self.message.set(message);
        frame
    }
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
fn sock_recv(socket: co::Co, timeout: f64) -> (isize, *mut u8, usize) {
    let received = co::recv_packet(socket, timeout);
    if received <= 0 {
        return (received, ptr::null_mut(), 0);
    }
    // A packet read leaves it in the socket's read buffer.
    let (data, len) = socket.with_buffer(false, |buffer| (buffer.as_mut_ptr(), buffer.len()));
    (received, data, len)
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
fn reply(settings: u32, socket: co::Co, opcode: u8, payload: &[u8]) {
    let flags = if settings & IN_SERVER != 0 {
        FLAG_FIN
    } else {
        FLAG_FIN | FLAG_MASK
    };
    if let Some(frame) = encode(payload, opcode, flags) {
        let frame = frame.bytes();
        _ = socket.send(frame);
    }
}

/// A frame object for `payload`, with the socket's fd.
fn deliver(socket: co::Co, opcode: u8, payload: &Value, flags: u8) -> Owned {
    // The frame takes its own reference to the payload.
    let frame = construct(zend_long::from(opcode), payload, flags);
    if let Some(object) = frame.object() {
        let fd = socket.sock_fd();
        set(object, FD, scalar(IS_LONG, zend_long::from(fd)));
    }
    frame
}

fn protocol_error() -> Owned {
    host::set_last_error(PROTOCOL_ERROR);
    Owned::null()
}

/// The payload as a string, inflated when `compressed`.
fn payload_string(payload: &[u8], compressed: bool) -> Option<Owned> {
    if compressed {
        uncompress(payload)
    } else {
        Some(Owned::string(ZString::new(payload)))
    }
}

/// `websocket::recv_frame`: the next whole message from a coroutine socket as
/// a frame object, answering pings and closes the settings leave to it.
/// `false` when the socket read failed, `""` when it closed, `null` on any
/// other error.
pub(crate) fn recv(settings: u32, slot: &mut Assembly, socket: co::Co, timeout: f64) -> Owned {
    let func = c"websocket::recv_frame";
    loop {
        let (received, data, length) = sock_recv(socket, timeout);
        if received < 0 {
            return Owned::bool(false);
        }
        if received == 0 {
            return Owned::string(ZString::empty());
        }
        if length < 2 {
            return protocol_error();
        }
        // SAFETY: the socket's read buffer, `length` bytes, unmasked in place.
        let buffer = unsafe { std::slice::from_raw_parts_mut(data, length) };
        let Some(frame) = decode(buffer) else {
            return protocol_error();
        };
        // `decode` accepted the frame, so the buffer holds its whole payload.
        let payload = buffer
            .get(frame.header_length..)
            .and_then(|rest| rest.get(..frame.payload_length))
            .unwrap_or_default();
        let opcode = frame.opcode;
        let respond = match opcode {
            OPCODE_PING if settings & OPEN_PING_FRAME == 0 => {
                reply(settings, socket, OPCODE_PONG, payload);
                continue;
            }
            OPCODE_PONG if settings & OPEN_PONG_FRAME == 0 => continue,
            OPCODE_CLOSE if settings & OPEN_CLOSE_FRAME == 0 => {
                reply(settings, socket, OPCODE_CLOSE, payload);
                continue;
            }
            OPCODE_PING | OPCODE_PONG | OPCODE_CLOSE => true,
            _ => false,
        };
        if respond {
            let payload = Owned::string(ZString::new(payload));
            return deliver(socket, opcode, &payload, FLAG_FIN);
        }
        if opcode == OPCODE_CONTINUATION {
            let Some(message) = slot.as_mut() else {
                warning(
                    func,
                    "A continuation frame cannot stand alone and MUST be preceded by an initial frame whose \
                     opcode indicates either text or binary data.",
                );
                return Owned::null();
            };
            message.data.extend_from_slice(payload);
            if frame.flags & FLAG_FIN == 0 {
                continue;
            }
            // The whole message is assembled: release its memory once delivered.
            let Some(message) = slot.take() else {
                return Owned::null();
            };
            let [complete_opcode, complete_flags] = message.ext_flags.to_be_bytes();
            let Some(payload) = payload_string(&message.data, complete_flags & FLAG_RSV1 != 0)
            else {
                return protocol_error();
            };
            return deliver(socket, complete_opcode, &payload, complete_flags | FLAG_FIN);
        }
        if slot.is_some() {
            warning(
                func,
                "All fragments of a message, except for the initial frame, must use the continuation \
                 frame opcode(0).",
            );
            return Owned::null();
        }
        if frame.flags & FLAG_FIN != 0 {
            let Some(payload) = payload_string(payload, frame.flags & FLAG_RSV1 != 0) else {
                return protocol_error();
            };
            return deliver(socket, opcode, &payload, frame.flags);
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
