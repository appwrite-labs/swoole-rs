//! The `coroutine::Socket` members of `src/coroutine/socket.cc` that are whole Rust
//! operations, defined under their upstream (Itanium) symbols when the hybrid
//! links this crate (`upstream-symbols`): the reactor callbacks, the
//! local and peer names, the timeouts and options, the buffered and packet reads,
//! `bind` of an address and `listen`, and the transfers (`recv`, `send`, their
//! `_all` and `_once` forms, `read`, `write`, `readv`, `writev` and their `_all`
//! forms, `recvmsg`, `sendmsg`, `recvfrom`), `peek` and `poll`.
//! Upstream's `GoogleTest` suites call them through the class's own declarations.
//!
//! Each takes the native object (`this`). `Socket` is dynamic and its one base is
//! the record (`swr_v1_co_socket`, not dynamic, pointer-aligned), so under the
//! Itanium ABI the record follows the vtable pointer; system.cc asserts that shape.
use crate::buffered::{swr_v1_hook_co_socket_recv_buffered, swr_v1_hook_recv_line};
use crate::co_io::{
    Timer, available, check, errno, host, set_err, swr_v1_hook_co_socket_borrowed_err,
    swr_v1_hook_co_socket_default_err, swr_v1_hook_co_socket_owned_err,
};
use crate::co_socket::{CoSocket, swr_v1_hook_co_socket_peer_name};
use crate::packet::swr_v1_hook_co_socket_recv_packet;
use crate::sendfile::{FAILED, ZERO, send_range};
use crate::shutdown::READ;
use crate::socket_wait::{
    swr_v1_hook_co_socket_error_event, swr_v1_hook_co_socket_poll, swr_v1_hook_socket_ready, wait,
};
use crate::transfer::{ALL, RETRY_INTERRUPTED, swr_v1_hook_co_socket_transfer};
use crate::vector::vector_all;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::mem::{align_of, size_of};
use core::ptr::null_mut;
use swoole_rs_corebase::log;
use swoole_rs_reactor::{
    Event, ReactorIov, SocketAddress, SocketRecord, swr_v1_reactor_iov_view, swr_v1_socket_bind,
    swr_v1_socket_get_name, swr_v1_socket_listen, swr_v1_socket_set_timeout, swr_v1_socket_timeout,
};
use swoole_rs_tls::upstream::{
    sendfile as socket_sendfile_fd, socket_peek, socket_readv, socket_recv, socket_send,
    socket_writev,
};

/// `SW_EVENT_WRITE`.
const WRITE: u32 = 1 << 10;
/// `sizeof(int)`.
const INT_LENGTH: libc::socklen_t = c_int::BITS / 8;

const _: () = assert!(
    align_of::<CoSocket>() <= size_of::<*const c_void>(),
    "the record follows the vtable pointer"
);

/// The record inside the native `Socket` at `this`.
pub(crate) fn record(this: *const c_void) -> *mut CoSocket {
    this.wrapping_byte_add(size_of::<*const c_void>())
        .cast_mut()
        .cast()
}

/// The `network::Socket` a reactor event came from.
///
/// # Safety
/// `event` is the reactor's live event for a coroutine socket.
unsafe fn event_socket(event: *const Event) -> *mut SocketRecord {
    // SAFETY: as the caller promises.
    unsafe { (*event).socket }.cast()
}

/// The `network::Socket` record of the native `Socket` at `this`.
///
/// # Safety
/// `this` is a live native socket.
unsafe fn network(this: *const c_void) -> *mut SocketRecord {
    // SAFETY: as the caller promises.
    unsafe { (*record(this)).socket }
}

/// `Socket::readable_event_callback` and `writable_event_callback`: Rust picks the
/// coroutine to wake, a TLS wait's other direction, or none while a barrier waits.
///
/// # Safety
/// `event` is the reactor's live event for a coroutine socket.
unsafe fn ready(event: *const Event, kind: u32) -> c_int {
    // SAFETY: as the caller promises.
    let source = unsafe { event_socket(event) };
    // SAFETY: the live network::Socket the event came from.
    let source = unsafe { &*source };
    let socket = record(source.object);
    // SAFETY: the live socket the reactor dispatched to.
    unsafe { set_err(socket, 0) };
    // SAFETY: as above.
    let state = unsafe { (*socket).state };
    // SAFETY: the record's live state.
    let co = unsafe { swr_v1_hook_socket_ready(state, kind, source.event_hup != 0) };
    if !co.is_null() {
        // SAFETY: the coroutine Rust chose.
        unsafe { (host().resume)(co) };
    }
    0
}

/// `Socket::readable_event_callback(reactor, event)`.
///
/// # Safety
/// As [`ready`].
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZN6swoole9coroutine6Socket23readable_event_callbackEPNS_7ReactorEPNS_5EventE"
    )
)]
pub unsafe extern "C" fn readable_event_callback(
    _reactor: *mut c_void,
    event: *mut Event,
) -> c_int {
    // SAFETY: forwarded.
    unsafe { ready(event, READ) }
}

/// `Socket::writable_event_callback(reactor, event)`.
///
/// # Safety
/// As [`ready`].
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZN6swoole9coroutine6Socket23writable_event_callbackEPNS_7ReactorEPNS_5EventE"
    )
)]
pub unsafe extern "C" fn writable_event_callback(
    _reactor: *mut c_void,
    event: *mut Event,
) -> c_int {
    // SAFETY: forwarded.
    unsafe { ready(event, WRITE) }
}

/// `Socket::error_event_callback(reactor, event)`: wakes the writer, then the
/// reader unless the writer closed the socket.
///
/// # Safety
/// As [`ready`].
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZN6swoole9coroutine6Socket20error_event_callbackEPNS_7ReactorEPNS_5EventE"
    )
)]
pub unsafe extern "C" fn error_event_callback(_reactor: *mut c_void, event: *mut Event) -> c_int {
    // SAFETY: as the caller promises.
    let source = unsafe { event_socket(event) };
    // SAFETY: as above.
    let object = unsafe { (*source).object };
    // SAFETY: the live socket and the network::Socket the event came from.
    unsafe { swr_v1_hook_co_socket_error_event(record(object), object, source) };
    0
}

/// `Socket::getsockname()`.
///
/// # Safety
/// `this` is a live, open native socket.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole9coroutine6Socket11getsocknameEv")
)]
#[must_use]
pub unsafe extern "C" fn getsockname(this: *const c_void) -> bool {
    // SAFETY: as the caller promises.
    let socket = unsafe { network(this) };
    // SAFETY: its network::Socket record.
    unsafe { swr_v1_socket_get_name(socket) == 0 }
}

/// `Socket::getpeername(sa)`.
///
/// # Safety
/// `this` is a live native socket and `address` writable.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket11getpeernameEPNS_7network7AddressE")
)]
pub unsafe extern "C" fn getpeername(this: *mut c_void, address: *mut SocketAddress) -> bool {
    let socket = record(this);
    // SAFETY: as the caller promises.
    let error = unsafe { swr_v1_hook_co_socket_peer_name(socket, address) };
    if error != 0 {
        // SAFETY: as above.
        unsafe { set_err(socket, error) };
    }
    error == 0
}

/// `Socket::get_timeout(type)`: the `network::Socket`'s, or -1 for no such type.
///
/// # Safety
/// `this` is a live, open native socket.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole9coroutine6Socket11get_timeoutE13swTimeoutType")
)]
#[must_use]
pub unsafe extern "C" fn get_timeout(this: *const c_void, kind: c_int) -> f64 {
    // SAFETY: as the caller promises.
    let socket = unsafe { network(this) };
    // SAFETY: its network::Socket record's timeouts.
    let timeouts = unsafe { &raw const (*socket).timeouts };
    // SAFETY: as above.
    let timeout = unsafe { swr_v1_socket_timeout(timeouts, kind) };
    debug_assert!(!timeout.is_null(), "timeout type {kind}");
    // SAFETY: null or one of the record's timeouts.
    unsafe { timeout.as_ref() }.copied().unwrap_or(-1.0)
}

/// `Socket::set_timeout(timeout, type)`.
///
/// # Safety
/// `this` is a live, open native socket.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole9coroutine6Socket11set_timeoutEdi")
)]
pub unsafe extern "C" fn set_timeout(this: *const c_void, timeout: f64, kind: c_int) {
    // SAFETY: as the caller promises.
    let socket = unsafe { network(this) };
    // SAFETY: its network::Socket record's timeouts.
    let timeouts = unsafe { &raw mut (*socket).timeouts };
    // SAFETY: as above.
    unsafe { swr_v1_socket_set_timeout(timeouts, timeout, kind) };
}

/// `Socket::recv_with_buffer(buf, n)`.
///
/// # Safety
/// `this` is a live native socket and `out` holds `size` writable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket16recv_with_bufferEPvm")
)]
pub unsafe extern "C" fn recv_with_buffer(this: *mut c_void, out: *mut u8, size: usize) -> isize {
    // SAFETY: as the caller promises.
    unsafe { swr_v1_hook_co_socket_recv_buffered(record(this), out, size) }
}

/// `Socket::recv_line(buf, maxlen)`, one buffered byte at a time.
///
/// # Safety
/// As [`recv_with_buffer`], with `limit` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket9recv_lineEPvm")
)]
pub unsafe extern "C" fn recv_line(this: *mut c_void, out: *mut u8, limit: usize) -> isize {
    // SAFETY: as the caller promises; each read is the socket's buffered read.
    unsafe { swr_v1_hook_recv_line(recv_with_buffer, this, out, limit) }
}

/// `Socket::recv_packet(timeout)`: the packet's length; it is the read buffer's offset.
///
/// # Safety
/// `this` is a live native socket.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket11recv_packetEd")
)]
pub unsafe extern "C" fn recv_packet(this: *mut c_void, timeout: f64) -> isize {
    // SAFETY: as the caller promises.
    unsafe { swr_v1_hook_co_socket_recv_packet(record(this), timeout) }
}

/// `Socket::set_option(level, optname, optval, optlen)`: `setsockopt`, warning on failure.
///
/// # Safety
/// `this` is a live, open native socket and `value` holds `length` readable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole9coroutine6Socket10set_optionEiiPKvj")
)]
#[must_use]
pub unsafe extern "C" fn set_option(
    this: *const c_void,
    level: c_int,
    name: c_int,
    value: *const c_void,
    length: libc::socklen_t,
) -> bool {
    // SAFETY: as the caller promises.
    let socket = unsafe { &*record(this) };
    // SAFETY: the open socket's network::Socket record.
    let fd = unsafe { (*socket.socket).fd };
    // SAFETY: as the caller promises.
    if unsafe { libc::setsockopt(fd, level, name, value, length) } < 0 {
        log::sys_warning(
            c"Socket::set_option",
            format_args!(
                "setsockopt({}, {level}, {name}, {length}) failed",
                socket.sock_fd
            ),
        );
        return false;
    }
    true
}

/// `Socket::set_option(level, optname, optval)` of an `int`.
///
/// # Safety
/// `this` is a live, open native socket.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole9coroutine6Socket10set_optionEiii")
)]
#[must_use]
pub unsafe extern "C" fn set_int_option(
    this: *const c_void,
    level: c_int,
    name: c_int,
    value: c_int,
) -> bool {
    // SAFETY: as the caller promises; `value` is an `int`.
    unsafe { set_option(this, level, name, (&raw const value).cast(), INT_LENGTH) }
}

/// `Socket::get_option(level, optname, optval, optlen)`: `getsockopt`, warning on failure.
///
/// # Safety
/// `this` is a live, open native socket; `value` holds `*length` writable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole9coroutine6Socket10get_optionEiiPvPj")
)]
pub unsafe extern "C" fn get_option(
    this: *const c_void,
    level: c_int,
    name: c_int,
    value: *mut c_void,
    length: *mut libc::socklen_t,
) -> bool {
    // SAFETY: as the caller promises.
    let socket = unsafe { &*record(this) };
    // SAFETY: the open socket's network::Socket record.
    let fd = unsafe { (*socket.socket).fd };
    // SAFETY: as the caller promises.
    if unsafe { libc::getsockopt(fd, level, name, value, length) } < 0 {
        log::sys_warning(
            c"Socket::get_option",
            format_args!("getsockopt({}, {level}, {name}) failed", socket.sock_fd),
        );
        return false;
    }
    true
}

/// `Socket::get_option(level, optname, optval)` into an `int`.
///
/// # Safety
/// `this` is a live, open native socket and `value` a writable `int`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole9coroutine6Socket10get_optionEiiPi")
)]
pub unsafe extern "C" fn get_int_option(
    this: *const c_void,
    level: c_int,
    name: c_int,
    value: *mut c_int,
) -> bool {
    let mut length = INT_LENGTH;
    // SAFETY: as the caller promises.
    unsafe { get_option(this, level, name, value.cast(), &raw mut length) }
}

/// `Socket::bind(sa, len)`.
///
/// # Safety
/// `this` is a live, open native socket and `address` holds `length` readable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket4bindEPK8sockaddrj")
)]
pub unsafe extern "C" fn bind(
    this: *mut c_void,
    address: *const libc::sockaddr,
    length: libc::socklen_t,
) -> bool {
    let socket = record(this);
    // SAFETY: as the caller promises.
    let network = unsafe { (*socket).socket };
    // SAFETY: its network::Socket record; the address as the caller promises.
    if unsafe { swr_v1_socket_bind(network, address, length) } < 0 {
        // SAFETY: as the caller promises.
        unsafe { swr_v1_hook_co_socket_default_err(socket, log::last_error()) };
        return false;
    }
    true
}

/// `Socket::listen(backlog)`: `SW_BACKLOG` for a non-positive backlog; a listening
/// socket's TLS is the server side.
///
/// # Safety
/// `this` is a live native socket.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket6listenEi")
)]
pub unsafe extern "C" fn listen(this: *mut c_void, backlog: c_int) -> bool {
    /// `SW_BACKLOG`.
    const BACKLOG: c_int = 512;
    let socket = record(this);
    // SAFETY: as the caller promises; `SW_EVENT_NULL` skips the bound check.
    if !unsafe { available(socket, 0) } {
        return false;
    }
    let backlog = if backlog <= 0 { BACKLOG } else { backlog };
    // SAFETY: as the caller promises.
    let record = unsafe { &mut *socket };
    record.backlog = backlog;
    // SAFETY: the open socket's network::Socket record.
    if unsafe { swr_v1_socket_listen(record.socket, backlog) } < 0 {
        // SAFETY: as above.
        unsafe { set_err(socket, errno()) };
        return false;
    }
    record.ssl_is_server = true;
    true
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

/// A transfer's `network::Socket` call, over the native socket's current one.
struct Transfer<F> {
    this: *mut c_void,
    call: F,
}

/// `Socket::transfer(event, buffer, length, flags, call)`: Rust's retry loop
/// (`swr_v1_hook_co_socket_transfer`) over `call`, which gets the
/// `network::Socket` at each try.
///
/// # Safety
/// `this` is a live native socket; `buffer` holds `length` bytes or is null; `call`
/// transfers on the `network::Socket` it is given.
unsafe fn transfer<F: FnMut(*mut SocketRecord, *mut u8, usize) -> isize>(
    this: *mut c_void,
    event: u32,
    flags: u32,
    buffer: *mut u8,
    length: usize,
    call: F,
) -> isize {
    unsafe extern "C" fn each<F: FnMut(*mut SocketRecord, *mut u8, usize) -> isize>(
        context: *mut c_void,
        at: *mut u8,
        length: usize,
    ) -> isize {
        // SAFETY: `transfer`'s own Transfer<F>, alive until the transfer returns.
        let transfer = unsafe { &mut *context.cast::<Transfer<F>>() };
        // SAFETY: the transfer runs on a live native socket.
        let socket = unsafe { network(transfer.this) };
        (transfer.call)(socket, at, length)
    }
    let mut transfer = Transfer { this, call };
    let context = (&raw mut transfer).cast();
    // SAFETY: as the caller promises; `each` matches `context`.
    unsafe {
        swr_v1_hook_co_socket_transfer(
            record(this),
            event,
            flags,
            buffer,
            length,
            each::<F>,
            context,
        )
    }
}

/// `Socket::recv(buf, n)`.
///
/// # Safety
/// `this` is a live native socket and `buf` holds `n` writable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket4recvEPvm")
)]
pub unsafe extern "C" fn recv(this: *mut c_void, buf: *mut c_void, n: usize) -> isize {
    let call = call!(|s, at, len| socket_recv(s, at.cast(), len, 0));
    // SAFETY: as the caller promises; each try receives into the transfer's buffer.
    unsafe { transfer(this, READ, 0, buf.cast(), n, call) }
}

/// `Socket::send(buf, n)`.
///
/// # Safety
/// `this` is a live native socket and `buf` holds `n` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket4sendEPKvm")
)]
pub unsafe extern "C" fn send(this: *mut c_void, buf: *const c_void, n: usize) -> isize {
    let call = call!(|s, at, len| socket_send(s, at.cast(), len, 0));
    // SAFETY: as the caller promises; each try sends from the transfer's buffer.
    unsafe { transfer(this, WRITE, 0, buf.cast_mut().cast(), n, call) }
}

/// `Socket::recv_all(buf, n)`.
///
/// # Safety
/// As [`recv`].
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket8recv_allEPvm")
)]
pub unsafe extern "C" fn recv_all(this: *mut c_void, buf: *mut c_void, n: usize) -> isize {
    let call = call!(|s, at, len| socket_recv(s, at.cast(), len, 0));
    // SAFETY: as the caller promises; each try receives into the transfer's buffer.
    unsafe { transfer(this, READ, ALL, buf.cast(), n, call) }
}

/// `Socket::send_all(buf, n)`.
///
/// # Safety
/// As [`send`].
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket8send_allEPKvm")
)]
pub unsafe extern "C" fn send_all(this: *mut c_void, buf: *const c_void, n: usize) -> isize {
    let call = call!(|s, at, len| socket_send(s, at.cast(), len, 0));
    // SAFETY: as the caller promises; each try sends from the transfer's buffer.
    unsafe { transfer(this, WRITE, ALL, buf.cast_mut().cast(), n, call) }
}

/// `Socket::read(buf, n)`.
///
/// # Safety
/// As [`recv`].
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket4readEPvm")
)]
pub unsafe extern "C" fn read(this: *mut c_void, buf: *mut c_void, n: usize) -> isize {
    let call = call!(|s, at, len| libc::read((*s).fd, at.cast(), len));
    // SAFETY: as the caller promises; each try reads into the transfer's buffer.
    unsafe { transfer(this, READ, 0, buf.cast(), n, call) }
}

/// `Socket::write(buf, n)`.
///
/// # Safety
/// As [`send`].
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket5writeEPKvm")
)]
pub unsafe extern "C" fn write(this: *mut c_void, buf: *const c_void, n: usize) -> isize {
    let call = call!(|s, at, len| libc::write((*s).fd, at.cast(), len));
    // SAFETY: as the caller promises; each try writes from the transfer's buffer.
    unsafe { transfer(this, WRITE, 0, buf.cast_mut().cast(), n, call) }
}

/// `Socket::readv(io_vector)`.
///
/// # Safety
/// `this` is a live native socket and `vector` a live `IOVector`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket5readvEPNS_7network8IOVectorE")
)]
pub unsafe extern "C" fn readv(this: *mut c_void, vector: *mut c_void) -> isize {
    let call = call!(|s, _, _| socket_readv(s, vector));
    // SAFETY: as the caller promises.
    unsafe { transfer(this, READ, 0, null_mut(), 0, call) }
}

/// `Socket::writev(io_vector)`.
///
/// # Safety
/// As [`readv`].
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket6writevEPNS_7network8IOVectorE")
)]
pub unsafe extern "C" fn writev(this: *mut c_void, vector: *mut c_void) -> isize {
    let call = call!(|s, _, _| socket_writev(s, vector));
    // SAFETY: as the caller promises.
    unsafe { transfer(this, WRITE, 0, null_mut(), 0, call) }
}

/// The remaining count of the live `IOVector` at `vector`.
///
/// # Safety
/// `vector` is a live `IOVector`, whose first member is its owner.
unsafe fn remaining(vector: *const c_void) -> usize {
    // SAFETY: as the caller promises.
    let owner = unsafe { *vector.cast::<*const ReactorIov>() };
    // SAFETY: the vector's live owner.
    let count = unsafe { swr_v1_reactor_iov_view(owner) }.remain_count;
    usize::try_from(count).unwrap_or(0)
}

/// `Socket::readv_all(io_vector)`.
///
/// # Safety
/// As [`readv`].
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket9readv_allEPNS_7network8IOVectorE")
)]
pub unsafe extern "C" fn readv_all(this: *mut c_void, vector: *mut c_void) -> isize {
    let call = || {
        // SAFETY: as the caller promises.
        let socket = unsafe { network(this) };
        // SAFETY: the socket's current network::Socket, over the live vector.
        unsafe { socket_readv(socket, vector) }
    };
    // SAFETY: as the caller promises.
    let left = || unsafe { remaining(vector) };
    // SAFETY: as the caller promises.
    unsafe { vector_all(record(this), READ, call, left) }
}

/// `Socket::writev_all(io_vector)`.
///
/// # Safety
/// As [`readv`].
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket10writev_allEPNS_7network8IOVectorE")
)]
pub unsafe extern "C" fn writev_all(this: *mut c_void, vector: *mut c_void) -> isize {
    let call = || {
        // SAFETY: as the caller promises.
        let socket = unsafe { network(this) };
        // SAFETY: the socket's current network::Socket, over the live vector.
        unsafe { socket_writev(socket, vector) }
    };
    // SAFETY: as the caller promises.
    let left = || unsafe { remaining(vector) };
    // SAFETY: as the caller promises.
    unsafe { vector_all(record(this), WRITE, call, left) }
}

/// `set_err(error, message)` with owned text.
///
/// # Safety
/// `socket` is a live record, not borrowed elsewhere for the call.
unsafe fn set_err_text(socket: *mut CoSocket, error: c_int, message: &str) {
    // SAFETY: as the caller promises; the text is readable for the call. A failed
    // copy (allocation) leaves the previous message, as the code and errno are set.
    unsafe {
        swr_v1_hook_co_socket_owned_err(socket, error, message.as_ptr().cast(), message.len())
    };
}

/// libc's `strerror(error)`.
fn strerror(error: c_int) -> String {
    // SAFETY: strerror returns a NUL-terminated message for any code.
    let message = unsafe { libc::strerror(error) };
    // SAFETY: as above.
    unsafe { CStr::from_ptr(message) }
        .to_string_lossy()
        .into_owned()
}

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

/// `Socket::sendfile(filename, offset, length)`: `length` bytes from `offset`, or
/// the rest of the file when `length` is 0.
///
/// # Safety
/// `this` is a live native socket and `filename` a NUL-terminated path.
#[cfg_attr(
    all(feature = "upstream-symbols", target_os = "macos"),
    unsafe(export_name = "_ZN6swoole9coroutine6Socket8sendfileEPKcxm")
)]
#[cfg_attr(
    all(feature = "upstream-symbols", not(target_os = "macos")),
    unsafe(export_name = "_ZN6swoole9coroutine6Socket8sendfileEPKclm")
)]
pub unsafe extern "C" fn sendfile(
    this: *mut c_void,
    filename: *const c_char,
    offset: libc::off_t,
    length: usize,
) -> bool {
    /// `SW_ERROR_SYSTEM_CALL_FAIL`.
    const SYSTEM_CALL_FAIL: c_int = 502;
    let socket = record(this);
    // SAFETY: as the caller promises.
    if !unsafe { available(socket, WRITE) } {
        return false;
    }
    // SAFETY: as the caller promises.
    let file = OpenFile(unsafe { libc::open(filename, libc::O_RDONLY) });
    // SAFETY: as the caller promises.
    let name = unsafe { CStr::from_ptr(filename) }.to_string_lossy();
    if file.0 == -1 {
        let error = errno();
        let message = format!("open({name}) failed, {}", strerror(error));
        // SAFETY: the live record.
        unsafe { set_err_text(socket, error, &message) };
        return false;
    }
    let end = if length == 0 {
        // SAFETY: a stat record is plain data.
        let mut stat: libc::stat = unsafe { core::mem::zeroed() };
        // SAFETY: the open descriptor, into the local record.
        if unsafe { libc::fstat(file.0, &raw mut stat) } != 0 {
            let error = errno();
            let message = format!("fstat({name}) failed, {}", strerror(error));
            // SAFETY: the live record.
            unsafe { set_err_text(socket, error, &message) };
            return false;
        }
        usize::try_from(stat.st_size).unwrap_or(0)
    } else {
        length.wrapping_add(usize::try_from(offset).unwrap_or(0))
    };
    // SAFETY: the live record.
    let record_ref = unsafe { &*socket };
    // SAFETY: an available record is open; the read ends here.
    let timeout = unsafe { record_ref.record() }.timeouts.write;
    // SAFETY: the live record; the timer stops before this returns.
    let mut timer = unsafe { Timer::new(socket, WRITE, timeout) };
    let mut offset = offset;
    let send = |offset: &mut libc::off_t, length| {
        // SAFETY: as the caller promises.
        let socket = unsafe { network(this) };
        // SAFETY: the socket's current network::Socket and the open file.
        unsafe { socket_sendfile_fd(socket, file.0, offset, length) }
    };
    // SAFETY: the live record.
    let ready = || timer.start() && unsafe { wait(socket, WRITE, None, null_mut(), 0) };
    let outcome = send_range(&mut offset, end, send, ready);
    let error = errno();
    timer.stop();
    match outcome {
        0 => return true,
        ZERO => {
            let message = c"sendfile return zero".as_ptr();
            // SAFETY: the live record; the message is static.
            unsafe { swr_v1_hook_co_socket_borrowed_err(socket, SYSTEM_CALL_FAIL, message) };
        }
        FAILED => {
            // SAFETY: the live record.
            let fd = unsafe { (*socket).sock_fd };
            let message = format!("sendfile({fd}, {name}) failed, {}", strerror(error));
            // SAFETY: the live record.
            unsafe { set_err_text(socket, error, &message) };
        }
        _ => {}
    }
    false
}

/// `Socket::recvfrom(buf, n, addr, addrlen)`, retrying `EINTR` at once.
///
/// # Safety
/// `this` is a live native socket, `buf` holds `n` writable bytes and `addr` and
/// `addrlen` are as recvfrom(2) takes them.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket8recvfromEPvmP8sockaddrPj")
)]
pub unsafe extern "C" fn recvfrom(
    this: *mut c_void,
    buf: *mut c_void,
    n: usize,
    addr: *mut libc::sockaddr,
    addrlen: *mut libc::socklen_t,
) -> isize {
    let call = call!(|_, at, len| libc::recvfrom(
        (*record(this)).sock_fd,
        at.cast(),
        len,
        0,
        addr,
        addrlen
    ));
    // SAFETY: as the caller promises; each try receives into the transfer's buffer.
    unsafe { transfer(this, READ, RETRY_INTERRUPTED, buf.cast(), n, call) }
}

/// `Socket::recvmsg(msg, flags)`.
///
/// # Safety
/// `this` is a live native socket and `msg` as recvmsg(2) takes it.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket7recvmsgEP6msghdri")
)]
pub unsafe extern "C" fn recvmsg(this: *mut c_void, msg: *mut libc::msghdr, flags: c_int) -> isize {
    let call = call!(|_, _, _| libc::recvmsg((*record(this)).sock_fd, msg, flags));
    // SAFETY: as the caller promises.
    unsafe { transfer(this, READ, 0, null_mut(), 0, call) }
}

/// `Socket::sendmsg(msg, flags)`.
///
/// # Safety
/// `this` is a live native socket and `msg` as sendmsg(2) takes it.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket7sendmsgEPK6msghdri")
)]
pub unsafe extern "C" fn sendmsg(
    this: *mut c_void,
    msg: *const libc::msghdr,
    flags: c_int,
) -> isize {
    let call = call!(|_, _, _| libc::sendmsg((*record(this)).sock_fd, msg, flags));
    // SAFETY: as the caller promises.
    unsafe { transfer(this, WRITE, 0, null_mut(), 0, call) }
}

/// `Socket::recv_once(buf, n)`: one `network::Socket` receive, no wait.
///
/// # Safety
/// `this` is a live, open native socket and `buf` holds `n` writable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket9recv_onceEPvm")
)]
pub unsafe extern "C" fn recv_once(this: *mut c_void, buf: *mut c_void, n: usize) -> isize {
    // SAFETY: as the caller promises.
    let socket = unsafe { network(this) };
    // SAFETY: its network::Socket, into the caller's buffer.
    unsafe { socket_recv(socket, buf, n, 0) }
}

/// `Socket::send_once(buf, n)`: one `network::Socket` send, no wait.
///
/// # Safety
/// `this` is a live, open native socket and `buf` holds `n` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket9send_onceEPKvm")
)]
pub unsafe extern "C" fn send_once(this: *mut c_void, buf: *const c_void, n: usize) -> isize {
    // SAFETY: as the caller promises.
    let socket = unsafe { network(this) };
    // SAFETY: its network::Socket, from the caller's buffer.
    unsafe { socket_send(socket, buf, n, 0) }
}

/// `Socket::peek(buf, n)`: one `network::Socket` peek, then `check_return_value`.
///
/// # Safety
/// As [`recv_once`].
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket4peekEPvm")
)]
pub unsafe extern "C" fn peek(this: *mut c_void, buf: *mut c_void, n: usize) -> isize {
    // SAFETY: as the caller promises.
    let socket = unsafe { network(this) };
    // SAFETY: its network::Socket, into the caller's buffer.
    let result = unsafe { socket_peek(socket, buf, n, 0) };
    // SAFETY: the live record.
    unsafe { check(record(this), result) };
    result
}

/// `Socket::poll(event, timeout)`.
///
/// # Safety
/// `this` is a live native socket.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket4pollE11swEventTyped")
)]
pub unsafe extern "C" fn poll(this: *mut c_void, event: u32, timeout: f64) -> bool {
    // SAFETY: as the caller promises.
    unsafe { swr_v1_hook_co_socket_poll(record(this), event, timeout) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::co_io::test_host::{install, take};
    use crate::co_socket::init;
    use crate::shutdown::swr_v1_hook_socket_bind;
    use core::ffi::c_long;
    use core::mem::MaybeUninit;

    /// A native `Socket`'s shape: the vtable pointer, then the record.
    #[repr(C)]
    struct Native {
        vptr: *const c_void,
        record: CoSocket,
    }

    struct Object(Box<Native>);

    impl Object {
        fn new() -> Self {
            let mut storage = Box::new(MaybeUninit::<Native>::uninit());
            let native = storage.as_mut_ptr();
            unsafe { init(&raw mut (*native).record) };
            let mut native = unsafe { Box::from_raw(Box::into_raw(storage).cast::<Native>()) };
            native.vptr = core::ptr::null();
            native.record.socket = swoole_rs_reactor::swr_v1_socket_make(-1, 6);
            native.record.sock_fd = 100;
            let object = (&raw mut *native).cast::<c_void>();
            native.record.socket_mut().object = object;
            Self(native)
        }
        fn this(&mut self) -> *mut c_void {
            (&raw mut *self.0).cast()
        }
        fn event(&mut self) -> Event {
            Event {
                fd: -1,
                reactor_id: 0,
                fd_type: 6,
                socket: self.0.record.socket.cast(),
            }
        }
    }

    impl Drop for Object {
        fn drop(&mut self) {
            unsafe {
                swoole_rs_reactor::swr_v1_socket_dealloc(self.0.record.socket);
                crate::co_socket::swr_v1_hook_co_socket_fini(&raw mut self.0.record);
            }
        }
    }

    trait SocketMut {
        fn socket_mut(&mut self) -> &mut SocketRecord;
    }
    impl SocketMut for CoSocket {
        fn socket_mut(&mut self) -> &mut SocketRecord {
            unsafe { &mut *self.socket }
        }
    }

    #[test]
    fn the_record_follows_the_vtable_pointer() {
        let mut object = Object::new();
        assert_eq!(record(object.this()), &raw mut object.0.record);
    }

    #[test]
    fn a_readable_event_resumes_the_reader_and_clears_the_error() {
        install();
        let mut object = Object::new();
        object.0.record.err_code = libc::EAGAIN;
        let mut co: c_long = 3;
        unsafe { swr_v1_hook_socket_bind(object.0.record.state, READ, (&raw mut co).cast()) };
        let mut event = object.event();
        assert_eq!(
            unsafe { readable_event_callback(core::ptr::null_mut(), &raw mut event) },
            0
        );
        assert_eq!(object.0.record.err_code, 0);
        assert_eq!(take(), ["last-error:0", "resume:3"]);
        // Nobody writes: a writable event wakes no one.
        assert_eq!(
            unsafe { writable_event_callback(core::ptr::null_mut(), &raw mut event) },
            0
        );
        assert_eq!(take(), ["last-error:0"]);
    }

    #[test]
    fn an_error_event_wakes_the_writer_then_the_reader() {
        install();
        let mut object = Object::new();
        let (mut reader, mut writer): (c_long, c_long) = (1, 2);
        let state = object.0.record.state;
        unsafe { swr_v1_hook_socket_bind(state, READ, (&raw mut reader).cast()) };
        unsafe { swr_v1_hook_socket_bind(state, WRITE, (&raw mut writer).cast()) };
        object.0.record.socket_mut().removed = 0;
        let mut event = object.event();
        unsafe { error_event_callback(core::ptr::null_mut(), &raw mut event) };
        assert_eq!(
            take(),
            ["last-error:0", "resume:2", "last-error:0", "resume:1"]
        );
    }

    #[test]
    fn sendfile_reports_a_file_it_cannot_open() {
        install();
        let mut object = Object::new();
        let extra = crate::socket_extra::swr_v1_hook_socket_extra_new();
        object.0.record.extra = extra.owner;
        let path = c"/nonexistent/swoole-rs-sendfile";
        assert!(!unsafe { sendfile(object.this(), path.as_ptr(), 0, 0) });
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
        unsafe { crate::socket_extra::swr_v1_hook_socket_extra_free(extra.owner) };
    }

    #[test]
    fn sendfile_of_an_empty_file_sends_nothing() {
        install();
        let mut object = Object::new();
        let path = std::env::temp_dir().join(format!("swoole-rs-sendfile-{}", std::process::id()));
        std::fs::write(&path, b"").unwrap();
        let name = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        assert!(unsafe { sendfile(object.this(), name.as_ptr(), 0, 0) });
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn timeouts_are_the_network_sockets() {
        let mut object = Object::new();
        unsafe { set_timeout(object.this(), 2.5, 4) };
        assert_eq!(unsafe { get_timeout(object.this(), 4) }, 2.5);
    }

    #[test]
    fn listen_stores_the_backlog_and_reports_a_closed_socket() {
        install();
        let mut object = Object::new();
        object.0.record.sock_fd = -1;
        assert!(!unsafe { listen(object.this(), 0) });
        assert_eq!(object.0.record.err_code, libc::EBADF);
        assert_eq!(object.0.record.backlog, 0);
    }

    #[test]
    fn options_go_to_the_network_sockets_descriptor() {
        let mut object = Object::new();
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        object.0.record.socket_mut().fd = fd;
        assert!(unsafe { set_int_option(object.this(), libc::SOL_SOCKET, libc::SO_REUSEADDR, 1) });
        let mut value = 0;
        assert!(unsafe {
            get_int_option(
                object.this(),
                libc::SOL_SOCKET,
                libc::SO_REUSEADDR,
                &raw mut value,
            )
        });
        assert_ne!(value, 0);
        unsafe { libc::close(fd) };
        object.0.record.socket_mut().fd = -1;
        assert!(!unsafe { set_int_option(object.this(), libc::SOL_SOCKET, libc::SO_REUSEADDR, 1) });
        assert!(!unsafe {
            get_int_option(
                object.this(),
                libc::SOL_SOCKET,
                libc::SO_REUSEADDR,
                &raw mut value,
            )
        });
    }

    #[test]
    fn a_failed_bind_takes_errno_without_a_last_error() {
        install();
        let mut object = Object::new();
        let address: libc::sockaddr_in = unsafe { core::mem::zeroed() };
        let length = libc::socklen_t::try_from(size_of::<libc::sockaddr_in>()).unwrap();
        assert!(!unsafe { bind(object.this(), (&raw const address).cast(), length) });
        assert_eq!(object.0.record.err_code, libc::EBADF);
    }
}
