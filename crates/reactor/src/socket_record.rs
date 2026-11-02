//! `network::Socket`'s record: every field, in Rust's layout, allocated, initialised,
//! duplicated and freed here. Native `Socket` derives from its mirror
//! (`native/reactor/swoole_rs_socket.h`) and adds only methods; both sides assert the
//! offsets below. The upstream 1-bit flags are bytes here, which keeps every value
//! the code assigns.
use crate::SocketOutput;
use crate::socket_timeouts::SocketTimeouts;
use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use std::ffi::{c_int, c_void};
use std::mem::{offset_of, size_of};
use std::ptr;

/// `network::Socket`'s TLS state group, in the layout `swoole_rs_tls` works on.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SocketTlsState {
    pub state: u32,
    pub want_read: u8,
    pub want_write: u8,
}

/// `network::Address`'s storage: the largest of the address families it holds.
#[repr(C)]
#[derive(Clone, Copy)]
pub union AddressStorage {
    pub ss: libc::sockaddr,
    pub inet_v4: libc::sockaddr_in,
    pub inet_v6: libc::sockaddr_in6,
    pub un: libc::sockaddr_un,
}

/// `network::Address`: the storage, its length and its `swSocketType`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SocketAddress {
    pub addr: AddressStorage,
    pub len: libc::socklen_t,
    pub kind: c_int,
}

/// `network::Socket`'s fields. Pointers to native objects (`Buffer`, `TimerNode`,
/// `String`, `SSL`, the owner) are opaque here.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SocketRecord {
    pub fd: c_int,
    /// `swFdType`.
    pub fd_type: c_int,
    /// `swSocketType`.
    pub socket_type: c_int,
    pub events: c_int,
    pub enable_tcp_nodelay: bool,
    pub kernel_nobufs: bool,
    pub removed: u8,
    pub silent_remove: u8,
    pub cloexec: u8,
    pub direct_send: u8,
    pub bound: u8,
    pub listened: u8,
    pub ssl_renegotiation: u8,
    pub ssl_handshake_buffer_set: u8,
    pub ssl_quiet_shutdown: u8,
    pub ssl_closed: u8,
    pub dtls: u8,
    pub tcp_nopush: u8,
    pub tcp_nodelay: u8,
    pub skip_recv: u8,
    pub recv_wait: u8,
    pub event_hup: u8,
    pub dont_restart: u8,
    pub close_notified: u8,
    pub chunk_size: u32,
    pub output: SocketOutput,
    pub object: *mut c_void,
    pub ssl: *mut c_void,
    pub tls: SocketTlsState,
    pub timeouts: SocketTimeouts,
    pub last_received_time: f64,
    pub last_sent_time: f64,
    pub in_buffer: *mut c_void,
    pub recv_buffer: *mut c_void,
    pub recv_timer: *mut c_void,
    pub total_recv_bytes: usize,
    pub total_send_bytes: usize,
    /// Last, so every offset before it is the same on each LP64 target.
    pub info: SocketAddress,
}

/// `Socket::default_*_timeout` and `Socket::default_buffer_size`, which native code
/// reads and assigns by name and every new record copies.
#[repr(C)]
pub struct SocketDefaults {
    pub timeouts: SocketTimeouts,
    pub buffer_size: u32,
}

/// `SW_SOCKET_DEFAULT_*_TIMEOUT` and `SW_SOCKET_BUFFER_SIZE`.
#[unsafe(no_mangle)]
pub static mut swr_v1_socket_defaults: SocketDefaults = SocketDefaults {
    timeouts: SocketTimeouts {
        dns: 60.0,
        connect: 10.0,
        read: 60.0,
        write: 60.0,
    },
    buffer_size: 8_388_608,
};

/// Field offsets native code asserts too (`swoole_rs_socket.h`).
#[cfg(target_pointer_width = "64")]
const OFFSETS: [(usize, usize); 15] = [
    (offset_of!(SocketRecord, events), 12),
    (offset_of!(SocketRecord, enable_tcp_nodelay), 16),
    (offset_of!(SocketRecord, removed), 18),
    (offset_of!(SocketRecord, close_notified), 35),
    (offset_of!(SocketRecord, chunk_size), 36),
    (offset_of!(SocketRecord, output), 40),
    (offset_of!(SocketRecord, object), 64),
    (offset_of!(SocketRecord, ssl), 72),
    (offset_of!(SocketRecord, tls), 80),
    (offset_of!(SocketRecord, timeouts), 88),
    (offset_of!(SocketRecord, last_received_time), 120),
    (offset_of!(SocketRecord, in_buffer), 136),
    (offset_of!(SocketRecord, recv_timer), 152),
    (offset_of!(SocketRecord, total_send_bytes), 168),
    (offset_of!(SocketRecord, info), 176),
];

#[cfg(target_pointer_width = "64")]
const _: () = {
    let mut i = 0;
    while i < OFFSETS.len() {
        assert!(
            OFFSETS[i].0 == OFFSETS[i].1,
            "network::Socket's layout moved"
        );
        i += 1;
    }
    #[cfg(target_os = "linux")]
    assert!(
        size_of::<SocketAddress>() == 120,
        "network::Address's size moved"
    );
    #[cfg(target_os = "macos")]
    assert!(
        size_of::<SocketAddress>() == 116,
        "network::Address's size moved"
    );
    assert!(
        size_of::<SocketRecord>() == (176 + size_of::<SocketAddress>()).next_multiple_of(8),
        "network::Socket's size moved"
    );
};

impl SocketRecord {
    /// A socket as `new Socket()` makes it: all zero but the default timeouts.
    #[must_use]
    pub fn new() -> Self {
        // SAFETY: every field is an integer, a float, a raw pointer or a union of
        // C address structs, all of which are valid zeroed.
        let mut socket: Self = unsafe { std::mem::zeroed() };
        // SAFETY: a plain read of the defaults, which only the owning thread assigns.
        socket.timeouts = unsafe { (*ptr::addr_of!(swr_v1_socket_defaults)).timeouts };
        socket
    }

    /// `make_socket(fd, fd_type)`'s fields.
    #[must_use]
    pub fn with_fd(fd: c_int, fd_type: c_int) -> Self {
        let mut socket = Self::new();
        socket.fd = fd;
        socket.fd_type = fd_type;
        socket.removed = 1;
        socket
    }

    fn into_raw(self) -> *mut Self {
        let socket = swr_v1_socket_alloc();
        // SAFETY: fresh, suitably aligned storage for one record.
        unsafe { socket.write(self) };
        socket
    }
}

impl Default for SocketRecord {
    fn default() -> Self {
        Self::new()
    }
}

/// `Socket::operator new`: storage for one record, uninitialised (the constructor
/// calls [`swr_v1_socket_init`]). Aborts if allocation fails, as `new` throws.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_socket_alloc() -> *mut SocketRecord {
    let layout = Layout::new::<SocketRecord>();
    // SAFETY: the layout has a nonzero size.
    let storage = unsafe { alloc(layout) };
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "alloc returns storage aligned for the layout it was given"
    )]
    let socket = storage.cast::<SocketRecord>();
    if socket.is_null() {
        handle_alloc_error(layout);
    }
    socket
}

/// `Socket::operator delete`.
///
/// # Safety
/// `socket` is null or came from [`swr_v1_socket_alloc`] (or one of the calls here
/// that return a record) and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_dealloc(socket: *mut SocketRecord) {
    if !socket.is_null() {
        // SAFETY: as the caller promises; the record has no destructor.
        unsafe { dealloc(socket.cast(), Layout::new::<SocketRecord>()) };
    }
}

/// `Socket()`: the record `new Socket()` value-initialises.
///
/// # Safety
/// `socket` is writable storage for one record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_init(socket: *mut SocketRecord) {
    // SAFETY: as the caller promises.
    unsafe { socket.write(SocketRecord::new()) };
}

/// `make_socket(fd, fd_type)`: a new record that owns `fd`, not yet registered.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_socket_make(fd: c_int, fd_type: c_int) -> *mut SocketRecord {
    SocketRecord::with_fd(fd, fd_type).into_raw()
}

/// `make_socket(type, fd_type, domain, type, protocol, flags)`: socket(2) through
/// `swoole::socket`, then a record of `socket_type` with the nonblocking and
/// close-on-exec flags `flags` asked for. Null, with errno set, if socket(2) failed.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_socket_make_open(
    socket_type: c_int,
    fd_type: c_int,
    domain: c_int,
    kind: c_int,
    protocol: c_int,
    flags: c_int,
) -> *mut SocketRecord {
    let fd = crate::swr_v1_socket_open(domain, kind, protocol, flags);
    if fd < 0 {
        return ptr::null_mut();
    }
    let mut socket = SocketRecord::with_fd(fd, fd_type);
    socket.output.nonblock = u8::from(flags & crate::socket_types::NONBLOCK != 0);
    socket.cloexec = u8::from(flags & crate::socket_types::CLOEXEC != 0);
    socket.socket_type = socket_type;
    socket.into_raw()
}

/// `make_socket(type, fd_type, flags)`: [`swr_v1_socket_make_open`] with the domain
/// and type `socket_type` maps to. For a type with none, null with errno
/// `ESOCKTNOSUPPORT` and `*unknown` set, so native code warns.
///
/// # Safety
/// `unknown` points to a writable bool.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_make_typed(
    socket_type: c_int,
    fd_type: c_int,
    flags: c_int,
    unknown: *mut bool,
) -> *mut SocketRecord {
    let (mut domain, mut kind) = (0, 0);
    // SAFETY: two local ints.
    if !unsafe { crate::swr_v1_socket_domain_and_type(socket_type, &raw mut domain, &raw mut kind) }
    {
        // SAFETY: as the caller promises.
        unsafe { unknown.write(true) };
        crate::socket_sync::set_errno(libc::ESOCKTNOSUPPORT);
        return ptr::null_mut();
    }
    swr_v1_socket_make_open(socket_type, fd_type, domain, kind, 0, flags)
}

/// `Socket::dup`: a copy of every field, owning `dup(fd)`.
///
/// # Safety
/// `socket` is a live record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_dup(socket: *const SocketRecord) -> *mut SocketRecord {
    // SAFETY: as the caller promises.
    let mut copy = unsafe { *socket };
    // SAFETY: dup(2) has no memory preconditions.
    copy.fd = unsafe { libc::dup(copy.fd) };
    copy.into_raw()
}

/// `Socket::accept`: a connection accepted on `listener`, as a new unregistered record
/// that inherits its type and nonblocking flag, or null (errno set). Without accept4
/// the listener is marked close-on-exec, as upstream's `set_fd_option` call does.
///
/// # Safety
/// `listener` is a live record no one else is using.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_accept_new(
    listener: *mut SocketRecord,
) -> *mut SocketRecord {
    // SAFETY: as the caller promises.
    let listener = unsafe { &mut *listener };
    let mut socket = SocketRecord::new();
    socket.removed = 1;
    socket.socket_type = listener.socket_type;
    // Upstream passes sizeof(Address), not sizeof(Address::addr).
    #[expect(
        clippy::cast_possible_truncation,
        reason = "sizeof(network::Address) fits socklen_t"
    )]
    let capacity = size_of::<SocketAddress>() as libc::socklen_t;
    socket.info.len = capacity;
    let mut listener_cloexec = false;
    // SAFETY: the new record's address storage, and a local bool.
    socket.fd = unsafe {
        crate::swr_v1_socket_accept(
            listener.fd,
            ptr::addr_of_mut!(socket.info.addr).cast(),
            &raw mut socket.info.len,
            listener.output.nonblock != 0,
            &raw mut listener_cloexec,
        )
    };
    if listener_cloexec {
        listener.cloexec = 1;
    }
    if socket.fd < 0 {
        return ptr::null_mut();
    }
    socket.info.kind = listener.socket_type;
    socket.output.nonblock = listener.output.nonblock;
    socket.cloexec = 1;
    socket.into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_records_carry_the_default_timeouts() {
        let socket = SocketRecord::new();
        assert_eq!(socket.timeouts.connect, 10.0);
        assert_eq!(socket.timeouts.read, 60.0);
        assert_eq!(socket.fd, 0);
        assert!(socket.object.is_null() && socket.output.out_buffer.is_null());
    }

    #[test]
    fn make_owns_the_fd_unregistered() {
        let socket = swr_v1_socket_make(7, 3);
        // SAFETY: the record just made.
        let record = unsafe { &*socket };
        assert_eq!((record.fd, record.fd_type, record.removed), (7, 3, 1));
        // SAFETY: as above, not used again.
        unsafe { swr_v1_socket_dealloc(socket) };
    }

    #[test]
    fn unknown_types_fail_with_esocktnosupport() {
        let mut unknown = false;
        // SAFETY: a local bool.
        let socket = unsafe { swr_v1_socket_make_typed(99, 0, 0, &raw mut unknown) };
        assert!(socket.is_null() && unknown);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESOCKTNOSUPPORT)
        );
    }

    #[test]
    fn typed_sockets_record_their_flags() {
        let mut unknown = false;
        // SAFETY: a local bool. SW_SOCK_TCP, nonblocking and close-on-exec.
        let socket = unsafe { swr_v1_socket_make_typed(1, 5, 0b1100, &raw mut unknown) };
        assert!(!socket.is_null() && !unknown);
        // SAFETY: the record just made.
        let record = unsafe { &*socket };
        assert_eq!((record.socket_type, record.fd_type), (1, 5));
        assert_eq!(
            (record.output.nonblock, record.cloexec, record.removed),
            (1, 1, 1)
        );
        // SAFETY: the socket's own fd.
        unsafe { libc::close(record.fd) };
        // SAFETY: not used again.
        unsafe { swr_v1_socket_dealloc(socket) };
    }

    #[test]
    fn dup_copies_fields_and_owns_a_new_fd() {
        let mut fds = [0; 2];
        // SAFETY: a two-int array.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let mut original = SocketRecord::with_fd(fds[0], 2);
        original.chunk_size = 42;
        // SAFETY: a live local record.
        let copy = unsafe { swr_v1_socket_dup(&raw const original) };
        // SAFETY: the record just made.
        let record = unsafe { &*copy };
        assert_eq!(record.chunk_size, 42);
        assert!(record.fd >= 0 && record.fd != fds[0]);
        for fd in [record.fd, fds[0], fds[1]] {
            // SAFETY: fds this test opened.
            unsafe { libc::close(fd) };
        }
        // SAFETY: not used again.
        unsafe { swr_v1_socket_dealloc(copy) };
    }

    #[test]
    fn accept_fills_a_new_record_from_the_listener() {
        let (mut unknown, flags) = (false, 0b1100);
        // SAFETY: a local bool; SW_SOCK_UNIX_STREAM.
        let listener = unsafe { swr_v1_socket_make_typed(5, 1, flags, &raw mut unknown) };
        // SAFETY: the record just made.
        let record = unsafe { &mut *listener };
        // An unbound, unlistened socket: accept fails, and no record is made.
        // SAFETY: the live listener.
        assert!(unsafe { swr_v1_socket_accept_new(record) }.is_null());
        // SAFETY: the listener's own fd.
        unsafe { libc::close(record.fd) };
        // SAFETY: not used again.
        unsafe { swr_v1_socket_dealloc(listener) };
    }
}
