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
use std::ffi::{c_char, c_int};
use std::slice;

pub mod http_routes;
pub mod lifecycle;
pub mod methods;
pub mod read;
#[cfg(not(test))]
mod read_native;
mod resp_recv;

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
