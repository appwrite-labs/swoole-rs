// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/protocol/base.cc (Swoole contributors).
//! `Protocol::recv_with_length_protocol`, `recv_with_eof_protocol` and
//! `recv_split_by_eof`: [`swoole_rs_framing::recv::recv`] with each effect on the socket,
//! the buffer and the protocol's callbacks; `default_length_func`, under its upstream
//! symbol; and `swoole_unpack` and `swoole_hton64`/`swoole_ntoh64`. Logs name the
//! upstream function that wrote them.
use core::ffi::{CStr, c_int, c_void};
use core::marker::PhantomData;
use core::sync::atomic::Ordering;
use swoole_rs_buffer::string::CppString;
use swoole_rs_clients::client::Framing;
use swoole_rs_corebase::constants::FdType::SW_FD_SESSION;
use swoole_rs_corebase::net_types::{PacketLength, Protocol};
use swoole_rs_corebase::{DataHead, log, swr_v1_core_pagesize};
use swoole_rs_error::ErrorCode;
use swoole_rs_framing::recv::{self, Buffer, Effects, recv};
use swoole_rs_framing::recv::{
    FLAG_DYNAMIC, FLAG_PAUSED, FLAG_REMOVED, FLAG_SKIP, FLAG_TLS, FLAG_WAIT, Mode, Op, RECV_CLOSE,
    RECV_DONE,
};
use swoole_rs_framing::{DefaultLength, UnpackError, default_length, unpack};
use swoole_rs_reactor::{Socket, SocketFields, swr_v1_socket_reset_on_close};
use swoole_rs_server::ConnectionRecord;

/// `SW_LOG_WARNING` and `SW_LOG_ERROR`.
const LOG_WARNING: c_int = 4;
const LOG_ERROR: c_int = 5;
/// `catch_read_error`'s `SW_CLOSE` and `SW_ERROR`.
const CLOSE: c_int = swoole_rs_corebase::constants::ReturnCode::SW_CLOSE as c_int;
const ERROR: c_int = swoole_rs_corebase::constants::ReturnCode::SW_ERROR as c_int;
const SW_ERR: isize = swoole_rs_corebase::constants::ResultCode::SW_ERR as isize;

/// `swoole::RecvData`.
#[repr(C)]
struct RecvData {
    info: DataHead,
    data: *const u8,
}

/// One receive: what each effect acts on, live for the receive.
pub(crate) struct Recv<'a> {
    mode: Mode,
    pub(crate) protocol: *const Protocol,
    pub(crate) socket: Socket<'a>,
    buffer: *mut CppString,
}

impl Recv<'_> {
    fn protocol(&self) -> &Protocol {
        // SAFETY: the receive's live protocol.
        unsafe { &*self.protocol }
    }
    fn string(&self) -> &CppString {
        // SAFETY: the receive's live buffer, changed only inside an effect.
        unsafe { &*self.buffer }
    }
}

/// How a length receive measures and hands on each package.
pub(crate) trait Packages {
    /// The fixed length header size, or `None` to ask the protocol's callbacks.
    const HEAD: Option<u32>;
    /// `get_package_length` over the buffered `bytes`.
    fn length(r: &Recv<'_>, p: &Protocol, bytes: &[u8]) -> i64;
    /// `onPackage` over the buffer `b`'s `offset` bytes (the awaited package) from byte `start`.
    fn dispatch(r: &Recv<'_>, b: &CppString, start: usize) -> c_int;
}

/// The protocol's own callbacks, as upstream calls them.
pub(crate) struct Callbacks;

impl Packages for Callbacks {
    const HEAD: Option<u32> = None;
    #[inline(always)]
    fn length(r: &Recv<'_>, p: &Protocol, bytes: &[u8]) -> i64 {
        let mut pl = PacketLength::new(bytes);
        let Some(get) = p.get_package_length else {
            return -1;
        };
        // SAFETY: upstream's call over the live protocol, socket and buffer.
        let length = unsafe { get(r.protocol, r.socket.as_ptr().cast(), &mut pl) };
        length as i64
    }
    #[inline(always)]
    fn dispatch(r: &Recv<'_>, b: &CppString, start: usize) -> c_int {
        let length = b.offset().unsigned_abs();
        // SAFETY: the live protocol and socket, and the package's `length` bytes from
        // `start`, inside the buffer (the loop dispatches only a whole one).
        unsafe {
            dispatch(
                r.protocol(),
                r.socket,
                b.as_ptr().wrapping_add(start),
                length,
            )
        }
    }
}

/// A [`Recv`] whose packages `K` measures and hands on, and the `recv_wait` the
/// socket holds: only this receive writes it, so a flag that didn't change costs no edit.
struct With<'a, K>(Recv<'a>, bool, PhantomData<K>);

impl<K> With<'_, K> {
    /// `socket->recv_wait = flags & FLAG_WAIT`, when it differs.
    #[inline(always)]
    fn apply_wait(&mut self, flags: u32) {
        let wait = flags & FLAG_WAIT != 0;
        if wait != self.1 {
            self.1 = wait;
            self.0.socket.edit(|s| s.recv_wait = wait);
        }
    }
}

fn socket_flags(socket: &SocketFields) -> u32 {
    (if socket.removed { FLAG_REMOVED } else { 0 })
        | (if socket.recv_wait { FLAG_WAIT } else { 0 })
        | (if socket.ssl().is_null() { 0 } else { FLAG_TLS })
        | (if paused(socket) { FLAG_PAUSED } else { 0 })
}

/// A session socket out of the reactor whose connection no one is closing: `pause`
/// took it out to stop reading.
fn paused(socket: &SocketFields) -> bool {
    if !socket.removed || socket.fd_type != SW_FD_SESSION as c_int || socket.object.is_null() {
        return false;
    }
    // SAFETY: a session socket's object is its live connection.
    let conn = unsafe { &*socket.object.cast::<ConnectionRecord>() };
    conn.is_active()
        && conn.closed.load(Ordering::Relaxed) == 0
        && !(conn.closing
            || conn.close_actively
            || conn.close_queued
            || conn.close_reset
            || conn.close_force
            || conn.peer_closed)
}

/// `onPackage` over `length` bytes at `data`, or `SW_ERR` without one.
///
/// # Safety
/// `length` readable bytes at `data`, a package of the socket `protocol` reads.
unsafe fn dispatch(protocol: &Protocol, socket: Socket<'_>, data: *const u8, length: u64) -> c_int {
    let packet = RecvData {
        info: DataHead {
            len: u32::try_from(length).unwrap_or(u32::MAX),
            ..DataHead::default()
        },
        data,
    };
    match protocol.on_package {
        // SAFETY: upstream's call, over the live protocol, socket and packet.
        Some(on_package) => unsafe {
            on_package(protocol, socket.as_ptr().cast(), (&raw const packet).cast())
        },
        None => -1,
    }
}

/// `proto_error`: the warning, with the peer's address (or the socket's own when
/// `getpeername` fails) appended as `<host:port>`.
fn proto_error(socket: Socket<'_>, code: ErrorCode, origin: &CStr, message: &str) {
    let address = socket.read(|socket| {
        let mut address = socket.info;
        if socket.get_peer_name(&mut address) < 0 {
            address = socket.info;
        }
        address
    });
    let (host, port) = crate::http_routes::host_port(&address);
    log::error_log(
        LOG_WARNING,
        code as c_int,
        origin,
        format_args!("{message}<{host}:{port}>"),
    );
}

impl<K: Packages> Effects for With<'_, K> {
    #[inline(always)]
    fn effect(&mut self, op: Op, arg: u64, flags: &mut u32) -> i64 {
        self.apply_wait(*flags);
        let result = effect::<K>(&self.0, op, arg);
        // A read or a length leaves the socket as it was; a callback or a failure
        // may remove, pause or close it.
        if !matches!(op, Op::Recv | Op::GetLength) || result < 0 {
            *flags = self.0.socket.read(socket_flags);
            self.1 = *flags & FLAG_WAIT != 0;
        }
        result
    }
    #[inline(always)]
    fn buffer(&self) -> Buffer {
        let b = self.0.string();
        Buffer {
            length: b.len(),
            size: b.size(),
            offset: b.offset(),
        }
    }
    #[inline(always)]
    fn set_buffer(&mut self, length: usize, offset: i64) {
        // SAFETY: the receive's live buffer; no other reference to it is live.
        let b = unsafe { &mut *self.0.buffer };
        // SAFETY: framing::recv counts only the bytes its receives wrote.
        unsafe { b.set_len(length) };
        b.set_offset(offset);
    }
    #[inline(always)]
    fn data(&self) -> &[u8] {
        self.0.string().view()
    }
    #[inline(always)]
    fn protocol(&self) -> recv::Protocol {
        let p = self.0.protocol();
        recv::Protocol {
            split_by_eof: p.split_by_eof,
            package_eof: p.package_eof,
            package_eof_len: p.package_eof_len,
            package_length_offset: p.package_length_offset,
            package_max_length: p.package_max_length,
        }
    }
}

/// Each call site's op is a constant, so the match folds: every frame's receive,
/// length and dispatch run inline, and a failed receive and the rare ops don't.
#[inline(always)]
fn effect<K: Packages>(r: &Recv<'_>, op: Op, arg: u64) -> i64 {
    // SAFETY: the receive's live buffer; no other reference to it is live.
    let b = unsafe { &mut *r.buffer };
    let p = r.protocol();
    let s = r.socket;
    match op {
        Op::Recv => {
            let size = usize::try_from(arg).unwrap_or(usize::MAX);
            // The loop asks for at most the buffer's free tail.
            let tail = b.spare().get_mut(..size).unwrap_or_default();
            let n = swoole_rs_tls::upstream::recv(s, tail, 0);
            if n >= 0 {
                return n as i64;
            }
            rare(r, b, p, s, op, arg)
        }
        Op::GetLength => K::length(r, p, b.view().get(start(arg)..).unwrap_or_default()),
        Op::Dispatch => i64::from(K::dispatch(r, b, start(arg))),
        _ => rare(r, b, p, s, op, arg),
    }
}

/// A byte position the loop passed as an effect's arg.
#[inline(always)]
fn start(arg: u64) -> usize {
    usize::try_from(arg).unwrap_or(usize::MAX)
}

/// A failed `Op::Recv`, and the effects a frame rarely needs.
#[cold]
#[inline(never)]
fn rare(r: &Recv<'_>, b: &mut CppString, p: &Protocol, s: Socket<'_>, op: Op, arg: u64) -> i64 {
    let length_mode = r.mode == Mode::Length;
    let origin = if length_mode {
        c"Protocol::recv_with_length_protocol"
    } else {
        c"Protocol::recv_with_eof_protocol"
    };
    let size = usize::try_from(arg).unwrap_or(usize::MAX);
    match op {
        Op::Recv => {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            match s.catch_error(errno) {
                ERROR => {
                    if length_mode {
                        log::sys_warning_errno(
                            errno,
                            origin,
                            format_args!("recv({}, {arg}) failed", s.fd()),
                        );
                    } else {
                        log::sys_warning_errno(
                            errno,
                            origin,
                            format_args!("recv from socket#{} failed", s.fd()),
                        );
                    }
                    RECV_DONE
                }
                CLOSE => RECV_CLOSE,
                _ => RECV_DONE,
            }
        }
        Op::Extend => {
            b.extend(size);
            0
        }
        Op::Reduce => {
            b.reduce(arg.cast_signed());
            0
        }
        Op::Split => {
            // The protocol's settings bound the length by the array.
            let Some(eof) = p.package_eof.get(..usize::from(p.package_eof_len)) else {
                return -1;
            };
            // Each part to `onPackage`, while the socket stays.
            b.split(eof, |data, length| {
                // SAFETY: the live protocol and socket, and the part inside the buffer.
                let result = unsafe { dispatch(p, s, data, length as u64) };
                result >= 0 && !s.read(|s| s.removed)
            }) as i64
        }
        Op::SslPending => swoole_rs_tls::Ssl::of(s).map_or(0, |ssl| i64::from(ssl.pending())),
        Op::ClearSkip => {
            s.edit(|s| s.skip_recv = false);
            0
        }
        Op::Page => i64::from(swr_v1_core_pagesize()),
        Op::LogMalformed => {
            let message = format!("received {} bytes of malformed data, remote_addr=", b.len());
            proto_error(
                s,
                ErrorCode::SW_ERROR_PACKAGE_MALFORMED_DATA,
                origin,
                &message,
            );
            0
        }
        Op::LogNoLength => {
            log::error_log(
                LOG_WARNING,
                ErrorCode::SW_ERROR_PACKAGE_LENGTH_NOT_FOUND as c_int,
                origin,
                format_args!(
                    "bad request, no length found in {} bytes, remote_addr=",
                    b.len()
                ),
            );
            0
        }
        Op::LogTooLarge => {
            let message = if length_mode {
                format!(
                    "the received packet length {} is too large, remote_addr=",
                    arg.cast_signed()
                )
            } else {
                format!("The received data packet is too large, length={}", b.len())
            };
            proto_error(
                s,
                ErrorCode::SW_ERROR_PACKAGE_LENGTH_TOO_LARGE,
                origin,
                &message,
            );
            0
        }
        Op::Reset => i64::from(swr_v1_socket_reset_on_close(s.fd())) - 1,
        _ => swoole_rs_corebase::fatal("an unknown framing op"),
    }
}

/// One receive of `mode` from `socket` into `buffer`.
///
/// # Safety
/// The live protocol and buffer, as upstream's methods take them.
#[inline(always)]
pub(crate) unsafe fn receive<K: Packages>(
    protocol: *const Protocol,
    socket: Socket<'_>,
    buffer: *mut CppString,
    mode: Mode,
) -> c_int {
    let mut r = With::<K>(
        Recv {
            mode,
            protocol,
            socket,
            buffer,
        },
        false,
        PhantomData,
    );
    let p = r.0.protocol();
    let (mut flags, skip) = socket.read(|s| (socket_flags(s), s.skip_recv));
    r.1 = flags & FLAG_WAIT != 0;
    let mut head = 0;
    if let Some(fixed) = K::HEAD {
        head = fixed;
        flags |= if skip { FLAG_SKIP } else { 0 };
    } else if mode == Mode::Length {
        let size = p.get_package_length_size;
        // SAFETY: upstream's call over the live socket.
        head = u32::from(size.map_or(p.package_length_size, |size| unsafe {
            size(socket.as_ptr().cast())
        }));
        flags |=
            (if skip { FLAG_SKIP } else { 0 }) | (if size.is_some() { FLAG_DYNAMIC } else { 0 });
    }
    let status = recv(mode, head, &mut flags, &mut r);
    r.apply_wait(flags);
    status as c_int
}

/// `int Protocol::recv_with_length_protocol(Socket *, String *) const` when `LENGTH`, else
/// `int Protocol::recv_with_eof_protocol(Socket *, String *) const`.
fn client_recv<const LENGTH: bool>(
    protocol: *const Protocol,
    socket: Socket<'_>,
    buffer: *mut CppString,
) -> c_int {
    let mode = if LENGTH { Mode::Length } else { Mode::Eof };
    // SAFETY: the live protocol and input buffer `Framing::new` asks for.
    unsafe { receive::<Callbacks>(protocol, socket, buffer, mode) }
}

/// The framed receives a client's reads use, which `swoole_init` registers
/// (`swoole_rs_clients::client::set_framing`).
// SAFETY: both receive a client's live protocol, socket and input buffer.
pub const FRAMING: Framing = unsafe { Framing::new(client_recv::<true>, client_recv::<false>) };

/// `static ssize_t Protocol::default_length_func(const Protocol *, Socket *, PacketLength *)`.
///
/// # Safety
/// A live protocol and socket, and a packet whose `buf` holds `buf_size` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZN6swoole8Protocol19default_length_funcEPKS0_PNS_7network6SocketEPNS_12PacketLengthE"
    )
)]
pub unsafe extern "C" fn default_length_func(
    p: *const Protocol,
    socket: *mut c_void,
    pl: &mut PacketLength,
) -> isize {
    // SAFETY: as the caller promises.
    let p = unsafe { &*p };
    let size = match p.get_package_length_size {
        // SAFETY: upstream's call over the live socket.
        Some(size) => unsafe { size(socket) },
        None => p.package_length_size,
    };
    let bytes = pl.bytes();
    match default_length(
        p.package_length_type,
        bytes,
        u32::from(p.package_length_offset),
        u32::from(size),
        u32::from(p.package_body_offset),
    ) {
        DefaultLength::Need(need) => {
            pl.header_len = u32::try_from(need).unwrap_or(u32::MAX);
            0
        }
        DefaultLength::Total(total) => {
            pl.header_len = u32::from(p.package_length_size);
            isize::try_from(total).unwrap_or(SW_ERR)
        }
        DefaultLength::Format => {
            invalid_format(p.package_length_type);
            SW_ERR
        }
        DefaultLength::Invalid => SW_ERR,
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

/// `int64_t swoole_unpack(char, const void *)` over `data`'s first bytes: -1 for too few
/// and -1 (logged) for an unknown format.
#[must_use]
pub fn swoole_unpack(format: u8, data: &[u8]) -> i64 {
    match unpack(format, data) {
        Ok(value) => value,
        Err(UnpackError::Format) => {
            invalid_format(format);
            -1
        }
        Err(UnpackError::Short) => -1,
    }
}

/// `uint64_t swoole_hton64(uint64_t)`.
#[must_use]
pub const fn swoole_hton64(value: u64) -> u64 {
    value.to_be()
}

/// `uint64_t swoole_ntoh64(uint64_t)`.
#[must_use]
pub const fn swoole_ntoh64(value: u64) -> u64 {
    value.to_be()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use swoole_rs_corebase::CCast;
    use swoole_rs_reactor::SocketRecord;

    thread_local! {
        static PACKAGES: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
    }

    unsafe extern "C-unwind" fn record(
        _: *const Protocol,
        _: *mut c_void,
        packet: *const c_void,
    ) -> c_int {
        let packet = packet.cast::<RecvData>();
        // SAFETY: the receive's packet, `len` bytes at `data`.
        let bytes =
            unsafe { core::slice::from_raw_parts((*packet).data, (*packet).info.len as usize) };
        PACKAGES.with_borrow_mut(|p| p.push(bytes.to_vec()));
        0
    }

    /// The handle a client's read passes for its live socket.
    fn handle(socket: &mut SocketRecord) -> Socket<'_> {
        unsafe { Socket::from_ptr(socket) }.expect("a socket")
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
        let mut protocol = Protocol::EMPTY;
        protocol.package_max_length = 1024;
        protocol.on_package = Some(record);
        let mut buffer = CppString::EMPTY;
        buffer.alloc(8, None);
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
        protocol.get_package_length = Some(default_length_func);
        let status = client_recv::<true>(&raw const protocol, handle(&mut socket), &raw mut buffer);
        assert_eq!(status, 0);
        let status = client_recv::<true>(&raw const protocol, handle(&mut socket), &raw mut buffer);
        assert_eq!(status, 0);
        PACKAGES.with_borrow(|p| {
            assert_eq!(
                *p,
                [b"\x00\x03abc".to_vec(), b"\x00\x0chello world!".to_vec()]
            );
        });
        unsafe { libc::close(peer) };
        // The peer's close ends the next receive.
        let status = client_recv::<true>(&raw const protocol, handle(&mut socket), &raw mut buffer);
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
        let status =
            client_recv::<false>(&raw const protocol, handle(&mut socket), &raw mut buffer);
        assert_eq!(status, 0);
        PACKAGES.with_borrow(|p| assert_eq!(*p, [b"ab\r\n".to_vec(), b"cd\r\n".to_vec()]));
        assert_eq!(buffer.len(), 1);
        buffer.dispose();
        unsafe { libc::close(peer) };
        unsafe { libc::close(socket.fd) };
    }

    // core-tests/src/protocol/base.cpp's protocol.swap_byte_order, protocol.unpack and
    // protocol.hton64, with the same inputs and expected values. upstream's
    // swoole_swap_endian{16,32,64} are the integers' `swap_bytes`.

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
    #[expect(clippy::cognitive_complexity, reason = "one upstream TEST, kept whole")]
    fn unpack_formats() {
        // 8-bit
        assert_eq!(swoole_unpack(b'c', &data(&(-42i8).to_ne_bytes())), -42);
        assert_eq!(swoole_unpack(b'C', &data(&200u8.to_ne_bytes())), 200);
        assert_eq!(
            swoole_unpack(b'c', &data(&i8::MIN.to_ne_bytes())),
            i8::MIN.into()
        );
        assert_eq!(
            swoole_unpack(b'c', &data(&i8::MAX.to_ne_bytes())),
            i8::MAX.into()
        );
        assert_eq!(
            swoole_unpack(b'C', &data(&u8::MAX.to_ne_bytes())),
            u8::MAX.into()
        );

        // 16-bit
        assert_eq!(
            swoole_unpack(b's', &data(&(-12345i16).to_ne_bytes())),
            -12345
        );
        assert_eq!(swoole_unpack(b'S', &data(&54321u16.to_ne_bytes())), 54321);
        let n_val = 0x1234u16;
        let n_be = n_val.rotate_left(8);
        assert_eq!(swoole_unpack(b'n', &data(&n_be.to_ne_bytes())), 0x1234);
        assert_eq!(swoole_unpack(b'v', &data(&0x1234u16.to_ne_bytes())), 0x1234);
        assert_eq!(
            swoole_unpack(b's', &data(&i16::MIN.to_ne_bytes())),
            i16::MIN.into()
        );
        assert_eq!(
            swoole_unpack(b's', &data(&i16::MAX.to_ne_bytes())),
            i16::MAX.into()
        );
        assert_eq!(
            swoole_unpack(b'S', &data(&u16::MAX.to_ne_bytes())),
            u16::MAX.into()
        );

        // 32-bit
        assert_eq!(
            swoole_unpack(b'l', &data(&(-123_456_789i32).to_ne_bytes())),
            -123_456_789
        );
        assert_eq!(
            swoole_unpack(b'L', &data(&3_000_000_000u32.to_ne_bytes())),
            3_000_000_000
        );
        let n_val = 0x1234_5678u32;
        let n_be = ((n_val & 0xFF) << 24)
            | ((n_val & 0xFF00) << 8)
            | ((n_val & 0x00FF_0000) >> 8)
            | ((n_val & 0xFF00_0000) >> 24);
        assert_eq!(swoole_unpack(b'N', &data(&n_be.to_ne_bytes())), 0x1234_5678);
        assert_eq!(
            swoole_unpack(b'V', &data(&0x1234_5678u32.to_ne_bytes())),
            0x1234_5678
        );
        assert_eq!(
            swoole_unpack(b'l', &data(&i32::MIN.to_ne_bytes())),
            i32::MIN.into()
        );
        assert_eq!(
            swoole_unpack(b'l', &data(&i32::MAX.to_ne_bytes())),
            i32::MAX.into()
        );
        assert_eq!(
            swoole_unpack(b'L', &data(&u32::MAX.to_ne_bytes())),
            u32::MAX.into()
        );

        // 64-bit
        assert_eq!(
            swoole_unpack(b'q', &data(&(-1_234_567_890_123_456_789i64).to_ne_bytes())),
            -1_234_567_890_123_456_789
        );
        assert_eq!(
            swoole_unpack(b'Q', &data(&10_234_567_890_123_456_789u64.to_ne_bytes())),
            10_234_567_890_123_456_789u64.c_cast::<i64>()
        );
        let j_be = 0x1234_5678_9ABC_DEF0u64.swap_bytes();
        assert_eq!(
            swoole_unpack(b'J', &data(&j_be.to_ne_bytes())),
            0x1234_5678_9ABC_DEF0
        );
        assert_eq!(
            swoole_unpack(b'P', &data(&0x1234_5678_9ABC_DEF0u64.to_ne_bytes())),
            0x1234_5678_9ABC_DEF0
        );
        assert_eq!(
            swoole_unpack(b'q', &data(&i64::MIN.to_ne_bytes())),
            i64::MIN
        );
        assert_eq!(
            swoole_unpack(b'q', &data(&i64::MAX.to_ne_bytes())),
            i64::MAX
        );
        assert_eq!(
            swoole_unpack(b'Q', &data(&u64::MAX.to_ne_bytes())),
            u64::MAX.c_cast::<i64>()
        );

        // Machine-dependent int
        assert_eq!(
            swoole_unpack(b'i', &data(&(-987_654_321i32).to_ne_bytes())),
            -987_654_321
        );
        assert_eq!(
            swoole_unpack(b'I', &data(&3_000_000_000u32.to_ne_bytes())),
            3_000_000_000
        );
        assert_eq!(
            swoole_unpack(b'i', &data(&i32::MIN.to_ne_bytes())),
            i32::MIN.into()
        );
        assert_eq!(
            swoole_unpack(b'i', &data(&i32::MAX.to_ne_bytes())),
            i32::MAX.into()
        );
        assert_eq!(
            swoole_unpack(b'I', &data(&u32::MAX.to_ne_bytes())),
            u32::MAX.into()
        );

        // Invalid format specifiers
        let zero = [0u8; 8];
        assert_eq!(swoole_unpack(b'x', &zero), -1);
        assert_eq!(swoole_unpack(b'?', &zero), -1);
        assert_eq!(swoole_unpack(b'Z', &zero), -1);

        // Endianness
        assert_eq!(swoole_unpack(b'n', &data(&[0x12, 0x34])), 0x1234);
        assert_eq!(swoole_unpack(b'v', &data(&[0x34, 0x12])), 0x1234);
        assert_eq!(
            swoole_unpack(b'N', &data(&[0x12, 0x34, 0x56, 0x78])),
            0x1234_5678
        );
        assert_eq!(
            swoole_unpack(b'V', &data(&[0x78, 0x56, 0x34, 0x12])),
            0x1234_5678
        );
        let be = [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0];
        assert_eq!(swoole_unpack(b'J', &be), 0x1234_5678_9ABC_DEF0);
        let le = [0xF0, 0xDE, 0xBC, 0x9A, 0x78, 0x56, 0x34, 0x12];
        assert_eq!(swoole_unpack(b'P', &le), 0x1234_5678_9ABC_DEF0);

        // htons, htonl and swoole_hton64 into n, N and J
        assert_eq!(
            swoole_unpack(b'n', &data(&0x1234u16.to_be().to_ne_bytes())),
            0x1234
        );
        assert_eq!(
            swoole_unpack(b'N', &data(&0x1234_5678u32.to_be().to_ne_bytes())),
            0x1234_5678
        );
        let be64 = swoole_hton64(0x1234_5678_9ABC_DEF0);
        assert_eq!(
            swoole_unpack(b'J', &data(&be64.to_ne_bytes())),
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
