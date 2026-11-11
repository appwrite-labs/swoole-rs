//! The `coroutine::Socket` members of `src/coroutine/socket.cc` that are whole Rust
//! operations, as methods of the socket's handle ([`Co`]): the reactor callbacks, the
//! local and peer names, the timeouts and options, the buffered and packet reads,
//! the read and write buffers, `bind` of an address, `listen` and `accept`, and the
//! transfers (`recv`, `send`, their `_all` and `_once` forms, `read`, `write`,
//! `readv`, `writev` and their `_all` forms, `recvmsg`, `sendmsg`, `recvfrom`),
//! `peek` and `poll`. Only the read and write buffers keep their upstream symbols,
//! which crates/phpclients and crates/phphttp link by name.
use crate::buffered::{recv_buffered, recv_line};
use crate::co_io::{
    NATIVE, Timer, available, borrowed_err, check, default_err, errno, owned_err, set_err,
};
use crate::co_socket::{Co, peer_name};
use crate::sendfile::{FAILED, ZERO, send_range};
use crate::setup::{Setup, swr_v1_hook_accept};
use crate::shutdown::READ;
use crate::socket_wait::{error_event, ready, wait_for};
use crate::transfer::{ALL, CHECKED, RETRY_INTERRUPTED, timeout_kind, transfer};
#[cfg(feature = "iouring")]
use crate::uring_socket as ring;
use crate::vector::vector_all;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::ptr::null_mut;
use swoole_rs_corebase::constants::SW_STACK_BUFFER_SIZE;
use swoole_rs_corebase::log;
use swoole_rs_reactor::{
    CLASS_DGRAM, Event, ReactorIov, Running, Socket, SocketAddress, SocketFields, SocketRecord,
    swr_v1_reactor_iov_view, type_is,
};
use swoole_rs_tls::owner_fields;
use swoole_rs_tls::upstream::{
    sendfile as socket_sendfile_fd, socket_peek, socket_readv, socket_recv, socket_send,
    socket_ssl_get_peer_certificate_pem, socket_writev,
};

/// `SW_EVENT_WRITE`.
const WRITE: u32 = 1 << 10;
/// `sizeof(int)`.
const INT_LENGTH: libc::socklen_t = c_int::BITS / 8;

/// With `io_uring`, a `UringSocket`'s member takes the ring's path
/// (`crate::uring_socket`, as `ring`) and returns its result.
macro_rules! uring {
    ($socket:expr, |$ops:ident| $body:expr) => {
        #[cfg(feature = "iouring")]
        if let Some($ops) = crate::uring_socket::of($socket) {
            let $ops = &$ops;
            // SAFETY: the socket's live view, with the member's arguments.
            return unsafe { $body };
        }
    };
}

/// The owner of the live `network::IOVector` at `vector`.
///
/// # Safety
/// `vector` is a live `IOVector`, whose first member is its owner.
#[cfg(feature = "iouring")]
unsafe fn iov_owner(vector: *mut c_void) -> crate::uring_socket::IoVector {
    // SAFETY: as the caller promises.
    crate::uring_socket::IoVector(unsafe { *vector.cast::<*mut ReactorIov>() })
}

/// The coroutine socket a reactor event came from, and its `network::Socket`.
fn event_socket(event: &Event) -> Option<(Co, Socket<'_>)> {
    let source = event.socket()?;
    let object = source.read(|record| record.object);
    // SAFETY: a coroutine socket's network::Socket's object is that socket.
    Some((unsafe { Co::of(object) }, source))
}

/// `Socket::readable_event_callback` and `writable_event_callback`: Rust picks the
/// coroutine to wake, a TLS wait's other direction, or none while a barrier waits.
fn on_ready(event: &Event, kind: u32) -> c_int {
    let Some((socket, source)) = event_socket(event) else {
        return 0;
    };
    set_err(socket, 0);
    let hup = source.read(|record| record.event_hup) != 0;
    let co = ready(socket, kind, hup);
    if !co.is_null() {
        (NATIVE.resume)(co);
    }
    0
}

/// `Socket::readable_event_callback(reactor, event)`, registered for
/// `SW_FD_CO_SOCKET` by `swr_v1_hook_init_reactor`.
pub extern "C" fn readable_event_callback(_: Running<'_>, event: &mut Event) -> c_int {
    on_ready(event, READ)
}

/// `Socket::writable_event_callback(reactor, event)`.
pub extern "C" fn writable_event_callback(_: Running<'_>, event: &mut Event) -> c_int {
    on_ready(event, WRITE)
}

/// `Socket::error_event_callback(reactor, event)`: wakes the writer, then the
/// reader unless the writer closed the socket; the `network::Socket` outlives the wake-ups.
pub extern "C" fn error_event_callback(_: Running<'_>, event: &mut Event) -> c_int {
    if let Some((socket, source)) = event_socket(event) {
        error_event(socket, source);
    }
    0
}

/// What `accept`'s setup steps act on: the listening socket, its wait's timeout and
/// the connection accepted.
struct Accepting {
    socket: Co,
    timeout: f64,
    conn: *mut SocketRecord,
}

impl Setup for Accepting {
    fn fail(&mut self, error: c_int) {
        if error < 0 {
            default_err(self.socket, log::last_error());
        } else {
            set_err(self.socket, error);
        }
    }

    fn wait(&mut self, event: c_int) -> bool {
        let mut timer = Timer::new(self.socket, READ, self.timeout);
        let ready = timer.start() && wait_for(self.socket, event.cast_unsigned());
        timer.stop();
        ready
    }

    fn call(&mut self, _: c_int) -> c_int {
        self.conn = self
            .socket
            .net(SocketFields::accept)
            .unwrap_or(core::ptr::null_mut());
        if self.conn.is_null() { -1 } else { 0 }
    }
}

/// A transfer's call over the `network::Socket` and the buffer it is given.
macro_rules! call {
    (|$s:tt, $at:tt, $len:tt| $body:expr) => {
        |$s: *mut SocketRecord, $at: *mut u8, $len: usize| {
            // SAFETY: the transfer's live network::Socket, over its buffer.
            unsafe { $body }
        }
    };
}

/// `SW_ERROR_BAD_HOST_ADDR` and `SW_ERROR_SSL_EMPTY_PEER_CERTIFICATE`.
const BAD_HOST_ADDR: c_int = 722;
const SSL_EMPTY_PEER_CERTIFICATE: c_int = 1013;

/// An all-zero address, as `Address` starts: a stale scope id would misroute an
/// interface-local IPv6 destination.
pub(crate) fn empty_address() -> SocketAddress {
    // SAFETY: the address is plain data: integers and the sockaddr union.
    unsafe { core::mem::zeroed() }
}

/// `Address::assign(type, std::string(host), port, false)` into `address`; false
/// with the last error set.
pub(crate) fn assign(address: &mut SocketAddress, kind: c_int, host: &[u8], port: c_int) -> bool {
    address.kind = kind;
    let terminated = [host, &[0]].concat();
    // SAFETY: the address's own union and length, and the host's bytes and NUL.
    let error = unsafe {
        swoole_rs_clients::address::swr_v1_clients_address_assign(
            (&raw mut address.addr).cast(),
            &raw mut address.len,
            kind,
            terminated.as_ptr().cast(),
            host.len(),
            port,
            None,
        )
    };
    if error != 0 {
        log::set_last_error(error);
    }
    error == 0
}

/// Appends the `length` bytes at `data` to the `Vec<u8>` at `out`.
pub(crate) unsafe extern "C" fn put_host(out: *mut c_void, data: *const c_char, length: usize) {
    // SAFETY: `data` holds `length` bytes.
    let data = unsafe { core::slice::from_raw_parts(data.cast::<u8>(), length) };
    // SAFETY: `out` is the caller's Vec, live for the call.
    unsafe { &mut *out.cast::<Vec<u8>>() }.extend_from_slice(data);
}

/// The remaining count of the live `IOVector` at `vector`.
///
/// # Safety
/// `vector` is a live `IOVector`, whose first member is its owner.
unsafe fn remaining(vector: *const c_void) -> usize {
    // SAFETY: as the caller promises.
    let owner = unsafe { *vector.cast::<*const ReactorIov>() };
    // SAFETY: the vector's live owner.
    let count = swr_v1_reactor_iov_view(unsafe { &*owner }).remain_count;
    usize::try_from(count).unwrap_or(0)
}

use swoole_rs_corebase::sys::strerror;

/// A descriptor `Socket::sendfile` opened, closed when it goes.
struct OpenFile(c_int);

impl Drop for OpenFile {
    fn drop(&mut self) {
        if self.0 != -1 {
            // SAFETY: the descriptor this owns.
            unsafe { libc::close(self.0) };
        }
    }
}

/// `Socket::get_read_buffer()`: the read buffer, made on first use.
///
/// # Safety
/// `this` is a live native socket.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket15get_read_bufferEv")
)]
pub unsafe extern "C" fn get_read_buffer(this: *mut c_void) -> *mut c_void {
    // SAFETY: as the caller promises.
    unsafe { Co::of(this) }.buffer(false).cast()
}

/// `Socket::get_write_buffer()`: the write buffer, made on first use.
///
/// # Safety
/// `this` is a live native socket.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket16get_write_bufferEv")
)]
pub unsafe extern "C" fn get_write_buffer(this: *mut c_void) -> *mut c_void {
    // SAFETY: as the caller promises.
    unsafe { Co::of(this) }.buffer(true).cast()
}

impl Co {
    /// `Socket::getsockname()`, into the open socket's `network::Socket`.
    #[must_use]
    pub fn getsockname(self) -> bool {
        self.net(|record| record.get_name() == 0) == Some(true)
    }

    /// `Socket::getpeername(sa)`.
    pub fn getpeername(self, address: &mut SocketAddress) -> bool {
        let error = peer_name(self, address);
        if error != 0 {
            set_err(self, error);
        }
        error == 0
    }

    /// `Socket::recv_with_buffer(buf, n)`.
    ///
    /// # Safety
    /// `out` holds `size` writable bytes, outside the socket's read buffer.
    pub unsafe fn recv_with_buffer(self, out: *mut u8, size: usize) -> isize {
        // SAFETY: as the caller promises.
        recv_buffered(self, unsafe { core::slice::from_raw_parts_mut(out, size) })
    }

    /// `Socket::recv_line(buf, maxlen)`, one buffered byte at a time.
    ///
    /// # Safety
    /// `out` holds `limit` bytes and one more, outside the socket's read buffer.
    pub unsafe fn recv_line(self, out: *mut u8, limit: usize) -> isize {
        // SAFETY: as the caller promises.
        let out = unsafe { core::slice::from_raw_parts_mut(out, limit + 1) };
        recv_line(out, |at| recv_buffered(self, at))
    }

    /// `Socket::set_option(level, optname, optval, optlen)`: `setsockopt`, warning on
    /// failure.
    #[must_use]
    pub fn set_option(self, level: c_int, name: c_int, value: &[u8]) -> bool {
        let fd = self.net(|record| record.fd).unwrap_or(-1);
        let length = libc::socklen_t::try_from(value.len()).unwrap_or(libc::socklen_t::MAX);
        // SAFETY: the option's bytes.
        if unsafe { libc::setsockopt(fd, level, name, value.as_ptr().cast(), length) } < 0 {
            log::sys_warning(
                c"Socket::set_option",
                format_args!(
                    "setsockopt({}, {level}, {name}, {length}) failed",
                    self.sock_fd()
                ),
            );
            return false;
        }
        true
    }

    /// `Socket::set_option(level, optname, optval)` of an `int`.
    #[must_use]
    pub fn set_int_option(self, level: c_int, name: c_int, value: c_int) -> bool {
        self.set_option(level, name, &value.to_ne_bytes())
    }

    /// `Socket::get_option(level, optname, optval)` into an `int`: `getsockopt`,
    /// warning on failure.
    pub fn get_int_option(self, level: c_int, name: c_int, value: &mut c_int) -> bool {
        let fd = self.net(|record| record.fd).unwrap_or(-1);
        let mut length = INT_LENGTH;
        // SAFETY: an int and its length.
        if unsafe { libc::getsockopt(fd, level, name, (&raw mut *value).cast(), &raw mut length) }
            < 0
        {
            log::sys_warning(
                c"Socket::get_option",
                format_args!("getsockopt({}, {level}, {name}) failed", self.sock_fd()),
            );
            return false;
        }
        true
    }

    /// `Socket::accept(timeout)`: a connection as a new native socket (the accepting
    /// constructor, which shares the TLS context), or `None`. A zero timeout waits
    /// for the socket's read timeout; a TLS context is made before the first accept.
    /// In a coroutine.
    #[must_use]
    pub fn accept(self, timeout: f64) -> Option<Self> {
        if !available(self, READ) {
            return None;
        }
        let owner = self.tls_owner();
        // SAFETY: the live owner of the socket's context.
        let needs_context = !owner.is_null() && unsafe { owner_fields(owner) }.context.is_null();
        if needs_context && !self.ssl_context_create() {
            return None;
        }
        let client = self.accepted(timeout)?;
        if client.sock_fd() < 0 {
            let error = errno();
            log::sys_warning(c"Socket::accept", format_args!("new Socket() failed"));
            set_err(self, error);
            // SAFETY: the new native socket, used no more.
            unsafe { (crate::hooked::SOCKETS.free)(client.this()) };
            return None;
        }
        Some(client)
    }

    /// `accept`'s connection as a new native socket. The socket is available.
    fn accepted(self, timeout: f64) -> Option<Self> {
        #[cfg(feature = "iouring")]
        if let Some(ops) = crate::uring_socket::of(self) {
            return ring::accept(self, &ops);
        }
        let timeout = if timeout == 0.0 {
            self.read_timeout()
        } else {
            timeout
        };
        let mut state = Accepting {
            socket: self,
            timeout,
            conn: null_mut(),
        };
        if !swr_v1_hook_accept(&mut state) {
            return None;
        }
        // SAFETY: the connection the listening socket accepted.
        Some(unsafe { self.adopt(state.conn) })
    }

    /// `new Socket(conn, server)`: the accepting constructor, then the server's TLS
    /// context shared with the new socket.
    ///
    /// # Safety
    /// `conn` is a live accepted `network::Socket` the new socket takes.
    pub(crate) unsafe fn adopt(self, conn: *mut SocketRecord) -> Self {
        // SAFETY: as the caller promises.
        let client = unsafe { crate::co_socket::accepted(self, conn) };
        client.share_tls(self);
        client
    }

    /// `Socket::listen(backlog)`: `SW_BACKLOG` for a non-positive backlog; a listening
    /// socket's TLS is the server side.
    #[must_use]
    pub fn listen(self, backlog: c_int) -> bool {
        /// `SW_BACKLOG`.
        const BACKLOG: c_int = 512;
        // `SW_EVENT_NULL` skips the bound check.
        if !available(self, 0) {
            return false;
        }
        let backlog = if backlog <= 0 { BACKLOG } else { backlog };
        self.set_backlog(backlog);
        if self.net(|record| record.listen(backlog)).unwrap_or(-1) < 0 {
            set_err(self, errno());
            return false;
        }
        self.set_ssl_is_server(true);
        true
    }

    /// `Socket::transfer(event, buffer, length, flags, call)`: Rust's retry loop
    /// over `call`, which gets the `network::Socket` at each try.
    ///
    /// # Safety
    /// `buffer` holds `length` bytes or is null; `call` transfers on the
    /// `network::Socket` it is given.
    unsafe fn transfer<F: FnMut(*mut SocketRecord, *mut u8, usize) -> isize>(
        self,
        event: u32,
        flags: u32,
        buffer: *mut u8,
        length: usize,
        call: F,
    ) -> isize {
        let mut call = call;
        let mut each = |at, length| call(self.socket(), at, length);
        // SAFETY: as the caller promises.
        unsafe { transfer(self, event, flags, buffer, length, &mut each) }
    }

    /// [`Co::transfer`] reading into the caller's buffer.
    fn transfer_in<F: FnMut(*mut SocketRecord, *mut u8, usize) -> isize>(
        self,
        flags: u32,
        buf: &mut [impl swoole_rs_corebase::sys::ReadSlot],
        call: F,
    ) -> isize {
        let (at, n) = (buf.as_mut_ptr().cast::<u8>(), buf.len());
        // SAFETY: the caller's `n` one-byte slots, any value valid, which `call`
        // receives into.
        unsafe { self.transfer(READ, flags, at, n, call) }
    }

    /// [`Co::transfer`] writing the caller's bytes, which a write only reads (its
    /// wait may move them to the write buffer, a copy).
    fn transfer_out<F: FnMut(*mut SocketRecord, *mut u8, usize) -> isize>(
        self,
        flags: u32,
        buf: &[u8],
        call: F,
    ) -> isize {
        // SAFETY: the caller's bytes, of their length, only read.
        unsafe { self.transfer(WRITE, flags, buf.as_ptr().cast_mut(), buf.len(), call) }
    }

    /// `Socket::bind(host, port)`: `SO_REUSEADDR` (a failure is only logged), bind to
    /// the host's literal address, then record the local name; false with the error
    /// set.
    #[must_use]
    pub fn bind(self, host: &[u8], port: c_int) -> bool {
        if !available(self, 0) {
            return false;
        }
        log::set_last_error(0);
        let Some((fd, kind)) = self.net(|network| (network.fd, network.socket_type)) else {
            return false;
        };
        let enable: c_int = 1;
        // SAFETY: setsockopt(2) of an int on the socket's descriptor.
        let reuse = unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_REUSEADDR,
                (&raw const enable).cast(),
                INT_LENGTH,
            )
        };
        if reuse < 0 {
            log::sys_warning(
                c"bind",
                format_args!("setsockopt({}, SO_REUSEADDR) failed", self.sock_fd()),
            );
        }
        let mut address = empty_address();
        let bound = assign(&mut address, kind, host, port)
            && self.net(|record| record.bind(&address) >= 0 && record.get_name() >= 0)
                == Some(true);
        if !bound {
            // The last error carries it too, as the Socket calls that set_err(int) do, so the
            // hooked socket_create_listen() leaves it for socket_last_error().
            let error = match log::last_error() {
                0 => errno(),
                error => error,
            };
            log::set_last_error(error);
            default_err(self, error);
        }
        bound
    }

    /// `Socket::sendto(host, port, buf, n)`: an available datagram socket sends to
    /// the host's address, resolving its name when it is not an address; -1 with the
    /// error set. In a coroutine.
    #[must_use]
    pub fn sendto(self, host: &[u8], port: c_int, buf: &[u8]) -> isize {
        if !available(self, WRITE) {
            return -1;
        }
        let (kind, domain) = (self.kind(), self.sock_domain());
        let (socket_type, timeout) = self
            .net(|network| (network.socket_type, network.timeouts.dns))
            .unwrap_or_default();
        if !type_is(socket_type, CLASS_DGRAM) {
            set_err(self, libc::EPROTONOSUPPORT);
            return -1;
        }
        let mut address = empty_address();
        let mut assigned = assign(&mut address, kind, host, port);
        if !assigned && log::last_error() == BAD_HOST_ADDR {
            let ip = crate::resolve::system_gethostbyname(host, domain, timeout);
            if !ip.is_empty() {
                assigned = assign(&mut address, kind, &ip, port);
            }
        }
        if !assigned {
            default_err(self, log::last_error());
            return -1;
        }
        let to = (&raw mut address.addr).cast::<libc::sockaddr>();
        let length = &raw mut address.len;
        #[cfg(feature = "iouring")]
        if let Some(ops) = crate::uring_socket::of(self) {
            // SAFETY: the live view, the caller's buffer and the local address.
            return unsafe {
                ops.io(
                    ring::SENDTO,
                    buf.as_ptr().cast_mut().cast(),
                    buf.len(),
                    to.cast(),
                    length,
                    0,
                )
            };
        }
        // SAFETY: the address's live length.
        let to_length = unsafe { *length };
        let call =
            call!(|s, at, len| libc::sendto((*s).fields.fd, at.cast(), len, 0, to, to_length));
        // Availability was checked above.
        self.transfer_out(RETRY_INTERRUPTED | CHECKED, buf, call)
    }

    /// The peer certificate's PEM, or empty with the error set when there is none.
    #[must_use]
    pub fn peer_cert(self) -> Vec<u8> {
        let mut pem = vec![0_u8; SW_STACK_BUFFER_SIZE as usize];
        // SAFETY: the live network::Socket and the local buffer.
        let length = unsafe {
            socket_ssl_get_peer_certificate_pem(self.socket(), pem.as_mut_ptr().cast(), pem.len())
        };
        if let Ok(length) = usize::try_from(length) {
            pem.truncate(length);
            return pem;
        }
        set_err(self, SSL_EMPTY_PEER_CERTIFICATE);
        Vec::new()
    }

    /// `Socket::recv(buf, n)`.
    pub fn recv(self, buf: &mut [impl swoole_rs_corebase::sys::ReadSlot]) -> isize {
        #[cfg(feature = "iouring")]
        if let Some(ops) = crate::uring_socket::of(self) {
            return ring::recv(&ops, buf);
        }
        let call = call!(|s, at, len| socket_recv(s, at.cast(), len, 0));
        self.transfer_in(0, buf, call)
    }

    /// `Socket::send(buf, n)`.
    #[must_use]
    pub fn send(self, buf: &[u8]) -> isize {
        #[cfg(feature = "iouring")]
        if let Some(ops) = crate::uring_socket::of(self) {
            return ring::send(&ops, ring::SEND, buf);
        }
        let call = call!(|s, at, len| socket_send(s, at.cast(), len, 0));
        self.transfer_out(0, buf, call)
    }

    /// `Socket::recv_all(buf, n)`.
    pub fn recv_all(self, buf: &mut [u8]) -> isize {
        #[cfg(feature = "iouring")]
        if let Some(ops) = crate::uring_socket::of(self) {
            return ring::recv_all(&ops, buf);
        }
        let call = call!(|s, at, len| socket_recv(s, at.cast(), len, 0));
        self.transfer_in(ALL, buf, call)
    }

    /// `Socket::send_all(buf, n)`.
    #[must_use]
    pub fn send_all(self, buf: &[u8]) -> isize {
        #[cfg(feature = "iouring")]
        if let Some(ops) = crate::uring_socket::of(self) {
            return ring::send_all(&ops, buf);
        }
        let call = call!(|s, at, len| socket_send(s, at.cast(), len, 0));
        self.transfer_out(ALL, buf, call)
    }

    /// `Socket::read(buf, n)`.
    pub fn read(self, buf: &mut [impl swoole_rs_corebase::sys::ReadSlot]) -> isize {
        #[cfg(feature = "iouring")]
        if let Some(ops) = crate::uring_socket::of(self) {
            return ring::read(&ops, buf);
        }
        let call = call!(|s, at, len| libc::read((*s).fields.fd, at.cast(), len));
        self.transfer_in(0, buf, call)
    }

    /// `Socket::write(buf, n)`.
    #[must_use]
    pub fn write(self, buf: &[u8]) -> isize {
        #[cfg(feature = "iouring")]
        if let Some(ops) = crate::uring_socket::of(self) {
            return ring::send(&ops, ring::WRITE, buf);
        }
        let call = call!(|s, at, len| libc::write((*s).fields.fd, at.cast(), len));
        self.transfer_out(0, buf, call)
    }

    /// `Socket::readv(io_vector)`.
    ///
    /// # Safety
    /// `vector` is a live `IOVector`.
    pub unsafe fn readv(self, vector: *mut c_void) -> isize {
        uring!(self, |ops| ring::swr_v1_hook_uring_vector(
            ops,
            &iov_owner(vector),
            false,
            false
        ));
        let call = call!(|s, _, _| socket_readv(s, vector));
        // SAFETY: as the caller promises.
        unsafe { self.transfer(READ, 0, null_mut(), 0, call) }
    }

    /// `Socket::writev(io_vector)`.
    ///
    /// # Safety
    /// As [`Co::readv`].
    pub unsafe fn writev(self, vector: *mut c_void) -> isize {
        uring!(self, |ops| ring::swr_v1_hook_uring_vector(
            ops,
            &iov_owner(vector),
            true,
            false
        ));
        let call = call!(|s, _, _| socket_writev(s, vector));
        // SAFETY: as the caller promises.
        unsafe { self.transfer(WRITE, 0, null_mut(), 0, call) }
    }

    /// `Socket::readv_all(io_vector)`.
    ///
    /// # Safety
    /// As [`Co::readv`].
    pub unsafe fn readv_all(self, vector: *mut c_void) -> isize {
        uring!(self, |ops| ring::swr_v1_hook_uring_vector(
            ops,
            &iov_owner(vector),
            false,
            true
        ));
        // SAFETY: the socket's current network::Socket, over the live vector.
        let call = || unsafe { socket_readv(self.socket(), vector) };
        // SAFETY: as the caller promises.
        let left = || unsafe { remaining(vector) };
        vector_all(self, READ, call, left)
    }

    /// `Socket::writev_all(io_vector)`.
    ///
    /// # Safety
    /// As [`Co::readv`].
    pub unsafe fn writev_all(self, vector: *mut c_void) -> isize {
        uring!(self, |ops| ring::swr_v1_hook_uring_vector(
            ops,
            &iov_owner(vector),
            true,
            true
        ));
        // SAFETY: the socket's current network::Socket, over the live vector.
        let call = || unsafe { socket_writev(self.socket(), vector) };
        // SAFETY: as the caller promises.
        let left = || unsafe { remaining(vector) };
        vector_all(self, WRITE, call, left)
    }

    /// `set_err(error, message)` with owned text. A failed copy (allocation) leaves
    /// the previous message, as the code and errno are set.
    fn set_err_text(self, error: c_int, message: &str) {
        owned_err(self, error, message.as_bytes());
    }

    /// `Socket::sendfile(filename, offset, length)`: `length` bytes from `offset`, or
    /// the rest of the file when `length` is 0.
    #[must_use]
    pub fn sendfile(self, filename: &CStr, offset: libc::off_t, length: usize) -> bool {
        /// `SW_ERROR_SYSTEM_CALL_FAIL`.
        const SYSTEM_CALL_FAIL: c_int = 502;
        if !available(self, WRITE) {
            return false;
        }
        // SAFETY: a NUL-terminated path.
        let file = OpenFile(unsafe { libc::open(filename.as_ptr(), libc::O_RDONLY) });
        let name = filename.to_string_lossy();
        if file.0 == -1 {
            let error = errno();
            self.set_err_text(error, &format!("open({name}) failed, {}", strerror(error)));
            return false;
        }
        let end = if length == 0 {
            // SAFETY: a stat record is plain data.
            let mut stat: libc::stat = unsafe { core::mem::zeroed() };
            // SAFETY: the open descriptor, into the local record.
            if unsafe { libc::fstat(file.0, &raw mut stat) } != 0 {
                let error = errno();
                self.set_err_text(error, &format!("fstat({name}) failed, {}", strerror(error)));
                return false;
            }
            usize::try_from(stat.st_size).unwrap_or(0)
        } else {
            length.wrapping_add(usize::try_from(offset).unwrap_or(0))
        };
        #[cfg(feature = "iouring")]
        if let Some(ops) = crate::uring_socket::of(self) {
            return ring::sendfile(&ops, file.0, offset, end);
        }
        let mut timer = Timer::new(self, WRITE, self.timeout(timeout_kind(WRITE)));
        let mut offset = offset;
        let send = |offset: &mut libc::off_t, length| {
            // SAFETY: the socket's current network::Socket and the open file.
            unsafe { socket_sendfile_fd(self.socket(), file.0, offset, length) }
        };
        let ready = || timer.start() && wait_for(self, WRITE);
        let outcome = send_range(&mut offset, end, send, ready);
        let error = errno();
        timer.stop();
        match outcome {
            0 => return true,
            ZERO => borrowed_err(self, SYSTEM_CALL_FAIL, c"sendfile return zero".as_ptr()),
            FAILED => {
                let fd = self.sock_fd();
                self.set_err_text(
                    error,
                    &format!("sendfile({fd}, {name}) failed, {}", strerror(error)),
                );
            }
            _ => {}
        }
        false
    }

    /// `Socket::recvfrom(buf, n, addr, addrlen)`, retrying `EINTR` at once.
    ///
    /// # Safety
    /// `buf` holds `n` writable bytes and `addr` and `addrlen` are as recvfrom(2)
    /// takes them.
    pub unsafe fn recvfrom(
        self,
        buf: *mut c_void,
        n: usize,
        addr: *mut libc::sockaddr,
        addrlen: *mut libc::socklen_t,
    ) -> isize {
        uring!(self, |ops| ops.io(
            ring::RECVFROM,
            buf,
            n,
            addr.cast(),
            addrlen,
            0
        ));
        let fd = self.sock_fd();
        let call = call!(|_, at, len| libc::recvfrom(fd, at.cast(), len, 0, addr, addrlen));
        // SAFETY: as the caller promises; each try receives into the caller's buffer.
        unsafe { self.transfer(READ, RETRY_INTERRUPTED, buf.cast(), n, call) }
    }

    /// `Socket::recvfrom(buf, n)`: into the `network::Socket`'s peer address.
    pub fn recvfrom_peer(self, buf: &mut [u8]) -> isize {
        let (buf, n) = (buf.as_mut_ptr().cast(), buf.len());
        let receive = |addr, addrlen| {
            // SAFETY: the caller's buffer, into the record's peer address.
            unsafe { self.recvfrom(buf, n, addr, addrlen) }
        };
        crate::transfer::recvfrom_peer(self, receive)
    }

    /// `Socket::recvmsg(msg, flags)`.
    ///
    /// # Safety
    /// `msg` is as recvmsg(2) takes it.
    pub unsafe fn recvmsg(self, msg: *mut libc::msghdr, flags: c_int) -> isize {
        uring!(self, |ops| ops.io(
            ring::RECVMSG,
            null_mut(),
            0,
            msg.cast(),
            null_mut(),
            flags
        ));
        let fd = self.sock_fd();
        let call = call!(|_, _, _| libc::recvmsg(fd, msg, flags));
        // SAFETY: as the caller promises.
        unsafe { self.transfer(READ, 0, null_mut(), 0, call) }
    }

    /// `Socket::sendmsg(msg, flags)`.
    ///
    /// # Safety
    /// `msg` is as sendmsg(2) takes it.
    #[must_use]
    pub unsafe fn sendmsg(self, msg: *const libc::msghdr, flags: c_int) -> isize {
        uring!(self, |ops| ops.io(
            ring::SENDMSG,
            null_mut(),
            0,
            msg.cast_mut().cast(),
            null_mut(),
            flags
        ));
        let fd = self.sock_fd();
        let call = call!(|_, _, _| libc::sendmsg(fd, msg, flags));
        // SAFETY: as the caller promises.
        unsafe { self.transfer(WRITE, 0, null_mut(), 0, call) }
    }

    /// `Socket::recv_once(buf, n)`: one `network::Socket` receive, no wait. The
    /// socket is open.
    pub fn recv_once(self, buf: &mut [u8]) -> isize {
        let (buf, n) = (buf.as_mut_ptr().cast(), buf.len());
        // SAFETY: the open socket's network::Socket, into the caller's buffer.
        let io = || unsafe { socket_recv(self.socket(), buf, n, 0) };
        #[cfg(feature = "iouring")]
        if let Some(ops) = crate::uring_socket::of(self) {
            return ring::once(&ops, io);
        }
        io()
    }

    /// `Socket::send_once(buf, n)`: one `network::Socket` send, no wait. The socket
    /// is open.
    #[must_use]
    pub fn send_once(self, buf: &[u8]) -> isize {
        let (buf, n) = (buf.as_ptr().cast_mut().cast(), buf.len());
        // SAFETY: the open socket's network::Socket, from the caller's buffer.
        let io = || unsafe { socket_send(self.socket(), buf, n, 0) };
        #[cfg(feature = "iouring")]
        if let Some(ops) = crate::uring_socket::of(self) {
            return ring::once(&ops, io);
        }
        io()
    }

    /// `Socket::peek(buf, n)`: one `network::Socket` peek, then `check_return_value`.
    /// The socket is open.
    pub fn peek(self, buf: &mut [impl swoole_rs_corebase::sys::ReadSlot]) -> isize {
        // SAFETY: the open socket's network::Socket, into the caller's buffer.
        let result = unsafe { socket_peek(self.socket(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        check(self, result);
        result
    }

    /// `Socket::poll(event, timeout)`.
    #[must_use]
    pub fn poll(self, event: u32, timeout: f64) -> bool {
        #[cfg(feature = "iouring")]
        if let Some(ops) = crate::uring_socket::of(self) {
            return ring::poll(&ops, event, timeout);
        }
        crate::socket_wait::poll(self, event, timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::co_io::test_host::{install, take};
    use crate::co_socket::{CoSocket, init};
    use core::ffi::c_long;
    use core::mem::MaybeUninit;

    /// A native `Socket`: the record, nothing more.
    #[repr(C)]
    struct Native {
        record: CoSocket,
    }

    struct Object(Box<Native>);

    impl Object {
        fn new() -> Self {
            let mut storage = Box::new(MaybeUninit::<Native>::uninit());
            let native = storage.as_mut_ptr();
            unsafe { init(&raw mut (*native).record) };
            let mut native = unsafe { Box::from_raw(Box::into_raw(storage).cast::<Native>()) };
            native.record.socket = SocketRecord::with_fd(-1, 6).into_raw();
            native.record.sock_fd = 100;
            let object = (&raw mut *native).cast::<c_void>();
            native.record.socket_mut().object = object;
            Self(native)
        }
        fn co(&mut self) -> Co {
            unsafe { Co::of((&raw mut *self.0).cast()) }
        }
        fn event(&mut self) -> Event {
            unsafe { Event::new(-1, 0, 6, self.0.record.socket.cast()) }
        }
        fn bind(&mut self, event: u32, co: *mut c_long) {
            unsafe { (*self.0.record.state).bind(event, co.cast()) };
        }
    }

    impl Drop for Object {
        fn drop(&mut self) {
            unsafe {
                SocketRecord::free(self.0.record.socket);
                crate::co_socket::swr_v1_hook_co_socket_fini(&raw mut self.0.record);
            }
        }
    }

    trait SocketMut {
        fn socket_mut(&mut self) -> &mut SocketFields;
    }
    impl SocketMut for CoSocket {
        fn socket_mut(&mut self) -> &mut SocketFields {
            unsafe { &mut *self.socket }
        }
    }

    #[test]
    fn the_record_is_the_native_socket() {
        let mut object = Object::new();
        assert_eq!(object.co().raw(), &raw mut object.0.record);
    }

    #[test]
    fn a_readable_event_resumes_the_reader_and_clears_the_error() {
        install();
        let mut object = Object::new();
        object.0.record.err_code = libc::EAGAIN;
        let mut co: c_long = 3;
        object.bind(READ, &raw mut co);
        let mut event = object.event();
        assert_eq!(
            readable_event_callback(unsafe { Running::new(core::ptr::null_mut()) }, &mut event),
            0
        );
        assert_eq!(object.0.record.err_code, 0);
        assert_eq!(take(), ["last-error:0", "resume:3"]);
        // Nobody writes: a writable event wakes no one.
        assert_eq!(
            writable_event_callback(unsafe { Running::new(core::ptr::null_mut()) }, &mut event),
            0
        );
        assert_eq!(take(), ["last-error:0"]);
    }

    #[test]
    fn an_error_event_wakes_the_writer_then_the_reader() {
        install();
        let mut object = Object::new();
        let (mut reader, mut writer): (c_long, c_long) = (1, 2);
        object.bind(READ, &raw mut reader);
        object.bind(WRITE, &raw mut writer);
        object.0.record.socket_mut().removed = 0;
        let mut event = object.event();
        error_event_callback(unsafe { Running::new(core::ptr::null_mut()) }, &mut event);
        assert_eq!(
            take(),
            ["last-error:0", "resume:2", "last-error:0", "resume:1"]
        );
    }

    #[test]
    fn sendfile_reports_a_file_it_cannot_open() {
        install();
        let mut object = Object::new();
        let path = c"/nonexistent/swoole-rs-sendfile";
        assert!(!object.co().sendfile(path, 0, 0));
        assert_eq!(object.0.record.err_code, libc::ENOENT);
        let message = unsafe { CStr::from_ptr(object.0.record.err_msg) };
        assert_eq!(
            message.to_str().unwrap(),
            format!(
                "open({}) failed, {}",
                path.to_str().unwrap(),
                strerror(libc::ENOENT)
            )
        );
    }

    #[test]
    fn sendfile_of_an_empty_file_sends_nothing() {
        install();
        let mut object = Object::new();
        let path = std::env::temp_dir().join(format!("swoole-rs-sendfile-{}", std::process::id()));
        std::fs::write(&path, b"").unwrap();
        let name = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        assert!(object.co().sendfile(&name, 0, 0));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn accept_reports_a_failed_accept_without_a_new_socket() {
        install();
        let mut object = Object::new();
        assert!(object.co().accept(0.0).is_none());
        assert_eq!(object.0.record.err_code, libc::EBADF);
        assert!(!take().iter().any(|entry| entry == "accepted"));
    }

    #[test]
    fn timeouts_are_the_network_sockets() {
        let mut object = Object::new();
        object.co().set_timeout(2.5, 4);
        assert_eq!(object.co().timeout(4), 2.5);
    }

    #[test]
    fn listen_stores_the_backlog_and_reports_a_closed_socket() {
        install();
        let mut object = Object::new();
        object.0.record.sock_fd = -1;
        assert!(!object.co().listen(0));
        assert_eq!(object.0.record.err_code, libc::EBADF);
        assert_eq!(object.0.record.backlog, 0);
    }

    #[test]
    fn options_go_to_the_network_sockets_descriptor() {
        let mut object = Object::new();
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        object.0.record.socket_mut().fd = fd;
        let socket = object.co();
        assert!(socket.set_int_option(libc::SOL_SOCKET, libc::SO_REUSEADDR, 1));
        let mut value = 0;
        assert!(socket.get_int_option(libc::SOL_SOCKET, libc::SO_REUSEADDR, &mut value));
        assert_ne!(value, 0);
        unsafe { libc::close(fd) };
        object.0.record.socket_mut().fd = -1;
        assert!(!socket.set_int_option(libc::SOL_SOCKET, libc::SO_REUSEADDR, 1));
        assert!(!socket.get_int_option(libc::SOL_SOCKET, libc::SO_REUSEADDR, &mut value));
    }
}
