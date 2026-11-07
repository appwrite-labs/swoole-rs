//! Callback-free `ListenPort` decisions. All native effects and object custody stay native.
#![expect(
    clippy::missing_safety_doc,
    reason = "every export is covered by the module-local C contract"
)]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
use std::ffi::{CStr, c_char, c_int};
use std::slice;

pub mod lifecycle;
pub mod methods;
pub mod read;

const _: () = {
    assert!(
        std::mem::size_of::<std::sync::atomic::AtomicU32>() == std::mem::size_of::<u32>(),
        "AtomicU32 size must match u32"
    );
    assert!(
        std::mem::align_of::<std::sync::atomic::AtomicU32>() == std::mem::align_of::<u32>(),
        "AtomicU32 alignment must match u32"
    );
};

#[repr(C)]
pub struct PortsDefaults {
    pub object_id: u32,
    pub length_type: u32,
    pub length_size: u32,
    pub body_offset: u32,
    pub max_length: u32,
    pub eof_length: u32,
}

/// Constructor scalar defaults; native literal bytes and server pointer stay native.
#[unsafe(no_mangle)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "native protocol-field narrowing, preserved deliberately"
)]
pub extern "C" fn swr_v1_ports_defaults(max_length: u32, eof_length: u32) -> PortsDefaults {
    PortsDefaults {
        object_id: 0,
        length_type: u32::from(b'N'),
        length_size: 4,
        body_offset: 4,
        max_length,
        eof_length: u32::from(eof_length as u8),
    }
}

/// Fixed network-u32 stream framing; callback addresses never cross this ABI.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ports_stream_settings() -> PortsFramingSettings {
    PortsFramingSettings {
        enabled: 1,
        length_type: u32::from(b'N'),
        length_size: 4,
        body_offset: 4,
        ..Default::default()
    }
}

#[repr(C)]
pub struct PortsFramingInput {
    pub kind: u32,
    pub find_from_right: u32,
    pub length: usize,
    pub capacity: usize,
    pub length_type: u32,
    pub length_size: i32,
    pub length_offset: u32,
    pub body_offset: u32,
}

#[repr(C)]
#[derive(Default)]
pub struct PortsFramingSettings {
    pub length: usize,
    pub enabled: u32,
    pub split: u32,
    pub length_type: u32,
    pub length_size: u32,
    pub length_offset: u32,
    pub body_offset: u32,
}

/// EOF bounds/direction and native protocol-field narrowing, with no byte access.
#[unsafe(no_mangle)]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "native protocol-field narrowing, preserved deliberately"
)]
pub extern "C" fn swr_v1_ports_framing_settings(input: PortsFramingInput) -> PortsFramingSettings {
    match input.kind {
        0 => PortsFramingSettings {
            length: input.length.min(input.capacity),
            enabled: 1,
            split: u32::from(input.find_from_right == 0),
            ..Default::default()
        },
        1 => PortsFramingSettings {
            enabled: 1,
            length_type: u32::from(input.length_type as u8),
            length_size: u32::from(input.length_size as u8),
            length_offset: u32::from(input.length_offset as u16),
            body_offset: u32::from(input.body_offset as u16),
            ..Default::default()
        },
        _ => PortsFramingSettings::default(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ports_number(kind: u32, value: i64) -> i64 {
    match kind {
        0 => value.clamp(0, i64::from(u16::MAX)),
        1 => match value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) {
            n if n <= 0 => i64::from(i32::MAX),
            n => n,
        },
        2 => value.clamp(0, i64::from(u32::MAX)),
        3 => value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)),
        4 => value.clamp(0, i64::from(u8::MAX)),
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ports_protocol(flags: u32, length_type: u8) -> u32 {
    if flags & 1 != 0 && flags & 2 == 0 {
        0
    } else if flags & 4 != 0 {
        1
    } else if flags & 8 != 0 {
        if length_type != 0 { 2 } else { 10 }
    } else if flags & 16 != 0 {
        match (flags & 32 != 0, flags & 64 != 0) {
            (true, true) => 6,
            (true, false) => 4,
            (false, true) => 5,
            (false, false) => 3,
        }
    } else if flags & 128 != 0 {
        7
    } else if flags & 256 != 0 {
        8
    } else {
        9
    }
}

/// Native layout facts only; no object, callback or pointer crosses this plan.
#[repr(C)]
pub struct PortsProtocolInput {
    pub flags: u32,
    pub length_type: u32,
    pub eof_len: u32,
    pub eof_max: u32,
    pub http2_header_size: u32,
    pub websocket_header_size: u32,
    pub mqtt_header_size: u32,
}
#[derive(Clone, Copy)]
#[repr(C)]
pub struct PortsProtocolAction {
    pub kind: u32,
    pub value: u32,
}
#[repr(C)]
pub struct PortsProtocolPlan {
    pub count: u32,
    pub actions: [PortsProtocolAction; 6],
}
#[derive(Clone, Copy)]
#[repr(u32)]
enum ProtocolField {
    EofLength = 1,
    Reader,
    Package,
    LengthFunction,
    LengthSizeFunction,
    LengthSize,
    LengthOffset,
    BodyOffset,
    NoDelay,
    Keepalive,
    CheckEof,
    SplitEof,
    HttpFlag,
    WebsocketFlag,
    Http2Flag,
    MqttFlag,
    RedisFlag,
    FastOpen,
    CheckLength,
}
const fn binding(field: ProtocolField, value: u32) -> PortsProtocolAction {
    PortsProtocolAction {
        kind: field as u32,
        value,
    }
}
fn protocol_plan<const N: usize>(actions: [PortsProtocolAction; N]) -> PortsProtocolPlan {
    const { assert!(N <= 6, "a protocol plan holds at most 6 actions") };
    let mut plan = PortsProtocolPlan {
        #[expect(clippy::cast_possible_truncation, reason = "N <= 6, asserted above")]
        count: N as u32,
        actions: [PortsProtocolAction { kind: 0, value: 0 }; 6],
    };
    for (destination, action) in plan.actions.iter_mut().zip(actions) {
        *destination = action;
    }
    plan
}
#[repr(C)]
pub struct PortsBufferSetting {
    pub count: u32,
    pub first: i32,
    pub last: i32,
}

/// Preserve the original clamped store followed by its nonpositive fallback store.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ports_buffer_setting(value: i64) -> PortsBufferSetting {
    // Kind 3 clamps to the i32 range.
    let first = i32::try_from(swr_v1_ports_number(3, value)).unwrap_or(i32::MAX);
    PortsBufferSetting {
        count: if first <= 0 { 2 } else { 1 },
        first,
        last: if first <= 0 { i32::MAX } else { 0 },
    }
}

#[repr(C)]
pub struct PortsPacketSetting {
    pub value: u32,
    pub status: u32,
    pub copy_len: usize,
}

/// Keep native u8 publication/error priority, and reject an over-capacity copy.
#[unsafe(no_mangle)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "native protocol-field narrowing, preserved deliberately"
)]
pub extern "C" fn swr_v1_ports_packet_eof(length: usize, capacity: u32) -> PortsPacketSetting {
    let value = u32::from(length as u8);
    let status = if value == 0 {
        1
    } else if value > capacity || length > capacity as usize {
        2
    } else {
        0
    };
    PortsPacketSetting {
        value,
        status,
        copy_len: if status == 0 { length } else { 0 },
    }
}

/// M19's callback-free width result is copied after the native type publication.
#[unsafe(no_mangle)]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "native protocol-field narrowing, preserved deliberately"
)]
pub extern "C" fn swr_v1_ports_packet_width(width: i32) -> PortsPacketSetting {
    let value = u32::from(width as u8);
    PortsPacketSetting {
        value,
        status: u32::from(value == 0),
        copy_len: 0,
    }
}

/// Offset narrowing and bound rejection; native publishes value before reporting.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ports_packet_offset(value: i64, limit: u32) -> PortsPacketSetting {
    // Kind 0 clamps to the u16 range.
    let value = u32::try_from(swr_v1_ports_number(0, value)).unwrap_or(0);
    PortsPacketSetting {
        value,
        status: u32::from(value > limit),
        copy_len: 0,
    }
}

#[repr(C)]
pub struct PortsOption {
    pub key: PortsSpan,
    pub alias: PortsSpan,
    pub action: u32,
    pub target: u32,
    pub converter: u32,
    pub number_kind: u32,
    pub policy: u32,
    pub reserved: u32,
}
#[derive(Clone, Copy)]
#[repr(u32)]
enum OptionAction {
    Number = 1,
    Buffer,
    Flag,
    Special,
}
#[derive(Clone, Copy)]
#[repr(u32)]
enum Converter {
    Long,
    Size,
    Truth,
    String,
}
#[derive(Clone, Copy)]
#[repr(u32)]
enum NumberField {
    Backlog = 1,
    SocketBuffer = 2,
    KernelRecv = 3,
    KernelSend = 4,
    Heartbeat = 5,
    HighWatermark = 6,
    LowWatermark = 7,
    DeferAccept = 8,
    KeepIdle = 9,
    KeepInterval = 10,
    KeepCount = 11,
    UserTimeout = 12,
    MaxLength = 13,
}
#[derive(Clone, Copy)]
#[repr(u32)]
enum SpecialSetting {
    PacketEof = 1,
    WebsocketOptions = 2,
    MaxIdle = 3,
    PacketType = 4,
    LengthOffsetSetting = 5,
    BodyOffsetSetting = 6,
    LengthCallback = 7,
}
// key, action, native destination, conversion, numeric mode. No mutable PHP data.
type OptionSpec = (&'static CStr, OptionAction, u32, Converter, u32);
#[rustfmt::skip]
static OPTIONS: [OptionSpec; 31] = {
    use OptionAction::{Number, Buffer, Flag, Special};
    use Converter::{Long, Size, Truth};
    use NumberField::{Backlog, SocketBuffer, KernelRecv, KernelSend, Heartbeat, HighWatermark, LowWatermark, DeferAccept, KeepIdle, KeepInterval, KeepCount, UserTimeout, MaxLength};
    use SpecialSetting::{PacketEof, WebsocketOptions, MaxIdle, PacketType, LengthOffsetSetting, BodyOffsetSetting, LengthCallback};
    [
        (c"backlog", Number, Backlog as u32, Long, 0),
        (c"socket_buffer_size", Buffer, SocketBuffer as u32, Size, 0),
        (c"kernel_socket_recv_buffer_size", Buffer, KernelRecv as u32, Size, 0),
        (c"kernel_socket_send_buffer_size", Buffer, KernelSend as u32, Size, 0),
        (c"heartbeat_idle_time", Number, Heartbeat as u32, Long, 0),
        (c"buffer_high_watermark", Number, HighWatermark as u32, Size, 2),
        (c"buffer_low_watermark", Number, LowWatermark as u32, Size, 2),
        (c"open_tcp_nodelay", Flag, ProtocolField::NoDelay as u32, Truth, 0),
        (c"tcp_defer_accept", Number, DeferAccept as u32, Long, 3),
        (c"open_tcp_keepalive", Flag, ProtocolField::Keepalive as u32, Truth, 0),
        (c"open_eof_check", Flag, ProtocolField::CheckEof as u32, Truth, 0),
        (c"open_eof_split", Flag, ProtocolField::SplitEof as u32, Truth, 0),
        (c"package_eof", Special, PacketEof as u32, Long, 0),
        (c"open_http_protocol", Flag, ProtocolField::HttpFlag as u32, Truth, 0),
        (c"open_websocket_protocol", Flag, ProtocolField::WebsocketFlag as u32, Truth, 0),
        (c"", Special, WebsocketOptions as u32, Long, 0),
        (c"open_http2_protocol", Flag, ProtocolField::Http2Flag as u32, Truth, 0),
        (c"open_mqtt_protocol", Flag, ProtocolField::MqttFlag as u32, Truth, 0),
        (c"open_redis_protocol", Flag, ProtocolField::RedisFlag as u32, Long, 0),
        (c"max_idle_time", Special, MaxIdle as u32, Long, 0),
        (c"tcp_keepidle", Number, KeepIdle as u32, Long, 3),
        (c"tcp_keepinterval", Number, KeepInterval as u32, Long, 3),
        (c"tcp_keepcount", Number, KeepCount as u32, Long, 3),
        (c"tcp_user_timeout", Number, UserTimeout as u32, Long, 3),
        (c"tcp_fastopen", Flag, ProtocolField::FastOpen as u32, Truth, 0),
        (c"open_length_check", Flag, ProtocolField::CheckLength as u32, Truth, 0),
        (c"package_length_type", Special, PacketType as u32, Long, 0),
        (c"package_length_offset", Special, LengthOffsetSetting as u32, Long, 0),
        (c"package_body_offset", Special, BodyOffsetSetting as u32, Long, 0),
        (c"package_length_func", Special, LengthCallback as u32, Long, 0),
        (c"package_max_length", Number, MaxLength as u32, Size, 2),
    ]
};
/// One static instruction, returned before each lazy native lookup/conversion/effect.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ports_option(index: usize) -> PortsOption {
    let Some(&(key, action, target, converter, number_kind)) = OPTIONS.get(index) else {
        return PortsOption {
            key: PortsSpan {
                data: std::ptr::null(),
                len: 0,
            },
            alias: PortsSpan {
                data: std::ptr::null(),
                len: 0,
            },
            action: 0,
            target: 0,
            converter: 0,
            number_kind: 0,
            policy: 0,
            reserved: 0,
        };
    };
    let alias = if matches!(action, OptionAction::Special)
        && target == SpecialSetting::BodyOffsetSetting as u32
    {
        c"package_body_start"
    } else {
        c""
    };
    let policy = match (action, target) {
        (OptionAction::Flag, n) if n == ProtocolField::NoDelay as u32 => 1,
        (OptionAction::Special, n) if n == SpecialSetting::WebsocketOptions as u32 => 2,
        _ => 0,
    };
    PortsOption {
        key: PortsSpan {
            data: key.as_ptr().cast(),
            len: key.to_bytes().len(),
        },
        alias: PortsSpan {
            data: alias.as_ptr().cast(),
            len: alias.to_bytes().len(),
        },
        action: action as u32,
        target,
        converter: converter as u32,
        number_kind,
        policy,
        reserved: 0,
    }
}

#[derive(Clone, Copy)]
#[repr(u32)]
enum TlsTarget {
    Compress = 1,
    Protocols = 2,
    VerifyPeer = 3,
    AllowSelfSigned = 4,
    ClientCert = 5,
    CaFile = 6,
    CaPath = 7,
    VerifyDepth = 8,
    PreferCiphers = 9,
    Ciphers = 10,
    EcdhCurve = 11,
    DhParam = 12,
}
type TlsOptionSpec = (&'static CStr, u32, OptionAction, Converter);
static TLS_FILES: [TlsOptionSpec; 2] = [
    (
        c"ssl_cert_file",
        1,
        OptionAction::Special,
        Converter::String,
    ),
    (c"ssl_key_file", 2, OptionAction::Special, Converter::String),
];
#[rustfmt::skip]
static TLS_PORT: [TlsOptionSpec; 12] = {
    use TlsTarget::{Compress, Protocols, VerifyPeer, AllowSelfSigned, ClientCert, CaFile, CaPath, VerifyDepth, PreferCiphers, Ciphers, EcdhCurve, DhParam};
    use OptionAction::{Flag, Number, Special};
    use Converter::{Truth, Long, String};
    [
        (c"ssl_compress", Compress as u32, Flag, Truth),
        (c"ssl_protocols", Protocols as u32, Number, Long),
        (c"ssl_verify_peer", VerifyPeer as u32, Flag, Truth),
        (c"ssl_allow_self_signed", AllowSelfSigned as u32, Flag, Truth),
        (c"ssl_client_cert_file", ClientCert as u32, Special, String),
        (c"ssl_cafile", CaFile as u32, Special, String),
        (c"ssl_capath", CaPath as u32, Special, String),
        (c"ssl_verify_depth", VerifyDepth as u32, Number, Long),
        (c"ssl_prefer_server_ciphers", PreferCiphers as u32, Flag, Truth),
        (c"ssl_ciphers", Ciphers as u32, Special, String),
        (c"ssl_ecdh_curve", EcdhCurve as u32, Special, String),
        (c"ssl_dhparam", DhParam as u32, Special, String),
    ]
};
/// TLS frontend metadata only; native contexts, crypto and SNI algorithms never enter Rust.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ports_tls_option(group: u32, index: usize) -> PortsOption {
    let spec = match group {
        0 => TLS_FILES.get(index),
        1 => TLS_PORT.get(index),
        _ => None,
    };
    let Some(&(key, target, action, converter)) = spec else {
        return PortsOption {
            key: PortsSpan {
                data: std::ptr::null(),
                len: 0,
            },
            alias: PortsSpan {
                data: std::ptr::null(),
                len: 0,
            },
            action: 0,
            target: 0,
            converter: 0,
            number_kind: 0,
            policy: 0,
            reserved: 0,
        };
    };
    PortsOption {
        key: PortsSpan {
            data: key.as_ptr().cast(),
            len: key.to_bytes().len(),
        },
        alias: PortsSpan {
            data: c"".as_ptr().cast(),
            len: 0,
        },
        action: action as u32,
        target,
        converter: converter as u32,
        number_kind: 0,
        policy: 0,
        reserved: 0,
    }
}
/// Exactly the existing bool inversion/normalization and U8 depth clamp, no TLS behavior repair.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ports_tls_value(target: u32, value: i64) -> i64 {
    match target {
        n if n == TlsTarget::Compress as u32 => i64::from(value == 0),
        n if n == TlsTarget::VerifyPeer as u32
            || n == TlsTarget::AllowSelfSigned as u32
            || n == TlsTarget::PreferCiphers as u32 =>
        {
            i64::from(value != 0)
        }
        n if n == TlsTarget::VerifyDepth as u32 => swr_v1_ports_number(4, value),
        _ => value,
    }
}

#[repr(C)]
pub struct PortsSniPlan {
    pub key: PortsSpan,
    pub status: u32,
    pub reserved: u32,
}

/// Frontend mapping admission only; native hash iteration, names, contexts and crypto stay native.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ports_sni_plan(stage: u32, has_key: u32, is_array: u32) -> PortsSniPlan {
    let key = if stage == 0 {
        let name = c"ssl_sni_certs";
        PortsSpan {
            data: name.as_ptr().cast(),
            len: name.to_bytes().len(),
        }
    } else {
        PortsSpan {
            data: std::ptr::null(),
            len: 0,
        }
    };
    let status = match stage {
        1 if is_array == 0 => 1,
        2 if has_key == 0 => 2,
        2 if is_array == 0 => 3,
        _ => 0,
    };
    PortsSniPlan {
        key,
        status,
        reserved: 0,
    }
}

/// Select a native Server callback binding; callback addresses and custody stay native.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ports_callback_hook(index: i32) -> u32 {
    match index {
        0 => 1,  // Connect
        3 => 2,  // Packet
        2 => 3,  // Close
        10 => 4, // BufferFull
        11 => 5, // BufferEmpty
        _ => 0,
    }
}

/// PHP setting truth values/defaults and ordered dependent flag publications.
/// The native caller converts one current zval before calling; no Port snapshot is copied.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ports_flag_setting(
    kind: u32,
    value: i64,
    present: u32,
) -> PortsProtocolPlan {
    use ProtocolField::{
        CheckEof, CheckLength, FastOpen, Http2Flag, HttpFlag, Keepalive, MqttFlag, NoDelay,
        RedisFlag, SplitEof, WebsocketFlag,
    };
    if present == 0 {
        return if kind == NoDelay as u32 {
            protocol_plan([binding(NoDelay, 1)])
        } else {
            protocol_plan([])
        };
    }
    let value = u32::from(value != 0);
    match kind {
        n if n == NoDelay as u32 => protocol_plan([binding(NoDelay, value)]),
        n if n == Keepalive as u32 => protocol_plan([binding(Keepalive, value)]),
        n if n == CheckEof as u32 => protocol_plan([binding(CheckEof, value)]),
        n if n == SplitEof as u32 && value != 0 => {
            protocol_plan([binding(SplitEof, value), binding(CheckEof, 1)])
        }
        n if n == SplitEof as u32 => protocol_plan([binding(SplitEof, value)]),
        n if n == HttpFlag as u32 => protocol_plan([binding(HttpFlag, value)]),
        n if n == WebsocketFlag as u32 && value != 0 => {
            protocol_plan([binding(WebsocketFlag, value), binding(HttpFlag, 1)])
        }
        n if n == WebsocketFlag as u32 => protocol_plan([binding(WebsocketFlag, value)]),
        n if n == Http2Flag as u32 && value != 0 => {
            protocol_plan([binding(Http2Flag, value), binding(HttpFlag, 1)])
        }
        n if n == Http2Flag as u32 => protocol_plan([binding(Http2Flag, value)]),
        n if n == MqttFlag as u32 => protocol_plan([binding(MqttFlag, value)]),
        n if n == RedisFlag as u32 => protocol_plan([binding(RedisFlag, value)]),
        n if n == FastOpen as u32 => protocol_plan([binding(FastOpen, value)]),
        n if n == CheckLength as u32 => protocol_plan([binding(CheckLength, value)]),
        _ => protocol_plan([]),
    }
}

/// Ordered native publications. Callback IDs marshal existing native addresses.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ports_protocol_plan(input: PortsProtocolInput) -> PortsProtocolPlan {
    use ProtocolField::{
        BodyOffset, EofLength, LengthFunction, LengthOffset, LengthSize, LengthSizeFunction,
        Package, Reader,
    };
    match swr_v1_ports_protocol(input.flags, u8::from(input.length_type != 0)) {
        0 => protocol_plan([]),
        1 if input.eof_len > input.eof_max => protocol_plan([
            binding(EofLength, input.eof_max),
            binding(Package, 1),
            binding(Reader, 1),
        ]),
        1 => protocol_plan([binding(Package, 1), binding(Reader, 1)]),
        2 => protocol_plan([
            binding(LengthFunction, 1),
            binding(Package, 1),
            binding(Reader, 2),
        ]),
        10 => protocol_plan([binding(Package, 1), binding(Reader, 2)]),
        3 => protocol_plan([
            binding(LengthOffset, 0),
            binding(BodyOffset, 0),
            binding(Reader, 3),
        ]),
        4 => protocol_plan([
            binding(LengthSize, input.http2_header_size),
            binding(LengthFunction, 3),
            binding(Package, 1),
            binding(LengthOffset, 0),
            binding(BodyOffset, 0),
            binding(Reader, 3),
        ]),
        5 => protocol_plan([
            binding(LengthSize, input.websocket_header_size),
            binding(LengthFunction, 4),
            binding(Package, 3),
            binding(LengthOffset, 0),
            binding(BodyOffset, 0),
            binding(Reader, 3),
        ]),
        6 => protocol_plan([
            binding(LengthFunction, 2),
            binding(LengthSizeFunction, 0),
            binding(Package, 2),
            binding(LengthOffset, 0),
            binding(BodyOffset, 0),
            binding(Reader, 3),
        ]),
        7 => protocol_plan([
            binding(LengthSize, input.mqtt_header_size),
            binding(LengthOffset, 1),
            binding(BodyOffset, 0),
            binding(LengthFunction, 5),
            binding(Package, 1),
            binding(Reader, 2),
        ]),
        8 => protocol_plan([binding(Package, 1), binding(Reader, 4)]),
        _ => protocol_plan([binding(Reader, 5)]),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ports_protocol_name(flags: u32) -> *const c_char {
    let name: &'static [u8] = if flags & 1 != 0 {
        b"dgram\0"
    } else {
        match swr_v1_ports_protocol(flags, 1) {
            1 => b"eof\0",
            2 => b"length\0",
            3 => b"http\0",
            4 => b"http|http2\0",
            5 => b"http|websocket\0",
            6 => b"http|http2|websocket\0",
            7 => b"mqtt\0",
            8 => b"redis\0",
            _ => b"raw\0",
        }
    };
    name.as_ptr().cast()
}

#[repr(C)]
pub struct PortsSpan {
    pub data: *const u8,
    pub len: usize,
}
#[repr(C)]
pub struct PortsEvent {
    pub index: i32,
    pub property: PortsSpan,
}
const EVENTS: &[(&[u8], &[u8])] = &[
    (b"connect", b"onConnect"),
    (b"receive", b"onReceive"),
    (b"close", b"onClose"),
    (b"packet", b"onPacket"),
    (b"request", b"onRequest"),
    (b"handshake", b"onHandshake"),
    (b"beforehandshakeresponse", b"onBeforeHandshakeResponse"),
    (b"open", b"onOpen"),
    (b"message", b"onMessage"),
    (b"disconnect", b"onDisconnect"),
    (b"bufferfull", b"onBufferFull"),
    (b"bufferempty", b"onBufferEmpty"),
];
/// Complete immutable PHP Port declaration metadata. Names share the existing
/// event catalog; no Zend pointer or mutable class/property registry enters Rust.
#[repr(C)]
pub struct PortsProperty {
    pub name: PortsSpan,
    pub value: i64,
    pub kind: u32,
    pub private: u32,
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ports_class_property(index: usize) -> PortsProperty {
    // Declaration order differs from callback enum order and is observable.
    const EVENT_ORDER: [usize; 12] = [0, 1, 2, 3, 10, 11, 4, 5, 7, 8, 9, 6];
    const PUBLIC: &[(&[u8], u32, i64)] = &[
        (b"host", 1, 0),
        (b"port", 2, 0),
        (b"type", 2, 0),
        (b"sock", 2, -1),
        (b"ssl", 3, 0),
        (b"setting", 1, 0),
        (b"connections", 1, 0),
    ];
    let (name, kind, value, private) =
        if let Some(&(_, name)) = EVENT_ORDER.get(index).and_then(|&event| EVENTS.get(event)) {
            (name, 1, 0, 1)
        } else if let Some(&(name, kind, value)) = index
            .checked_sub(EVENT_ORDER.len())
            .and_then(|i| PUBLIC.get(i))
        {
            (name, kind, value, 0)
        } else {
            return PortsProperty {
                name: PortsSpan {
                    data: std::ptr::null(),
                    len: 0,
                },
                value: 0,
                kind: 0,
                private: 0,
            };
        };
    PortsProperty {
        name: PortsSpan {
            data: name.as_ptr(),
            len: name.len(),
        },
        value,
        kind,
        private,
    }
}
unsafe extern "C" {
    fn strncasecmp(a: *const c_char, b: *const c_char, n: usize) -> c_int;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_ports_event(
    data: *const u8,
    len: usize,
    native_casefold: u8,
) -> PortsEvent {
    let missing = PortsEvent {
        index: -1,
        property: PortsSpan {
            data: std::ptr::null(),
            len: 0,
        },
    };
    if len == 0 || data.is_null() || len > isize::MAX as usize {
        return missing;
    }
    // SAFETY: `data` is non-null and the caller lends `len <= isize::MAX` initialized bytes.
    let bytes = unsafe { slice::from_raw_parts(data, len) };
    for (index, &(key, property)) in EVENTS.iter().enumerate() {
        if len == key.len()
            && if native_casefold != 0 {
                // SAFETY: both ranges hold `len` readable bytes (`len == key.len()`).
                unsafe { strncasecmp(data.cast(), key.as_ptr().cast(), len) == 0 }
            } else {
                bytes == key
            }
        {
            return PortsEvent {
                index: i32::try_from(index).unwrap_or(-1),
                property: PortsSpan {
                    data: property.as_ptr(),
                    len: property.len(),
                },
            };
        }
    }
    missing
}

/// Callback getter normalization is Zend ASCII, distinct from registration's libc fold.
/// Only the caller's initialized bytes are borrowed; returned property bytes are static.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_ports_callback_property(data: *const u8, len: usize) -> PortsSpan {
    if !data.is_null() && isize::try_from(len).is_ok() {
        // SAFETY: `data` is non-null and the caller lends `len <= isize::MAX` initialized bytes.
        let name = unsafe { slice::from_raw_parts(data, len) };
        if let Some(&(_, property)) = EVENTS
            .iter()
            .find(|(key, _)| name.eq_ignore_ascii_case(key))
        {
            return PortsSpan {
                data: property.as_ptr(),
                len: property.len(),
            };
        }
    }
    PortsSpan {
        data: std::ptr::null(),
        len: 0,
    }
}

/// A phase captures control flow, never native objects or references.
#[repr(C)]
pub struct PortsSocketState {
    pub phase: u32,
}
#[repr(C)]
pub struct PortsSocketInput {
    pub capabilities: u32,
    pub socket_present: u32,
    pub result: i32,
}
#[repr(C)]
pub struct PortsSocketAction {
    pub kind: u32,
    pub status: i32,
}
fn socket_action(s: &mut PortsSocketState, next: u32, kind: u32) -> PortsSocketAction {
    s.phase = next;
    PortsSocketAction { kind, status: 0 }
}
fn socket_done(status: i32) -> PortsSocketAction {
    PortsSocketAction { kind: 0, status }
}

/// Queries preserve the original server/DTLS read points; cleanup actions terminate natively.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_ports_socket_step(
    state: *mut PortsSocketState,
    input: *const PortsSocketInput,
) -> PortsSocketAction {
    // SAFETY: the caller passes null or its live, exclusive step state.
    let state = unsafe { state.as_mut() };
    // SAFETY: the caller passes null or an initialized input, refreshed after each action.
    let input = unsafe { input.as_ref() };
    let (Some(s), Some(i)) = (state, input) else {
        return socket_done(-1);
    };
    loop {
        match s.phase {
            0 => {
                if i.socket_present != 0 {
                    return if i.capabilities & 1 != 0 {
                        socket_action(s, 1, 1)
                    } else {
                        socket_done(0)
                    };
                }
                s.phase = 3;
            }
            1 => {
                if i.result != 0 {
                    return socket_action(s, 3, 2);
                }
                return socket_done(0);
            }
            3 => return socket_action(s, 4, 3),
            4 => {
                if i.socket_present == 0 {
                    return socket_action(s, 90, 4);
                }
                if i.capabilities & 2 != 0 {
                    return socket_action(s, 5, 5);
                }
                s.phase = 7;
            }
            5 => {
                s.phase = 7;
                if i.result != 0 {
                    return socket_action(s, 7, 6);
                }
            }
            7 => {
                if i.capabilities & 1 != 0 {
                    return socket_action(s, 8, 1);
                }
                s.phase = 10;
            }
            8 => {
                s.phase = 10;
                if i.result != 0 {
                    return socket_action(s, 9, 7);
                }
            }
            9 => {
                if i.result < 0 {
                    return socket_action(s, 90, 8);
                }
                s.phase = 10;
            }
            10 => return socket_action(s, 11, 9),
            11 => {
                if i.result == 0 {
                    return socket_action(s, 12, 10);
                }
                s.phase = 13;
            }
            12 => return socket_action(s, 90, 8),
            13 => return socket_action(s, 14, 11),
            14 => return socket_action(s, 15, 12),
            15 => {
                if i.result < 0 {
                    return socket_action(s, 90, 8);
                }
                return socket_action(s, 16, 13);
            }
            16 => {
                if i.result < 0 {
                    return socket_action(s, 90, 8);
                }
                return socket_action(s, 17, 14);
            }
            17 => return socket_done(0),
            _ => return socket_done(-1),
        }
    }
}
