//! The `coroutine::Socket` members of `src/coroutine/socket.cc` that are whole Rust
//! operations, defined under their upstream (Itanium) symbols when the hybrid
//! links this crate (`upstream-symbols`): the reactor callbacks, the
//! local and peer names, the timeouts and options, the buffered and packet reads,
//! `bind` of an address and `listen`.
//! Upstream's `GoogleTest` suites call them through the class's own declarations.
//!
//! Each takes the native object (`this`). `Socket` is dynamic and its one base is
//! the record (`swr_v1_co_socket`, not dynamic, pointer-aligned), so under the
//! Itanium ABI the record follows the vtable pointer; socket.cc asserts that shape.
use crate::buffered::{swr_v1_hook_co_socket_recv_buffered, swr_v1_hook_recv_line};
use crate::co_io::{available, errno, host, set_err, swr_v1_hook_co_socket_default_err};
use crate::co_socket::{CoSocket, swr_v1_hook_co_socket_peer_name};
use crate::packet::swr_v1_hook_co_socket_recv_packet;
use crate::shutdown::READ;
use crate::socket_wait::{swr_v1_hook_co_socket_error_event, swr_v1_hook_socket_ready};
use core::ffi::{c_int, c_void};
use core::mem::{align_of, size_of};
use swoole_rs_corebase::log;
use swoole_rs_reactor::{
    Event, SocketAddress, SocketRecord, swr_v1_socket_bind, swr_v1_socket_get_name,
    swr_v1_socket_listen, swr_v1_socket_set_timeout, swr_v1_socket_timeout,
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
