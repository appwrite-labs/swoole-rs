// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/protocol/base.cc (Swoole contributors).
//! `Protocol::recv_with_length_protocol`, `recv_with_eof_protocol` and
//! `recv_split_by_eof`: [`swr_v1_framing_recv`] with each effect on the socket,
//! the buffer and the protocol's callbacks; and `default_length_func`,
//! `swoole_unpack` and `swoole_hton64`/`swoole_ntoh64`, under their upstream
//! symbols. Logs name the upstream function that wrote them.
use core::ffi::{CStr, c_char, c_int, c_void};
use core::ptr;
use swoole_rs_buffer::string::CppString;
use swoole_rs_clients::address::{swr_v1_clients_address_host, swr_v1_clients_address_port};
use swoole_rs_corebase::net_types::Protocol;
use swoole_rs_corebase::{DataHead, log, swr_v1_core_pagesize};
use swoole_rs_error::ErrorCode;
use swoole_rs_framing::recv::{
    FLAG_DYNAMIC, FLAG_REMOVED, FLAG_SKIP, FLAG_TLS, FLAG_WAIT, MODE_EOF, MODE_LENGTH, MODE_SPLIT,
    OP_CLEAR_SKIP, OP_DISPATCH, OP_EXTEND, OP_GET_LENGTH, OP_LOG_MALFORMED, OP_LOG_NO_LENGTH,
    OP_LOG_TOO_LARGE, OP_PAGE, OP_RECV, OP_REDUCE, OP_SPLIT, OP_SSL_PENDING, RECV_CLOSE, RECV_DONE,
    SwrFramingIo,
};
use swoole_rs_framing::{swr_v1_framing_default, swr_v1_framing_hton64, swr_v1_framing_recv};
use swoole_rs_framing::{swr_v1_framing_unpack, swr_v1_framing_width};
use swoole_rs_reactor::{SocketRecord, swr_v1_socket_get_peer_name};
use swoole_rs_websocket::frame_api::PacketLength;

/// `SW_LOG_WARNING` and `SW_LOG_ERROR`.
const LOG_WARNING: c_int = 4;
const LOG_ERROR: c_int = 5;
/// `catch_read_error`'s `SW_CLOSE` and `SW_ERROR`.
const CLOSE: c_int = 3;
const ERROR: c_int = 4;
const SW_ERR: isize = -1;

/// `swoole::RecvData`.
#[repr(C)]
struct RecvData {
    info: DataHead,
    data: *const u8,
}

type OnPackage = unsafe extern "C" fn(*const Protocol, *mut SocketRecord, *const RecvData) -> c_int;
type GetLength =
    unsafe extern "C" fn(*const Protocol, *mut SocketRecord, *mut PacketLength) -> isize;
type GetLengthSize = unsafe extern "C" fn(*mut SocketRecord) -> u8;

unsafe extern "C" {
    fn SSL_pending(ssl: *const c_void) -> c_int;
}

/// One receive: the shared state [`swr_v1_framing_recv`] reads, and what each
/// effect acts on.
#[repr(C)]
struct Recv {
    io: SwrFramingIo,
    protocol: *const Protocol,
    socket: *mut SocketRecord,
    buffer: *mut CppString,
}

fn socket_flags(socket: &SocketRecord) -> u32 {
    (if socket.removed != 0 { FLAG_REMOVED } else { 0 })
        | (if socket.recv_wait != 0 { FLAG_WAIT } else { 0 })
        | (if socket.ssl.is_null() { 0 } else { FLAG_TLS })
}

fn on_package(protocol: &Protocol) -> Option<OnPackage> {
    // SAFETY: `onPackage` is a function of this type, or null.
    unsafe { core::mem::transmute::<*mut c_void, Option<OnPackage>>(protocol.on_package) }
}

/// `onPackage` over `length` bytes at `data`, or `SW_ERR` without one.
///
/// # Safety
/// The live protocol and socket, and `length` readable bytes at `data`.
unsafe fn dispatch(
    protocol: *const Protocol,
    socket: *mut SocketRecord,
    data: *const u8,
    length: u64,
) -> c_int {
    let packet = RecvData {
        info: DataHead {
            len: u32::try_from(length).unwrap_or(u32::MAX),
            ..DataHead::default()
        },
        data,
    };
    // SAFETY: as the caller promises.
    match on_package(unsafe { &*protocol }) {
        // SAFETY: upstream's call, over the live protocol, socket and packet.
        Some(on_package) => unsafe { on_package(protocol, socket, &raw const packet) },
        None => -1,
    }
}

/// `buffer->split`'s handler: each part to `onPackage`, while the socket stays.
unsafe extern "C" fn split_part(
    context: *const c_void,
    data: *const c_char,
    length: usize,
) -> bool {
    // SAFETY: `effect`'s live receive.
    let r = unsafe { &*context.cast::<Recv>() };
    // SAFETY: the live protocol and socket, and the part inside the buffer.
    let result = unsafe { dispatch(r.protocol, r.socket, data.cast(), length as u64) };
    // SAFETY: the live socket.
    result >= 0 && unsafe { (*r.socket).removed } == 0
}

/// `proto_error`: the warning, with the peer's address (or the socket's own when
/// `getpeername` fails) appended as `<host:port>`.
fn proto_error(socket: &SocketRecord, code: ErrorCode, origin: &CStr, message: &str) {
    let mut address = socket.info;
    // SAFETY: the live socket and a writable address.
    if unsafe { swr_v1_socket_get_peer_name(socket, &raw mut address) } < 0 {
        address = socket.info;
    }
    // SAFETY: the address holds its socket type's member.
    let host = unsafe { swr_v1_clients_address_host(&raw const address) };
    // SAFETY: the address text is NUL-terminated and lives until the next lookup.
    let host = unsafe { CStr::from_ptr(host) }.to_string_lossy();
    // SAFETY: as above.
    let port = unsafe { swr_v1_clients_address_port(&raw const address) };
    log::error_log(
        LOG_WARNING,
        code as c_int,
        origin,
        format_args!("{message}<{host}:{port}>"),
    );
}

/// One effect, with `recv_wait` applied before it and the socket's flags read back
/// after it.
unsafe extern "C" fn call(context: *mut c_void, op: u32, arg: u64) -> i64 {
    // SAFETY: `receive`'s live context; `swr_v1_framing_recv` holds no reference into it.
    let r = unsafe { &mut *context.cast::<Recv>() };
    // SAFETY: the receive's live socket.
    let socket = unsafe { &mut *r.socket };
    socket.recv_wait = u8::from(r.io.flags & FLAG_WAIT != 0);
    let result = effect(r, op, arg);
    // SAFETY: as above; the effect has returned.
    r.io.flags = socket_flags(unsafe { &*r.socket });
    result
}

fn effect(r: &Recv, op: u32, arg: u64) -> i64 {
    // SAFETY: the receive's live buffer; no other reference to it is live.
    let b = unsafe { &mut *r.buffer };
    // SAFETY: the receive's live protocol.
    let p = unsafe { &*r.protocol };
    // SAFETY: the receive's live socket; no other reference to it is live.
    let s = unsafe { &mut *r.socket };
    let length_mode = r.io.mode == MODE_LENGTH;
    let origin = if length_mode {
        c"Protocol::recv_with_length_protocol"
    } else {
        c"Protocol::recv_with_eof_protocol"
    };
    let size = usize::try_from(arg).unwrap_or(usize::MAX);
    match op {
        OP_RECV => {
            let at = b.str.wrapping_add(b.length);
            // SAFETY: the live socket and the buffer's free tail of at least `size` bytes.
            let n = unsafe { swoole_rs_tls::upstream::socket_recv(s, at.cast(), size, 0) };
            if n >= 0 {
                return n as i64;
            }
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            match s.catch_error(errno) {
                ERROR => {
                    if length_mode {
                        log::sys_warning_errno(
                            errno,
                            origin,
                            format_args!("recv({}, {arg}) failed", s.fd),
                        );
                    } else {
                        log::sys_warning_errno(
                            errno,
                            origin,
                            format_args!("recv from socket#{} failed", s.fd),
                        );
                    }
                    RECV_DONE
                }
                CLOSE => RECV_CLOSE,
                _ => RECV_DONE,
            }
        }
        OP_GET_LENGTH => {
            let mut pl = PacketLength {
                buf: b.str.cast_const(),
                buf_size: u32::try_from(b.length).unwrap_or(u32::MAX),
                header_len: 0,
            };
            // SAFETY: `get_package_length` is a function of this type, or null.
            let get = unsafe {
                core::mem::transmute::<*mut c_void, Option<GetLength>>(p.get_package_length)
            };
            // SAFETY: upstream's call over the live protocol, socket and buffer.
            get.map_or(-1, |get| unsafe { get(r.protocol, r.socket, &raw mut pl) }
                as i64)
        }
        OP_EXTEND => {
            b.extend(size);
            0
        }
        // SAFETY: the live protocol and socket, and `arg` bytes at the buffer start.
        OP_DISPATCH => i64::from(unsafe { dispatch(r.protocol, r.socket, b.str, arg) }),
        OP_REDUCE => {
            b.reduce(arg.cast_signed());
            0
        }
        OP_SPLIT => {
            let eof = p.package_eof.as_ptr();
            let eof_len = usize::from(p.package_eof_len);
            let context = ptr::from_ref(r).cast();
            // SAFETY: the EOF holds `eof_len > 0` bytes (the loop checks it); the
            // handler takes this receive.
            unsafe { b.split(eof, eof_len, split_part, context) as i64 }
        }
        // SAFETY: the socket's live SSL.
        OP_SSL_PENDING => i64::from(unsafe { SSL_pending(s.ssl) }),
        OP_CLEAR_SKIP => {
            s.skip_recv = 0;
            0
        }
        OP_PAGE => i64::from(swr_v1_core_pagesize()),
        OP_LOG_MALFORMED => {
            let message = format!(
                "received {} bytes of malformed data, remote_addr=",
                b.length
            );
            proto_error(
                s,
                ErrorCode::SW_ERROR_PACKAGE_MALFORMED_DATA,
                origin,
                &message,
            );
            0
        }
        OP_LOG_NO_LENGTH => {
            log::error_log(
                LOG_WARNING,
                ErrorCode::SW_ERROR_PACKAGE_LENGTH_NOT_FOUND as c_int,
                origin,
                format_args!(
                    "bad request, no length found in {} bytes, remote_addr=",
                    b.length
                ),
            );
            0
        }
        OP_LOG_TOO_LARGE => {
            let message = if length_mode {
                format!(
                    "the received packet length {} is too large, remote_addr=",
                    arg.cast_signed()
                )
            } else {
                format!("The received data packet is too large, length={}", b.length)
            };
            proto_error(
                s,
                ErrorCode::SW_ERROR_PACKAGE_LENGTH_TOO_LARGE,
                origin,
                &message,
            );
            0
        }
        _ => std::process::abort(),
    }
}

/// One receive of `mode` into `buffer`.
///
/// # Safety
/// The live protocol, socket and buffer, as upstream's methods take them.
unsafe fn receive(
    protocol: *const Protocol,
    socket: *mut SocketRecord,
    buffer: *mut CppString,
    mode: u32,
) -> c_int {
    // SAFETY: as the caller promises.
    let p = unsafe { &*protocol };
    // SAFETY: as the caller promises.
    let s = unsafe { &mut *socket };
    let mut r = Recv {
        io: SwrFramingIo {
            protocol: protocol.cast(),
            buffer: buffer.cast(),
            mode,
            head: 0,
            flags: socket_flags(s),
        },
        protocol,
        socket,
        buffer,
    };
    if mode == MODE_LENGTH {
        // SAFETY: `get_package_length_size` is a function of this type, or null.
        let size = unsafe {
            core::mem::transmute::<*mut c_void, Option<GetLengthSize>>(p.get_package_length_size)
        };
        // SAFETY: upstream's call over the live socket.
        r.io.head = u32::from(size.map_or(p.package_length_size, |size| unsafe { size(socket) }));
        r.io.flags |= (if s.skip_recv != 0 { FLAG_SKIP } else { 0 })
            | (if size.is_some() { FLAG_DYNAMIC } else { 0 });
    }
    let context = &raw mut r;
    // SAFETY: `context` is the live local above.
    let io = unsafe { &raw mut (*context).io };
    // SAFETY: the receive's state, its context and its effects.
    let status = unsafe { swr_v1_framing_recv(io, context.cast(), Some(call)) };
    // SAFETY: the live socket; the receive has returned.
    unsafe { (*socket).recv_wait = u8::from(r.io.flags & FLAG_WAIT != 0) };
    status
}

/// `int Protocol::recv_split_by_eof(Socket *, String *) const`.
///
/// # Safety
/// As [`receive`].
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZNK6swoole8Protocol17recv_split_by_eofEPNS_7network6SocketEPNS_6StringE"
    )
)]
pub unsafe extern "C" fn recv_split_by_eof(
    protocol: *const Protocol,
    socket: *mut SocketRecord,
    buffer: *mut CppString,
) -> c_int {
    // SAFETY: as the caller promises.
    unsafe { receive(protocol, socket, buffer, MODE_SPLIT) }
}

/// `int Protocol::recv_with_length_protocol(Socket *, String *) const`.
///
/// # Safety
/// As [`receive`].
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZNK6swoole8Protocol25recv_with_length_protocolEPNS_7network6SocketEPNS_6StringE"
    )
)]
pub unsafe extern "C" fn recv_with_length_protocol(
    protocol: *const Protocol,
    socket: *mut SocketRecord,
    buffer: *mut CppString,
) -> c_int {
    // SAFETY: as the caller promises.
    unsafe { receive(protocol, socket, buffer, MODE_LENGTH) }
}

/// `int Protocol::recv_with_eof_protocol(Socket *, String *) const`.
///
/// # Safety
/// As [`receive`].
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZNK6swoole8Protocol22recv_with_eof_protocolEPNS_7network6SocketEPNS_6StringE"
    )
)]
pub unsafe extern "C" fn recv_with_eof_protocol(
    protocol: *const Protocol,
    socket: *mut SocketRecord,
    buffer: *mut CppString,
) -> c_int {
    // SAFETY: as the caller promises.
    unsafe { receive(protocol, socket, buffer, MODE_EOF) }
}

/// `static ssize_t Protocol::default_length_func(const Protocol *, Socket *, PacketLength *)`.
///
/// # Safety
/// The live protocol, and a packet whose `buf` holds `buf_size` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZN6swoole8Protocol19default_length_funcEPKS0_PNS_7network6SocketEPNS_12PacketLengthE"
    )
)]
pub unsafe extern "C" fn default_length_func(
    protocol: *const Protocol,
    socket: *mut SocketRecord,
    pl: *mut PacketLength,
) -> isize {
    // SAFETY: as the caller promises.
    let p = unsafe { &*protocol };
    // SAFETY: as the caller promises.
    let pl = unsafe { &mut *pl };
    let size = if p.get_package_length_size.is_null() {
        p.package_length_size
    } else {
        // SAFETY: a non-null `get_package_length_size` is a function of this type.
        let size = unsafe {
            core::mem::transmute::<*mut c_void, GetLengthSize>(p.get_package_length_size)
        };
        // SAFETY: upstream's call over the live socket.
        unsafe { size(socket) }
    };
    let mut value = 0;
    // SAFETY: the packet's bytes and a writable result.
    let result = unsafe {
        swr_v1_framing_default(
            p.package_length_type,
            pl.buf,
            pl.buf_size as usize,
            u32::from(p.package_length_offset),
            u32::from(size),
            u32::from(p.package_body_offset),
            &raw mut value,
        )
    };
    if result == -2 {
        invalid_format(p.package_length_type);
    }
    match result {
        0 => {
            pl.header_len = u32::try_from(value).unwrap_or(u32::MAX);
            0
        }
        1 => {
            pl.header_len = u32::from(p.package_length_size);
            isize::try_from(value).unwrap_or(SW_ERR)
        }
        _ => SW_ERR,
    }
}

fn invalid_format(format: u8) {
    log::error_log(
        LOG_ERROR,
        ErrorCode::SW_ERROR_INVALID_PARAMS as c_int,
        c"swoole_unpack",
        format_args!("Invalid format specifier '{}'", char::from(format)),
    );
}

/// `int64_t swoole_unpack(char, const void *)`: -1 (logged) for an unknown format.
///
/// # Safety
/// `data` holds the format's width of readable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z13swoole_unpackcPKv")
)]
#[must_use]
pub unsafe extern "C" fn swoole_unpack(format: u8, data: *const c_void) -> i64 {
    let mut value = -1;
    // SAFETY: as the caller promises.
    let result = unsafe {
        swr_v1_framing_unpack(
            format,
            data.cast(),
            swr_v1_framing_width(format),
            &raw mut value,
        )
    };
    if result == -1 {
        invalid_format(format);
    }
    value
}

/// `uint64_t swoole_hton64(uint64_t)`.
#[cfg_attr(
    all(feature = "upstream-symbols", target_os = "macos"),
    unsafe(export_name = "_Z13swoole_hton64y")
)]
#[cfg_attr(
    all(feature = "upstream-symbols", not(target_os = "macos")),
    unsafe(export_name = "_Z13swoole_hton64m")
)]
#[must_use]
pub extern "C" fn swoole_hton64(value: u64) -> u64 {
    swr_v1_framing_hton64(value)
}

/// `uint64_t swoole_ntoh64(uint64_t)`.
#[cfg_attr(
    all(feature = "upstream-symbols", target_os = "macos"),
    unsafe(export_name = "_Z13swoole_ntoh64y")
)]
#[cfg_attr(
    all(feature = "upstream-symbols", not(target_os = "macos")),
    unsafe(export_name = "_Z13swoole_ntoh64m")
)]
#[must_use]
pub extern "C" fn swoole_ntoh64(value: u64) -> u64 {
    swr_v1_framing_hton64(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static PACKAGES: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
    }

    unsafe extern "C" fn record(
        _: *const Protocol,
        _: *mut SocketRecord,
        packet: *const RecvData,
    ) -> c_int {
        // SAFETY: the receive's packet, `len` bytes at `data`.
        let bytes =
            unsafe { core::slice::from_raw_parts((*packet).data, (*packet).info.len as usize) };
        PACKAGES.with_borrow_mut(|p| p.push(bytes.to_vec()));
        0
    }

    /// A connected socket with `sent` waiting, the protocol and an 8-byte buffer.
    fn stream(sent: &[u8]) -> (SocketRecord, c_int, Protocol, CppString) {
        let mut pair = [0; 2];
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pair.as_mut_ptr()) },
            0
        );
        assert_eq!(
            unsafe { libc::send(pair[0], sent.as_ptr().cast(), sent.len(), 0) },
            sent.len().cast_signed()
        );
        unsafe { libc::fcntl(pair[1], libc::F_SETFL, libc::O_NONBLOCK) };
        let mut socket = SocketRecord::new();
        socket.fd = pair[1];
        socket.socket_type = 5; // SW_SOCK_UNIX_STREAM
        let mut protocol: Protocol = unsafe { core::mem::zeroed() };
        protocol.package_max_length = 1024;
        protocol.on_package = record as *mut c_void;
        let mut buffer = CppString::EMPTY;
        buffer.alloc(8, core::ptr::null());
        PACKAGES.with_borrow_mut(Vec::clear);
        (socket, pair[0], protocol, buffer)
    }

    #[test]
    fn length_protocol_with_the_default_length() {
        let (mut socket, peer, mut protocol, mut buffer) =
            stream(b"\x00\x03abc\x00\x0chello world!");
        protocol.package_length_type = b'n';
        protocol.package_length_size = 2;
        protocol.package_body_offset = 2;
        protocol.get_package_length = default_length_func as *mut c_void;
        let status = unsafe {
            recv_with_length_protocol(&raw const protocol, &raw mut socket, &raw mut buffer)
        };
        assert_eq!(status, 0);
        let status = unsafe {
            recv_with_length_protocol(&raw const protocol, &raw mut socket, &raw mut buffer)
        };
        assert_eq!(status, 0);
        PACKAGES.with_borrow(|p| {
            assert_eq!(
                *p,
                [b"\x00\x03abc".to_vec(), b"\x00\x0chello world!".to_vec()]
            );
        });
        unsafe { libc::close(peer) };
        // The peer's close ends the next receive.
        let status = unsafe {
            recv_with_length_protocol(&raw const protocol, &raw mut socket, &raw mut buffer)
        };
        assert_eq!(status, -1);
        buffer.dispose();
        unsafe { libc::close(socket.fd) };
    }

    #[test]
    fn eof_protocol_splits_and_keeps_the_rest() {
        let (mut socket, peer, mut protocol, mut buffer) = stream(b"ab\r\ncd\r\ne");
        protocol.package_eof[..2].copy_from_slice(b"\r\n");
        protocol.package_eof_len = 2;
        protocol.split_by_eof = true;
        buffer.extend(64);
        let status = unsafe {
            recv_with_eof_protocol(&raw const protocol, &raw mut socket, &raw mut buffer)
        };
        assert_eq!(status, 0);
        PACKAGES.with_borrow(|p| assert_eq!(*p, [b"ab\r\n".to_vec(), b"cd\r\n".to_vec()]));
        assert_eq!(buffer.length, 1);
        buffer.dispose();
        unsafe { libc::close(peer) };
        unsafe { libc::close(socket.fd) };
    }

    // core-tests/src/protocol/base.cpp's protocol.swap_byte_order, protocol.unpack and
    // protocol.hton64, with the same inputs and expected values. upstream's
    // swoole_swap_endian{16,32,64} are the integers' `swap_bytes`.

    fn unpack(format: u8, buffer: &[u8]) -> i64 {
        // SAFETY: every buffer here holds 8 readable bytes, the widest format.
        unsafe { swoole_unpack(format, buffer.as_ptr().cast()) }
    }

    /// `createBinaryData`: the value's native bytes at the front of an 8-byte buffer.
    fn data(bytes: &[u8]) -> [u8; 8] {
        let mut buffer = [0; 8];
        buffer[..bytes.len()].copy_from_slice(bytes);
        buffer
    }

    #[test]
    fn swap_byte_order() {
        assert_eq!(0x1234u16.swap_bytes(), 0x3412);
        assert_eq!(0x0001u16.swap_bytes(), 0x0100);
        assert_eq!(0x00FFu16.swap_bytes(), 0xFF00);
        assert_eq!(0xFF00u16.swap_bytes(), 0x00FF);
        assert_eq!(0xFFFFu16.swap_bytes(), 0xFFFF);

        assert_eq!(0x1234_5678u32.swap_bytes(), 0x7856_3412);
        assert_eq!(0x0000_0001u32.swap_bytes(), 0x0100_0000);
        assert_eq!(0x0000_FF00u32.swap_bytes(), 0x00FF_0000);
        assert_eq!(0xFF00_0000u32.swap_bytes(), 0x0000_00FF);
        assert_eq!(0xFFFF_FFFFu32.swap_bytes(), 0xFFFF_FFFF);

        let v = 0xABCDu16;
        assert_eq!(v.swap_bytes().swap_bytes(), v);
        let v = 0xABCD_EF01u32;
        assert_eq!(v.swap_bytes().swap_bytes(), v);

        let converted = 0x1122_3344_5566_7788u64.swap_bytes();
        assert_eq!(
            converted.to_ne_bytes(),
            [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]
        );
    }

    #[test]
    #[expect(clippy::cast_possible_wrap, reason = "upstream's (int64_t) casts")]
    #[expect(clippy::cognitive_complexity, reason = "one upstream TEST, kept whole")]
    fn unpack_formats() {
        // 8-bit
        assert_eq!(unpack(b'c', &data(&(-42i8).to_ne_bytes())), -42);
        assert_eq!(unpack(b'C', &data(&200u8.to_ne_bytes())), 200);
        assert_eq!(unpack(b'c', &data(&i8::MIN.to_ne_bytes())), i8::MIN.into());
        assert_eq!(unpack(b'c', &data(&i8::MAX.to_ne_bytes())), i8::MAX.into());
        assert_eq!(unpack(b'C', &data(&u8::MAX.to_ne_bytes())), u8::MAX.into());

        // 16-bit
        assert_eq!(unpack(b's', &data(&(-12345i16).to_ne_bytes())), -12345);
        assert_eq!(unpack(b'S', &data(&54321u16.to_ne_bytes())), 54321);
        let n_val = 0x1234u16;
        let n_be = n_val.rotate_left(8);
        assert_eq!(unpack(b'n', &data(&n_be.to_ne_bytes())), 0x1234);
        assert_eq!(unpack(b'v', &data(&0x1234u16.to_ne_bytes())), 0x1234);
        assert_eq!(
            unpack(b's', &data(&i16::MIN.to_ne_bytes())),
            i16::MIN.into()
        );
        assert_eq!(
            unpack(b's', &data(&i16::MAX.to_ne_bytes())),
            i16::MAX.into()
        );
        assert_eq!(
            unpack(b'S', &data(&u16::MAX.to_ne_bytes())),
            u16::MAX.into()
        );

        // 32-bit
        assert_eq!(
            unpack(b'l', &data(&(-123_456_789i32).to_ne_bytes())),
            -123_456_789
        );
        assert_eq!(
            unpack(b'L', &data(&3_000_000_000u32.to_ne_bytes())),
            3_000_000_000
        );
        let n_val = 0x1234_5678u32;
        let n_be = ((n_val & 0xFF) << 24)
            | ((n_val & 0xFF00) << 8)
            | ((n_val & 0x00FF_0000) >> 8)
            | ((n_val & 0xFF00_0000) >> 24);
        assert_eq!(unpack(b'N', &data(&n_be.to_ne_bytes())), 0x1234_5678);
        assert_eq!(
            unpack(b'V', &data(&0x1234_5678u32.to_ne_bytes())),
            0x1234_5678
        );
        assert_eq!(
            unpack(b'l', &data(&i32::MIN.to_ne_bytes())),
            i32::MIN.into()
        );
        assert_eq!(
            unpack(b'l', &data(&i32::MAX.to_ne_bytes())),
            i32::MAX.into()
        );
        assert_eq!(
            unpack(b'L', &data(&u32::MAX.to_ne_bytes())),
            u32::MAX.into()
        );

        // 64-bit
        assert_eq!(
            unpack(b'q', &data(&(-1_234_567_890_123_456_789i64).to_ne_bytes())),
            -1_234_567_890_123_456_789
        );
        assert_eq!(
            unpack(b'Q', &data(&10_234_567_890_123_456_789u64.to_ne_bytes())),
            10_234_567_890_123_456_789u64 as i64
        );
        let j_be = 0x1234_5678_9ABC_DEF0u64.swap_bytes();
        assert_eq!(
            unpack(b'J', &data(&j_be.to_ne_bytes())),
            0x1234_5678_9ABC_DEF0
        );
        assert_eq!(
            unpack(b'P', &data(&0x1234_5678_9ABC_DEF0u64.to_ne_bytes())),
            0x1234_5678_9ABC_DEF0
        );
        assert_eq!(unpack(b'q', &data(&i64::MIN.to_ne_bytes())), i64::MIN);
        assert_eq!(unpack(b'q', &data(&i64::MAX.to_ne_bytes())), i64::MAX);
        assert_eq!(
            unpack(b'Q', &data(&u64::MAX.to_ne_bytes())),
            u64::MAX as i64
        );

        // Machine-dependent int
        assert_eq!(
            unpack(b'i', &data(&(-987_654_321i32).to_ne_bytes())),
            -987_654_321
        );
        assert_eq!(
            unpack(b'I', &data(&3_000_000_000u32.to_ne_bytes())),
            3_000_000_000
        );
        assert_eq!(
            unpack(b'i', &data(&i32::MIN.to_ne_bytes())),
            i32::MIN.into()
        );
        assert_eq!(
            unpack(b'i', &data(&i32::MAX.to_ne_bytes())),
            i32::MAX.into()
        );
        assert_eq!(
            unpack(b'I', &data(&u32::MAX.to_ne_bytes())),
            u32::MAX.into()
        );

        // Invalid format specifiers
        let zero = [0u8; 8];
        assert_eq!(unpack(b'x', &zero), -1);
        assert_eq!(unpack(b'?', &zero), -1);
        assert_eq!(unpack(b'Z', &zero), -1);

        // Endianness
        assert_eq!(unpack(b'n', &data(&[0x12, 0x34])), 0x1234);
        assert_eq!(unpack(b'v', &data(&[0x34, 0x12])), 0x1234);
        assert_eq!(unpack(b'N', &data(&[0x12, 0x34, 0x56, 0x78])), 0x1234_5678);
        assert_eq!(unpack(b'V', &data(&[0x78, 0x56, 0x34, 0x12])), 0x1234_5678);
        let be = [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0];
        assert_eq!(unpack(b'J', &be), 0x1234_5678_9ABC_DEF0);
        let le = [0xF0, 0xDE, 0xBC, 0x9A, 0x78, 0x56, 0x34, 0x12];
        assert_eq!(unpack(b'P', &le), 0x1234_5678_9ABC_DEF0);

        // htons, htonl and swoole_hton64 into n, N and J
        assert_eq!(
            unpack(b'n', &data(&0x1234u16.to_be().to_ne_bytes())),
            0x1234
        );
        assert_eq!(
            unpack(b'N', &data(&0x1234_5678u32.to_be().to_ne_bytes())),
            0x1234_5678
        );
        let be64 = swoole_hton64(0x1234_5678_9ABC_DEF0);
        assert_eq!(
            unpack(b'J', &data(&be64.to_ne_bytes())),
            0x1234_5678_9ABC_DEF0
        );
    }

    #[test]
    fn hton64() {
        let val = 0x1122_3344_5566_7788u64;
        let converted = swoole_hton64(val);
        assert_eq!(
            converted.to_ne_bytes(),
            [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]
        );
        assert_eq!(swoole_ntoh64(converted), val);

        let min_converted = swoole_hton64(0);
        assert_eq!(min_converted.to_ne_bytes(), [0x00; 8]);
        assert_eq!(swoole_ntoh64(min_converted), 0);
        let max_converted = swoole_hton64(u64::MAX);
        assert_eq!(max_converted.to_ne_bytes(), [0xFF; 8]);
        assert_eq!(swoole_ntoh64(max_converted), u64::MAX);

        for pattern in [0xAAAA_AAAA_AAAA_AAAAu64, 0x5555_5555_5555_5555] {
            assert_eq!(swoole_ntoh64(swoole_hton64(pattern)), pattern);
        }
        for i in 0..8 {
            let single_byte = 0xFFu64 << (i * 8);
            assert_eq!(
                swoole_ntoh64(swoole_hton64(single_byte)),
                single_byte,
                "Failed for byte position {i}"
            );
        }

        for _ in 0..100 {
            let random_val = swoole_rs_corebase::swoole_random_int();
            assert_eq!(
                swoole_ntoh64(swoole_hton64(random_val)),
                random_val,
                "Failed for random value: 0x{random_val:x}"
            );
        }

        let converted = swoole_hton64(0x0102_0304_0506_0708);
        assert_eq!(converted.to_ne_bytes(), [1, 2, 3, 4, 5, 6, 7, 8]);

        for _ in 0..100 {
            let val = swoole_rs_corebase::swoole_random_int();
            assert_eq!(
                swoole_ntoh64(swoole_hton64(val)),
                val,
                "hton64->ntoh64 failed for 0x{val:x}"
            );
            assert_eq!(
                swoole_hton64(swoole_ntoh64(val)),
                val,
                "ntoh64->hton64 failed for 0x{val:x}"
            );
        }
    }
}
