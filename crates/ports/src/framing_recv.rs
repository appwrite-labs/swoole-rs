// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/protocol/base.cc (Swoole contributors).
//! `Protocol::recv_with_length_protocol`, `recv_with_eof_protocol` and
//! `recv_split_by_eof`: [`swoole_rs_framing::recv::recv`] with each effect on the socket,
//! the buffer and the protocol's callbacks; `default_length_func`; and `swoole_hton64`/`swoole_ntoh64`. Logs name the
//! upstream function that wrote them.
use core::ffi::{CStr, c_int};
use core::sync::atomic::Ordering;
use swoole_rs_clients::client::{ClientFraming, Framing};
use swoole_rs_clients::protocol::{Package, Protocol};
use swoole_rs_corebase::buffer::string::StringBuffer;
use swoole_rs_corebase::log::LogLevel::{SW_LOG_ERROR as LOG_ERROR, SW_LOG_WARNING as LOG_WARNING};
use swoole_rs_corebase::net_types::PacketLength;
use swoole_rs_corebase::reactor::{Caught, Socket, SocketFields, swr_v1_socket_reset_on_close};
use swoole_rs_corebase::{DataHead, log, swr_v1_core_pagesize};
use swoole_rs_error::ErrorCode;
use swoole_rs_framing::recv::{self, Buffer, Effects, Received, Status, recv};
use swoole_rs_framing::recv::{
    FLAG_DYNAMIC, FLAG_PAUSED, FLAG_REMOVED, FLAG_SKIP, FLAG_TLS, FLAG_WAIT, Mode, Op,
};
use swoole_rs_framing::{DefaultLength, default_length};
use swoole_rs_server::Shared;

const SW_ERR: isize = swoole_rs_corebase::constants::ResultCode::SW_ERR as isize;

/// One receive: the protocol and socket each effect acts on, live for the receive.
#[derive(Clone, Copy)]
pub(crate) struct Recv<'a> {
    mode: Mode,
    pub(crate) socket: Socket<'a>,
    /// Supplies the current protocol and its optional PHP length callable.
    owner: Owner<'a>,
}

/// The source of the receive's protocol and its length callable.
#[derive(Clone, Copy)]
pub(crate) enum Owner<'a> {
    /// Protocol settings without a PHP length callable.
    Plain(&'a Shared<Protocol>),
    /// The client the receive reads for.
    Client(&'a ClientFraming),
    /// The port the receive reads for, through this thread's registry.
    Port(&'a swoole_rs_server::port_config::PortConfig),
}

impl Owner<'_> {
    /// A reference to the owner, which the length callback drops. Inlined into each
    /// measure, so a port without one tests the variant and calls nothing.
    #[inline(always)]
    fn get(self) -> Option<std::rc::Rc<dyn core::any::Any>> {
        match self {
            Self::Plain(_) => None,
            found => found.find(),
        }
    }

    /// [`Self::get`] of a client or a port, which has a PHP length callable.
    #[cold]
    #[inline(never)]
    fn find(self) -> Option<std::rc::Rc<dyn core::any::Any>> {
        match self {
            Self::Plain(_) => None,
            Self::Client(client) => client.length_owner(),
            Self::Port(port) => port.length_owner(),
        }
    }
}

impl Recv<'_> {
    /// Read fresh settings after any callback that can reconfigure the socket.
    #[inline]
    fn protocol(&self) -> Protocol {
        match self.owner {
            Owner::Plain(protocol) => protocol.get(),
            Owner::Client(client) => client.protocol(),
            Owner::Port(port) => port.protocol.get(),
        }
    }

    fn protocol_address(&self) -> *const Protocol {
        match self.owner {
            Owner::Plain(protocol) => protocol.as_ptr(),
            Owner::Client(client) => client.protocol_address(),
            Owner::Port(port) => port.protocol.as_ptr(),
        }
    }
}

/// `package` as the payload a dispatch with the head `info` sends: none (a null one)
/// when the package is.
pub(crate) fn send_data<'a>(
    package: &'a mut Package<'_>,
    info: DataHead,
) -> swoole_rs_server::SendData<'a> {
    use swoole_rs_server::SendData;
    if package.as_ptr().is_null() {
        SendData::empty(info)
    } else {
        SendData::new(info, package.bytes())
    }
}

/// How a length receive measures and hands on each package.
pub(crate) trait Packages {
    /// The fixed length header size, or `None` to ask the protocol's callbacks.
    const HEAD: Option<u32>;
    fn dynamic_header(&self, _: &Protocol) -> Option<u8> {
        None
    }
    /// `get_package_length` over the buffered `bytes`: `None` for malformed bytes,
    /// `Some(0)` while the header is incomplete. The protocol's, `None` without one.
    #[inline(always)]
    fn length(&self, r: &Recv<'_>, bytes: &[u8]) -> Option<usize> {
        let mut pl = PacketLength::new(bytes);
        // A copy: the callback may run PHP that rewrites the protocol.
        let protocol = r.protocol();
        let get = protocol.get_package_length?;
        let owner = r.owner.get();
        let length = get(&protocol, self.dynamic_header(&protocol), owner, &mut pl);
        usize::try_from(length).ok()
    }
    /// `onPackage` over a whole package; whether it succeeded. No reference to the
    /// protocol is held across it: the callback may run PHP that rewrites it.
    fn dispatch(&self, r: &Recv<'_>, package: Package<'_>) -> bool;
    /// Whether the socket, which is out of the reactor, was paused rather than closed.
    fn paused(&self) -> bool;
}

/// The protocol's `onPackage` over `package`; false without one.
pub(crate) fn protocol_dispatch(r: &Recv<'_>, package: Package<'_>) -> bool {
    // Read before the call, which may rewrite the protocol.
    let Some(on_package) = r.protocol().on_package else {
        return false;
    };
    on_package(r.protocol_address(), r.socket, package) >= 0
}

/// The protocol's own callbacks, as upstream calls them, over a client's socket: never
/// a server session's, so never a paused one.
pub(crate) struct Callbacks;

impl Packages for Callbacks {
    const HEAD: Option<u32> = None;
    #[inline(always)]
    fn dispatch(&self, r: &Recv<'_>, package: Package<'_>) -> bool {
        protocol_dispatch(r, package)
    }
    fn paused(&self) -> bool {
        false
    }
}

/// A [`Recv`] and its buffer, whose packages `K` measures and hands on, and the
/// `recv_wait` the socket holds: only this receive writes it, so a flag that didn't
/// change costs no edit.
struct With<'a, K>(Recv<'a>, bool, K, &'a mut StringBuffer);

impl<K: Packages> With<'_, K> {
    /// `socket->recv_wait = flags & FLAG_WAIT`, when it differs.
    #[inline(always)]
    fn apply_wait(&mut self, flags: u32) {
        let wait = flags & FLAG_WAIT != 0;
        if wait != self.1 {
            self.1 = wait;
            self.0.socket.edit(|s| s.recv_wait = wait);
        }
    }

    /// `flags` from the socket, after an effect that may have removed, paused or
    /// closed it.
    #[inline(always)]
    fn refresh(&mut self, flags: &mut u32) {
        *flags = self.0.socket.read(|s| socket_flags(s, || self.2.paused()));
        self.1 = *flags & FLAG_WAIT != 0;
    }

    /// Runs `f` between applying `flags`' wait and refreshing them.
    #[inline(always)]
    fn around<T>(
        &mut self,
        flags: &mut u32,
        f: impl FnOnce(Recv<'_>, &mut StringBuffer) -> T,
    ) -> T {
        self.apply_wait(*flags);
        let result = f(self.0, self.3);
        self.refresh(flags);
        result
    }
}

pub(crate) fn socket_flags(socket: &SocketFields, paused: impl FnOnce() -> bool) -> u32 {
    (if socket.removed { FLAG_REMOVED } else { 0 })
        | (if socket.recv_wait { FLAG_WAIT } else { 0 })
        | (if socket.ssl().is_null() { 0 } else { FLAG_TLS })
        | (if socket.removed && paused() {
            FLAG_PAUSED
        } else {
            0
        })
}

/// A session out of the reactor whose connection no one is closing: `pause` took its
/// socket out to stop reading.
pub(crate) fn paused(conn: swoole_rs_server::Connection<'_>) -> bool {
    conn.is_active()
        && conn.closed().load(Ordering::Relaxed) == 0
        && !(conn.closing()
            || conn.close_actively()
            || conn.close_queued()
            || conn.close_reset()
            || conn.close_force()
            || conn.peer_closed())
}

/// `proto_error`: the warning, with the peer's address (or the socket's own when
/// `getpeername` fails) appended as `<host:port>`.
fn proto_error(socket: Socket<'_>, code: ErrorCode, origin: &CStr, message: &str) {
    let address = socket.read(|socket| {
        let mut address = socket.info;
        if !socket.get_peer_name(&mut address) {
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
    fn effect(&mut self, op: Op, arg: usize, flags: &mut u32) {
        self.around(flags, |r, b| rare(r, b, op, arg));
    }
    /// A read leaves the socket as it was; only a failed one refreshes `flags`.
    #[inline(always)]
    fn recv(&mut self, size: usize, flags: &mut u32) -> Received {
        self.apply_wait(*flags);
        let socket = self.0.socket;
        // The loop asks for at most the buffer's free tail.
        let read = self.3.fill_within(size, |tail| {
            swoole_rs_corebase::tls::upstream::recv_spare(socket, tail, 0)
        });
        if let Some(n) = read {
            return Received::Bytes(n);
        }
        let received = failed_recv(&self.0, size);
        self.refresh(flags);
        received
    }
    /// A length leaves the socket as it was; only a failed one refreshes `flags`.
    #[inline(always)]
    fn length(&mut self, from: usize, flags: &mut u32) -> Option<usize> {
        self.apply_wait(*flags);
        let length = self
            .2
            .length(&self.0, self.3.view().get(from..).unwrap_or_default());
        if length.is_none() {
            self.refresh(flags);
        }
        length
    }
    #[inline(always)]
    fn dispatch(&mut self, from: usize, flags: &mut u32) -> bool {
        self.apply_wait(*flags);
        let b = &mut *self.3;
        let length = usize::try_from(b.offset().unsigned_abs()).unwrap_or(usize::MAX);
        let handled = self.2.dispatch(&self.0, Package::of(b, from, length));
        self.refresh(flags);
        handled
    }
    fn split(&mut self, flags: &mut u32) -> Option<usize> {
        self.around(flags, split)
    }
    fn ssl_pending(&mut self, flags: &mut u32) -> bool {
        self.around(flags, |r, _| {
            swoole_rs_corebase::tls::Ssl::of(r.socket).is_some_and(|ssl| ssl.get().pending() > 0)
        })
    }
    fn page(&mut self, flags: &mut u32) -> usize {
        self.around(flags, |_, _| swr_v1_core_pagesize() as usize)
    }
    #[inline(always)]
    fn buffer(&mut self) -> Buffer {
        Buffer {
            length: self.3.len(),
            size: self.3.size(),
            offset: self.3.offset(),
        }
    }
    #[inline(always)]
    fn set_buffer(&mut self, length: usize, offset: i64) {
        self.3.truncate(length);
        self.3.set_offset(offset);
    }
    #[inline(always)]
    fn data(&mut self) -> &[u8] {
        self.3.view()
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

/// The function a log names.
fn origin(r: &Recv<'_>) -> &'static CStr {
    if r.mode == Mode::Length {
        c"Protocol::recv_with_length_protocol"
    } else {
        c"Protocol::recv_with_eof_protocol"
    }
}

/// A failed read of at most `size` bytes: `catch_read_error` decides.
#[cold]
#[inline(never)]
fn failed_recv(r: &Recv<'_>, size: usize) -> Received {
    let s = r.socket;
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    match s.catch_error(errno) {
        Caught::Error => {
            let origin = origin(r);
            if r.mode == Mode::Length {
                log::sys_warning_errno(
                    errno,
                    origin,
                    format_args!("recv({}, {size}) failed", s.fd()),
                );
            } else {
                log::sys_warning_errno(
                    errno,
                    origin,
                    format_args!("recv from socket#{} failed", s.fd()),
                );
            }
            Received::Done
        }
        Caught::Close => Received::Close,
        Caught::Wait | Caught::Invalid | Caught::ReduceSize => Received::Done,
    }
}

/// `buffer->split` on the EOF, each part to `onPackage` while the socket stays.
#[cold]
#[inline(never)]
fn split(r: Recv<'_>, b: &mut StringBuffer) -> Option<usize> {
    let s = r.socket;
    // A copy: `onPackage` may run PHP whose `set()` rewrites the protocol.
    let (eof, eof_len) = {
        let p = r.protocol();
        (p.package_eof, p.package_eof_len)
    };
    // The protocol's settings bound the length by the array.
    let eof = eof.get(..usize::from(eof_len))?;
    b.split_with(eof, |b, start, length| {
        protocol_dispatch(&r, Package::of(b, start, length)) && !s.read(|s| s.removed)
    })
}

/// The effects a frame rarely needs.
#[cold]
#[inline(never)]
fn rare(r: Recv<'_>, b: &mut StringBuffer, op: Op, arg: usize) {
    let s = r.socket;
    let length_mode = r.mode == Mode::Length;
    let origin = origin(&r);
    match op {
        Op::Extend => b.extend(arg),
        Op::Reduce => b.reduce(i64::try_from(arg).unwrap_or(i64::MAX)),
        Op::ClearSkip => s.edit(|s| s.skip_recv = false),
        Op::LogMalformed => {
            let message = format!("received {} bytes of malformed data, remote_addr=", b.len());
            proto_error(
                s,
                ErrorCode::SW_ERROR_PACKAGE_MALFORMED_DATA,
                origin,
                &message,
            );
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
        }
        Op::LogTooLarge => {
            let message = if length_mode {
                format!("the received packet length {arg} is too large, remote_addr=")
            } else {
                format!("The received data packet is too large, length={}", b.len())
            };
            proto_error(
                s,
                ErrorCode::SW_ERROR_PACKAGE_LENGTH_TOO_LARGE,
                origin,
                &message,
            );
        }
        Op::Reset => _ = swr_v1_socket_reset_on_close(s.fd()),
    }
}

/// One receive of `mode` from `socket` into `buffer`.
#[inline(always)]
pub(crate) fn receive<'a, K: Packages>(
    socket: Socket<'a>,
    owner: Owner<'a>,
    buffer: &'a mut StringBuffer,
    mode: Mode,
    packages: K,
    initial: Option<(u32, bool)>,
) -> Status {
    let mut r = With::<K>(
        Recv {
            mode,
            socket,
            owner,
        },
        false,
        packages,
        buffer,
    );
    let (mut flags, skip) =
        initial.unwrap_or_else(|| socket.read(|s| (socket_flags(s, || r.2.paused()), s.skip_recv)));
    r.1 = flags & FLAG_WAIT != 0;
    let mut head = 0;
    if let Some(fixed) = K::HEAD {
        head = fixed;
        flags |= if skip { FLAG_SKIP } else { 0 };
    } else if mode == Mode::Length {
        let p = r.0.protocol();
        let size = p.get_package_length_size;
        head = u32::from(r.2.dynamic_header(&p).unwrap_or(p.package_length_size));
        flags |=
            (if skip { FLAG_SKIP } else { 0 }) | (if size.is_some() { FLAG_DYNAMIC } else { 0 });
    }
    let status = recv(mode, head, &mut flags, &mut r);
    r.apply_wait(flags);
    status
}

/// `int Protocol::recv_with_length_protocol(Socket *, String *) const` when `LENGTH`, else
/// `int Protocol::recv_with_eof_protocol(Socket *, String *) const`, over `client`'s.
fn client_recv<const LENGTH: bool>(
    client: &ClientFraming,
    socket: Socket<'_>,
    buffer: &mut StringBuffer,
) -> c_int {
    let mode = if LENGTH { Mode::Length } else { Mode::Eof };
    receive(socket, Owner::Client(client), buffer, mode, Callbacks, None) as c_int
}

/// The framed receives a client's reads use, which `swoole_init` registers
/// (`swoole_rs_clients::client::set_framing`).
pub const FRAMING: Framing = Framing::new(client_recv::<true>, client_recv::<false>);

/// `static ssize_t Protocol::default_length_func(const Protocol *, Socket *, PacketLength *)`.
pub fn default_length_func(
    p: &Protocol,
    header: Option<u8>,
    _owner: Option<std::rc::Rc<dyn core::any::Any>>,
    pl: &mut PacketLength,
) -> isize {
    let size = header.unwrap_or(p.package_length_size);
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
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use swoole_rs_corebase::CCast;
    use swoole_rs_corebase::reactor::SocketRecord;

    /// Upstream's `int64_t swoole_unpack(char, const void *)`: -1 for too few bytes or
    /// an unknown format.
    fn swoole_unpack(format: u8, data: &[u8]) -> i64 {
        swoole_rs_framing::unpack(format, data).unwrap_or(-1)
    }

    thread_local! {
        static PACKAGES: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
    }

    fn record(_: *const Protocol, _: Socket<'_>, mut packet: Package<'_>) -> c_int {
        PACKAGES.with_borrow_mut(|p| p.push(packet.bytes().to_vec()));
        0
    }

    /// The framed receive of `LENGTH` a client's read makes, over these records.
    fn client_recv<const LENGTH: bool>(
        protocol: &Shared<Protocol>,
        socket: Socket<'_>,
        buffer: &mut StringBuffer,
    ) -> c_int {
        let mode = if LENGTH { Mode::Length } else { Mode::Eof };
        receive(
            socket,
            Owner::Plain(protocol),
            buffer,
            mode,
            Callbacks,
            None,
        ) as c_int
    }

    /// A connected socket with `sent` waiting, the protocol and an 8-byte buffer.
    fn stream(sent: &[u8]) -> (SocketRecord, UnixStream, UnixStream, Protocol, StringBuffer) {
        let (mut peer, reader) = UnixStream::pair().unwrap();
        assert_eq!(peer.write(sent).unwrap(), sent.len());
        reader.set_nonblocking(true).unwrap();
        let mut socket = SocketRecord::new();
        socket.fd = reader.as_raw_fd();
        socket.socket_type = 5; // SW_SOCK_UNIX_STREAM
        let mut protocol = Protocol::EMPTY;
        protocol.package_max_length = 1024;
        protocol.on_package = Some(record);
        let mut buffer = StringBuffer::EMPTY;
        buffer.alloc(8, None);
        PACKAGES.with_borrow_mut(Vec::clear);
        (socket, peer, reader, protocol, buffer)
    }

    #[test]
    fn framing_reads_updated_settings_at_the_same_protocol_address() {
        let protocol = Shared::new(Protocol::EMPTY);
        let mut socket = SocketRecord::new();
        let recv = Recv {
            mode: Mode::Length,
            socket: Socket::new(&mut socket),
            owner: Owner::Plain(&protocol),
        };
        let original = recv.protocol_address();
        protocol.update(|p| p.package_length_size = 8);
        assert_eq!(recv.protocol().package_length_size, 8);
        assert_eq!(recv.protocol_address(), original);
        assert_eq!(original, protocol.as_ptr().cast_const());
    }

    #[test]
    fn length_protocol_with_the_default_length() {
        let (mut socket, peer, reader, mut protocol, mut buffer) =
            stream(b"\x00\x03abc\x00\x0chello world!");
        protocol.package_length_type = b'n';
        protocol.package_length_size = 2;
        protocol.package_body_offset = 2;
        protocol.get_package_length = Some(default_length_func);
        let protocol = Shared::new(protocol);
        let status = client_recv::<true>(&protocol, Socket::new(&mut socket), &mut buffer);
        assert_eq!(status, 0);
        let status = client_recv::<true>(&protocol, Socket::new(&mut socket), &mut buffer);
        assert_eq!(status, 0);
        PACKAGES.with_borrow(|p| {
            assert_eq!(
                *p,
                [b"\x00\x03abc".to_vec(), b"\x00\x0chello world!".to_vec()]
            );
        });
        peer.shutdown(std::net::Shutdown::Write).unwrap();
        // The shutdown ends the next receive even if a spawning child inherited the fd.
        let status = client_recv::<true>(&protocol, Socket::new(&mut socket), &mut buffer);
        assert_eq!(status, -1);
        buffer.dispose();
        drop(reader);
    }

    #[test]
    fn eof_protocol_splits_and_keeps_the_rest() {
        let (mut socket, peer, reader, mut protocol, mut buffer) = stream(b"ab\r\ncd\r\ne");
        protocol.package_eof[..2].copy_from_slice(b"\r\n");
        protocol.package_eof_len = 2;
        protocol.split_by_eof = true;
        let protocol = Shared::new(protocol);
        buffer.extend(64);
        let status = client_recv::<false>(&protocol, Socket::new(&mut socket), &mut buffer);
        assert_eq!(status, 0);
        PACKAGES.with_borrow(|p| assert_eq!(*p, [b"ab\r\n".to_vec(), b"cd\r\n".to_vec()]));
        assert_eq!(buffer.len(), 1);
        buffer.dispose();
        drop(peer);
        drop(reader);
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
