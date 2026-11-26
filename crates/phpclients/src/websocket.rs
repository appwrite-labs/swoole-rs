//! Zend WebSocket frame classes, their wire codec, and coroutine client state.
use std::{
    borrow::Cow,
    cell::Cell,
    ffi::{CStr, c_int},
};
use swoole_rs_corebase::lent::{Spare, lend_spare};
use swoole_rs_corebase::{CCast, CellExt, WebSocketStatus, log};
use swoole_rs_phpruntime::co;
use swoole_rs_phpstubs::websocket as stubs;
use swoole_rs_websocket::{
    CLOSE_CODE_LEN, CLOSE_REASON_MAX_LEN, CONTROL_PAYLOAD_MAX_LEN, DecodeErrorKind, DecodedFrame,
    EncodeError, FrameFlags, FrameMetadata, Opcode, control_invalid, encode_close, encode_frame,
    encoded_size, header, pack_policy,
};
use swoole_rs_zend::{
    Array, Call, Class, Declared, Minit, Owned, Value, ZStr, ZString, bailout, handlers,
    method_table, string,
    sys::{IS_NULL, ZEND_ACC_PUBLIC, ZEND_ACC_STATIC, zend_long},
};
static FRAME: Class = Class::new();
static CLOSE_FRAME: Class = Class::new();
/// Frame's declared `fd`, `data`, `opcode`, `flags` and `finish`, as byte offsets
/// into a `Frame` or `CloseFrame` (which keeps its parent's slots), set in MINIT.
static SLOTS: [Declared; 5] = [const { Declared::new() }; 5];
const SLOT_NAMES: [&CStr; 5] = [c"fd", c"data", c"opcode", c"flags", c"finish"];
const FD: usize = 0;
const DATA: usize = 1;
const OPCODE: usize = 2;
const FLAGS: usize = 3;
const FINISH: usize = 4;

/// Whether the loaded extension has zlib support.
#[must_use]
pub fn has_zlib() -> bool {
    swoole_rs_corebase::built_with(swoole_rs_corebase::Switch::Zlib)
}

/// `swoole::websocket::Flag`.
pub const FLAG_FIN: u8 = 1;
const FLAG_COMPRESS: u8 = 2;
pub const FLAG_RSV1: u8 = 4;
const FLAG_RSV2: u8 = 8;
const FLAG_RSV3: u8 = 16;
const FLAG_MASK: u8 = 32;
const FLAG_ENCODE_HEADER_ONLY: u8 = 64;
pub(crate) const FLAGS_ALL: i64 = 63;
/// `swoole::websocket::CloseReason`, as their constants name them.
pub const CLOSE_NORMAL: i64 = 1000;
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

/// Swoole error codes and log levels (`include/swoole_error.h`, `swoole_log.h`).
const PROTOCOL_ERROR: c_int = swoole_rs_error::ErrorCode::SW_ERROR_PROTOCOL_ERROR as c_int;
const WEBSOCKET_INCOMPLETE_PACKET: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_WEBSOCKET_INCOMPLETE_PACKET as c_int;

/// A boolean setting's value, when set and not null.
fn setting(table: &Array, key: &[u8]) -> Option<bool> {
    table.get(key).map(Value::truthy)
}

method_table! {
    static FRAME_METHODS = [
        c"__toString" => zim_swoole_websocket_frame___toString, &stubs::arginfo_class_Swoole_WebSocket_Frame___toString, ZEND_ACC_PUBLIC;
        c"pack" => zim_swoole_websocket_server_pack, &stubs::arginfo_class_Swoole_WebSocket_Frame_pack, ZEND_ACC_PUBLIC | ZEND_ACC_STATIC;
        c"unpack" => zim_swoole_websocket_server_unpack, &stubs::arginfo_class_Swoole_WebSocket_Frame_unpack, ZEND_ACC_PUBLIC | ZEND_ACC_STATIC;
    ];
}

/// Register the frame classes and their constants after the WebSocket server class.
pub fn register_frames(minit: &Minit) {
    minit.register(
        &FRAME,
        c"Swoole\\WebSocket\\Frame",
        FRAME_METHODS.table(),
        |_, _| {},
    );
    minit.implements(&FRAME, minit.stringable());
    for (name, value) in [
        (c"fd", 0),
        (c"opcode", i64::from(Opcode::Text.code())),
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
        slot.resolve(&FRAME, name);
    }
    let frame = minit.parent(&FRAME);
    minit.register_child(
        &CLOSE_FRAME,
        c"Swoole\\WebSocket\\CloseFrame",
        swoole_rs_zend::arginfo::Table::NONE,
        frame,
        |_, _| {},
    );
    for (name, value) in [
        (c"opcode", i64::from(Opcode::Close.code())),
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
        (c"CONTINUATION", Opcode::Continuation.code()),
        (c"TEXT", Opcode::Text.code()),
        (c"BINARY", Opcode::Binary.code()),
        (c"CLOSE", Opcode::Close.code()),
        (c"PING", Opcode::Ping.code()),
        (c"PONG", Opcode::Pong.code()),
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
            (c"CONNECTION", WebSocketStatus::Connection),
            (c"HANDSHAKE", WebSocketStatus::Handshake),
        ];
        if bc {
            statuses.push((c"FRAME", WebSocketStatus::Active));
        }
        statuses.extend([
            (c"ACTIVE", WebSocketStatus::Active),
            (c"CLOSING", WebSocketStatus::Closing),
        ]);
        if bc {
            statuses.push((c"HANDSHAKE_FAILED", WebSocketStatus::HandshakeFailed));
        }
        for (name, status) in statuses {
            register(prefix, "STATUS_", name, i64::from(status.code()));
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
    /// An outgoing server frame uses the connection's compression setting and is unmasked.
    #[must_use]
    pub fn server(data: Option<&'a Value>, opcode: i64, flags: i64, compression: bool) -> Self {
        let mut frame = Self::new(data, opcode, flags, 0);
        if compression {
            frame.flags |= FLAG_COMPRESS;
        }
        frame.flags &= !FLAG_MASK;
        frame
    }

    /// Send the header and payload separately when the payload can be borrowed.
    pub fn send_parts(&self, send: impl FnOnce(&[u8], &[u8]) -> bool) -> Option<bool> {
        packed(self, |payload, flags| {
            // A large data frame's payload is sent from where it is, behind its header.
            if payload.len() > INLINE
                && self.opcode < Opcode::Close.code()
                && flags & FLAG_ENCODE_HEADER_ONLY == 0
            {
                let (head, length) = header(
                    payload.len() as u64,
                    Opcode::from_code(self.opcode),
                    FrameFlags::from_bits(flags),
                );
                let head = head.get(..length).unwrap_or_default();
                return Some(send(head, payload));
            }
            let frame = whole(self, payload, flags)?;
            Some(send(frame.bytes(), &[]))
        })
    }

    /// `FrameObject(zdata, opcode, flags, code)`: a `Frame` object's
    /// properties, or the arguments around any other value.
    pub fn new(data: Option<&'a Value>, opcode: i64, flags: i64, code: i64) -> Self {
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
        let long = |name: &CStr| frame.property(name).map_or(0, |value| value.long());
        let opcode = byte(long(c"opcode"));
        let mut code = 0;
        let data = if opcode == Opcode::Close.code() {
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
    log::warning(func, format_args!("{message}"));
}

fn fatal(message: &str) {
    let message = std::ffi::CString::new(message).unwrap_or_default();
    crate::native::swoole_rs_php_error(swoole_rs_zend::E_WARNING, true, &message);
}

/// `FrameObject::pack`: the frame on the wire as a new string, or `None`.
pub fn pack(frame: &FrameObject) -> Option<Frame> {
    packed(frame, |payload, flags| whole(frame, payload, flags))
}

/// `frame` around its packed `payload`, in one piece.
fn whole(frame: &FrameObject, payload: &[u8], flags: u8) -> Option<Frame> {
    if frame.opcode == Opcode::Close.code() {
        close_frame(i32::from(frame.code), payload, flags)
    } else {
        encode(payload, frame.opcode, flags)
    }
}

/// What `FrameObject::pack` puts on the wire, to `emit`: the payload, deflated when
/// the flags ask and zlib can, and the flags that leaves. `None` for a frame that
/// cannot be packed.
fn packed<T>(frame: &FrameObject, emit: impl FnOnce(&[u8], u8) -> Option<T>) -> Option<T> {
    if frame.opcode > Opcode::Pong.code() {
        fatal("the maximum value of opcode is 10");
        return None;
    }
    let data = frame.data.as_deref().filter(|data| !data.is_null());
    let data = data.map(Value::string);
    let data = data.as_deref().map_or(&[][..], ZStr::bytes);
    if bailout::pending() {
        return None;
    }
    let opcode = Opcode::from_code(frame.opcode);
    if opcode == Opcode::Close && data.len() > CLOSE_REASON_MAX_LEN {
        fatal(&format!(
            "the max length of close reason is {CLOSE_REASON_MAX_LEN}"
        ));
        return None;
    }
    if opcode.is_control() && data.len() > CONTROL_PAYLOAD_MAX_LEN {
        fatal(&format!(
            "websocket control frame payload must not exceed {CONTROL_PAYLOAD_MAX_LEN} bytes"
        ));
        return None;
    }
    if !has_zlib() {
        warning(
            c"FrameObject::pack",
            "Unable to compress websocket data frame, the `zlib` supports is required",
        );
        return None;
    }
    let (flags, deflate) = pack_policy(opcode, FrameFlags::from_bits(frame.flags), data.len());
    let mut flags = flags.bits();
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
            fatal(&format!("deflateInit2() failed, Error: [{status}]"));
            None
        }
        Err(MessageFailure::Deflate) => {
            fatal("Failed to compress outgoing frame");
            None
        }
    }
}

/// Frames up to this size encode on the stack: upstream encodes into a reused
/// buffer, so a small message costs no allocation there either.
const INLINE: usize = 128;

/// An encoded frame, on the stack when it fits.
pub enum Frame {
    Inline([u8; INLINE], usize),
    Heap(ZString),
}

impl Frame {
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        match self {
            Self::Inline(buffer, size) => buffer.get(..*size).unwrap_or_default(),
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
fn encoded(
    size: usize,
    write: impl FnOnce(&mut Spare<'_>) -> Result<usize, EncodeError>,
) -> Option<Frame> {
    if size <= INLINE {
        let mut buffer = [0; INLINE];
        let written = lend_spare(&mut buffer, write).0;
        return (written.ok()? == size).then_some(Frame::Inline(buffer, size));
    }
    let mut out = string::ZBuf::with_capacity(size);
    // The fresh string's `size` unwritten bytes; dropped, the string is freed.
    (out.fill(write).ok()? == size).then(|| Frame::Heap(out.into_zstring()))
}

/// Upstream's encoders fail a masked header-only frame after writing it.
fn header_only_mask(flags: FrameFlags) -> bool {
    flags.is_masked() && flags.is_header_only()
}

/// `websocket::encode`.
#[must_use]
pub fn encode(data: &[u8], opcode: u8, flags: u8) -> Option<Frame> {
    let opcode = Opcode::from_code(opcode);
    let flags = FrameFlags::from_bits(flags);
    if control_invalid(opcode, flags, data.len()) {
        warning(
            c"websocket::encode",
            &format!(
                "invalid websocket control frame: opcode={}, fin={}, payload_length={}",
                opcode.code(),
                u8::from(flags.is_final()),
                data.len()
            ),
        );
        return None;
    }
    let size = encoded_size(data.len(), flags).ok()?;
    let frame = encoded(size, |out| {
        encode_frame(data.len(), data, opcode, flags, out)
    })?;
    (!header_only_mask(flags)).then_some(frame)
}

/// `websocket::pack_close_frame`.
#[must_use]
pub fn close_frame(code: i32, reason: &[u8], flags: u8) -> Option<Frame> {
    let flags = FrameFlags::from_bits(flags);
    let func = c"websocket::pack_close_frame";
    if reason.len() > CLOSE_REASON_MAX_LEN {
        warning(
            func,
            &format!("the max length of close reason is {CLOSE_REASON_MAX_LEN}"),
        );
        return None;
    }
    let length = reason.len() + CLOSE_CODE_LEN;
    if control_invalid(Opcode::Close, FrameFlags::from_bits(FLAG_FIN), length) {
        warning(
            func,
            &format!("invalid websocket control frame: opcode=8, fin=1, payload_length={length}"),
        );
        return None;
    }
    let size = encoded_size(length, flags.with_final()).ok()?;
    let frame = encoded(size, |out| encode_close(code, reason, flags, out))?;
    (!header_only_mask(flags)).then_some(frame)
}

/// `FrameObject::uncompress`: the inflated payload, as a string.
#[must_use]
pub fn uncompress(data: &[u8], max: usize) -> Option<Owned> {
    use swoole_rs_inflate::{MessageFailure, error, message};

    if !has_zlib() {
        warning(
            c"FrameObject::uncompress",
            "The compressed websocket data frame is received, the `zlib` supports is required",
        );
        return None;
    }
    match message(data, max) {
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
        Err(MessageFailure::Limit(limit)) => {
            warning(
                c"websocket::message_uncompress",
                &format!("decoded message exceeds {limit} bytes"),
            );
            None
        }
        Err(MessageFailure::Inflate(status)) => {
            fatal(&format!(
                "inflate() failed, Error: {}[{status}]",
                error(status).to_string_lossy()
            ));
            None
        }
    }
}

/// `websocket::construct_frame`: a new `Frame`, or a `CloseFrame` with the
/// close payload's code and reason.
fn construct(opcode: zend_long, payload: Owned, mut flags: u8) -> Owned {
    let close = opcode == zend_long::from(Opcode::Close.code());
    let class = if close { &CLOSE_FRAME } else { &FRAME };
    let frame = class.create_object();
    if frame.object().is_none() {
        return frame;
    }
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
        SLOTS[DATA].init(&frame, payload);
    }
    if flags & FLAG_RSV1 != 0 {
        flags |= FLAG_COMPRESS;
    }
    SLOTS[OPCODE].init(&frame, Owned::long(opcode));
    SLOTS[FLAGS].init(&frame, Owned::long(i64::from(flags)));
    SLOTS[FINISH].init(&frame, Owned::bool(flags & FLAG_FIN != 0));
    frame
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
            call.long(2, i64::from(Opcode::Text.code()))?,
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
    let error = match swoole_rs_websocket::decode_in_place(data) {
        Ok(frame) => return Some(frame),
        Err(error) => error,
    };
    let frame = error.frame;
    let expected = match error.kind {
        DecodeErrorKind::CompressedControl => {
            warning(
                func,
                &format!(
                    "invalid compressed websocket control frame received: opcode={}",
                    frame.opcode.code()
                ),
            );
            return None;
        }
        DecodeErrorKind::IncompleteHeader => 0,
        DecodeErrorKind::IncompleteBody => (frame.header_length + frame.payload_length) as u64,
        DecodeErrorKind::LengthOverflow => u64::MAX,
    };
    log::error_log(
        log::LogLevel::SW_LOG_WARNING,
        WEBSOCKET_INCOMPLETE_PACKET,
        func,
        format_args!(
            "incomplete packet, expected length is {expected}, actual length is {}",
            data.len()
        ),
    );
    None
}

fn unpack(call: &Call) {
    let Ok(Some(string)) = call.arity(1, 1).and_then(|()| call.zstr(1)) else {
        call.set_bool(false);
        return;
    };
    // The argument is the caller's string: a masked payload is unmasked in a copy.
    let mut data = string.bytes().to_vec();
    if data.len() < 2 {
        log::set_last_error(PROTOCOL_ERROR);
        call.set_bool(false);
        return;
    }
    let Some(frame) = decode(&mut data) else {
        log::set_last_error(PROTOCOL_ERROR);
        call.set_bool(false);
        return;
    };
    let payload = data.get(frame.header_length..frame.header_length + frame.payload_length);
    let payload = payload.unwrap_or_default();
    let zpayload = if frame.flags.is_compressed() {
        let Some(plain) = uncompress(
            payload,
            swoole_rs_corebase::constants::SW_INPUT_BUFFER_SIZE as usize,
        ) else {
            log::set_last_error(PROTOCOL_ERROR);
            call.set_bool(false);
            return;
        };
        plain
    } else {
        Owned::string(ZString::new(payload))
    };
    call.set_owned(construct(
        i64::from(frame.opcode.code()),
        zpayload,
        frame.flags.bits(),
    ));
}

handlers! {
    zim_swoole_websocket_server_pack => pack_method;
    zim_swoole_websocket_server_unpack => unpack;
    zim_swoole_websocket_frame___toString => to_string;
}

/// Where a coroutine client frame send failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PushError {
    Pack,
    Send,
}

/// `Coroutine\Http\Client`'s `WebSocketSettings` and the message it is
/// assembling, in cells: a `recv` yields, and other coroutines push or apply
/// settings meanwhile.
pub(crate) struct ClientWebSocket {
    /// `OPEN_*_FRAME` bits for [`recv`].
    frames: Cell<u8>,
    mask: Cell<bool>,
    compression: Cell<bool>,
    /// Out of the cell while a `recv` assembles it.
    message: Cell<Assembly>,
}

impl Default for ClientWebSocket {
    fn default() -> Self {
        Self {
            frames: Cell::new(0),
            mask: Cell::new(true),
            compression: Cell::new(false),
            message: Cell::new(Assembly::Idle),
        }
    }
}

impl ClientWebSocket {
    pub(crate) fn apply(&self, table: &Array) {
        self.mask
            .set(setting(table, b"websocket_mask").unwrap_or(true));
        if has_zlib()
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

    pub(crate) fn compression(&self) -> bool {
        self.compression.get()
    }

    pub(crate) fn push(
        &self,
        socket: crate::Co,
        data: &Value,
        opcode: i64,
        flags: i64,
        code: i64,
        compress: bool,
    ) -> Result<(), PushError> {
        let mut frame = FrameObject::new(Some(data), opcode, flags, code);
        if self.mask.get() {
            frame.flags |= FLAG_MASK;
        }
        if compress {
            frame.flags |= FLAG_COMPRESS;
        }
        let packed = pack(&frame).ok_or(PushError::Pack)?;
        let packed = packed.bytes();
        if socket.handle().send_all(packed) == Some(packed.len()) {
            Ok(())
        } else {
            Err(PushError::Send)
        }
    }

    pub(crate) fn recv(&self, socket: crate::Co, timeout: f64) -> Owned {
        self.message
            .with_taken(|message| recv(self.frames.get(), message, socket.handle(), timeout))
    }
}

/// `WebSocketSettings`' bits for [`recv`], and with `compression` a
/// coroutine server's `websocket_settings`.
pub const IN_SERVER: u8 = 1;
pub const OPEN_PING_FRAME: u8 = 1 << 1;
pub const OPEN_PONG_FRAME: u8 = 1 << 2;
pub const OPEN_CLOSE_FRAME: u8 = 1 << 3;
pub const SETTING_COMPRESSION: u8 = 1 << 4;

/// A message being assembled from fragments: its payload so far, and the
/// first frame's opcode and flags.
pub struct Message {
    data: Vec<u8>,
    metadata: FrameMetadata,
}

/// Where a message being assembled lives: a context's `ws_buffer`, or a
/// client's `continue_frame_buffer` (null when none).
// HTTP contexts zero-initialize this field.
#[repr(u8)]
#[derive(Default)]
pub enum Assembly {
    #[default]
    Idle = 0,
    Collecting(Box<Message>),
    Discarding,
}

/// `websocket::send_frame`: answer a control frame the user did not ask for.
fn reply(settings: u8, socket: co::Co, opcode: u8, payload: &[u8]) {
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
pub fn deliver(fd: zend_long, opcode: u8, payload: Owned, flags: u8) -> Owned {
    let frame = construct(zend_long::from(opcode), payload, flags);
    SLOTS[FD].init(&frame, Owned::long(fd));
    frame
}

fn protocol_error() -> Owned {
    log::set_last_error(PROTOCOL_ERROR);
    Owned::null()
}

/// The payload as a string, inflated when `compressed`.
fn payload_string(payload: &[u8], compressed: bool, max: usize) -> Option<Owned> {
    if compressed {
        uncompress(payload, max)
    } else {
        Some(Owned::string(ZString::new(payload)))
    }
}

/// `websocket::recv_frame`: the next whole message from a coroutine socket as
/// a frame object, answering pings and closes the settings leave to it.
/// `false` when the socket read failed, `""` when it closed, `null` on any
/// other error.
pub fn recv(settings: u8, slot: &mut Assembly, socket: co::Co, timeout: f64) -> Owned {
    loop {
        let received = co::recv_packet(socket, timeout);
        if received.is_none() {
            return Owned::bool(false);
        }
        if received == Some(0) {
            return Owned::string(ZString::empty());
        }
        // A packet read leaves it in the socket's read buffer, which a send may not
        // use while it is lent: a reply waits for the loan to end.
        let next = socket.with_buffer(false, |buffer| {
            receive(settings, slot, socket, buffer.view_mut())
        });
        match next {
            Next::Done(frame) => return frame,
            Next::Reply(opcode, payload) => reply(settings, socket, opcode.code(), &payload),
            Next::Recv => {}
        }
    }
}

/// What [`recv`] does after one packet.
enum Next {
    Done(Owned),
    /// Answer a control frame with its payload, then read the next packet.
    Reply(Opcode, Vec<u8>),
    Recv,
}

/// One packet of [`recv`], `packet`, unmasked in place.
fn receive(settings: u8, slot: &mut Assembly, socket: co::Co, packet: &mut [u8]) -> Next {
    let deliver =
        |opcode, payload, flags| deliver(zend_long::from(socket.sock_fd()), opcode, payload, flags);
    let func = c"websocket::recv_frame";
    if packet.len() < 2 {
        return Next::Done(protocol_error());
    }
    let Some(frame) = decode(packet) else {
        return Next::Done(protocol_error());
    };
    // `decode` accepted the frame, so the packet holds its whole payload.
    let payload = packet
        .get(frame.header_length..)
        .and_then(|rest| rest.get(..frame.payload_length))
        .unwrap_or_default();
    let opcode = frame.opcode;
    let respond = match opcode {
        Opcode::Ping if settings & OPEN_PING_FRAME == 0 => {
            return Next::Reply(Opcode::Pong, payload.to_vec());
        }
        Opcode::Pong if settings & OPEN_PONG_FRAME == 0 => return Next::Recv,
        Opcode::Close if settings & OPEN_CLOSE_FRAME == 0 => {
            return Next::Reply(Opcode::Close, payload.to_vec());
        }
        opcode => opcode.is_control(),
    };
    if respond {
        let payload = Owned::string(ZString::new(payload));
        return Next::Done(deliver(opcode.code(), payload, FLAG_FIN));
    }
    if opcode == Opcode::Continuation && matches!(slot, Assembly::Discarding) {
        if frame.flags.is_final() {
            *slot = Assembly::Idle;
        }
        return Next::Recv;
    }
    let limit = socket.edit_protocol(|protocol| protocol.package_max_length as usize);
    if opcode == Opcode::Continuation || !frame.flags.is_final() {
        let assembled = match slot {
            Assembly::Collecting(message) => message.data.len(),
            _ => 0,
        };
        if assembled
            .checked_add(payload.len())
            .is_none_or(|len| len > limit)
        {
            *slot = if frame.flags.is_final() {
                Assembly::Idle
            } else {
                Assembly::Discarding
            };
            log::set_last_error(
                swoole_rs_error::ErrorCode::SW_ERROR_PACKAGE_LENGTH_TOO_LARGE as c_int,
            );
            return Next::Done(Owned::null());
        }
    }
    if opcode == Opcode::Continuation {
        let Assembly::Collecting(message) = slot else {
            warning(
                func,
                "A continuation frame cannot stand alone and MUST be preceded by an initial frame whose \
                 opcode indicates either text or binary data.",
            );
            return Next::Done(Owned::null());
        };
        message.data.extend_from_slice(payload);
        if !frame.flags.is_final() {
            return Next::Recv;
        }
        // The whole message is assembled: release its memory once delivered.
        let Assembly::Collecting(message) = std::mem::take(slot) else {
            return Next::Done(Owned::null());
        };
        let complete = message.metadata;
        let Some(payload) = payload_string(&message.data, complete.flags.is_compressed(), limit)
        else {
            return Next::Done(protocol_error());
        };
        let flags = complete.flags.with_final().bits();
        return Next::Done(deliver(complete.opcode.code(), payload, flags));
    }
    if !matches!(slot, Assembly::Idle) {
        warning(
            func,
            "All fragments of a message, except for the initial frame, must use the continuation \
             frame opcode(0).",
        );
        return Next::Done(Owned::null());
    }
    if frame.flags.is_final() {
        let Some(payload) = payload_string(payload, frame.flags.is_compressed(), limit) else {
            return Next::Done(protocol_error());
        };
        return Next::Done(deliver(opcode.code(), payload, frame.flags.bits()));
    }
    *slot = Assembly::Collecting(Box::new(Message {
        data: payload.to_vec(),
        metadata: FrameMetadata {
            opcode,
            flags: frame.flags,
        },
    }));
    Next::Recv
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
            header_only_mask(FrameFlags::from_bits(FLAG_MASK | FLAG_ENCODE_HEADER_ONLY)),
            "both"
        );
        assert!(
            !header_only_mask(FrameFlags::from_bits(FLAG_MASK)),
            "mask only"
        );
    }
}
