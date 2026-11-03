//! `coroutine::Socket`'s record: its scalar fields in Rust's layout, initialised,
//! opened, accepted and released here. Native `Socket` derives from the mirror
//! (`native/hooks/swoole_rs_co_socket.h`) and keeps only what C++ owns (strings,
//! the proxies, the TLS context, the destructor callback); both sides assert the
//! offsets below. The `network::Socket` it wraps is the reactor's record.
use crate::shutdown::{
    SocketState, swr_v1_hook_socket_connect, swr_v1_hook_socket_state_free,
    swr_v1_hook_socket_state_new,
};
use core::ffi::{c_char, c_int, c_void};
use core::mem::{offset_of, size_of};
use core::ptr;
use swoole_rs_reactor::{SocketAddress, SocketRecord};

/// `swoole::Protocol`, whole (`split_by_eof` is a C++ `bool`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Protocol {
    pub split_by_eof: bool,
    pub package_eof: [u8; 8],
    pub package_eof_len: u8,
    /// A C `char`.
    pub package_length_type: u8,
    pub package_length_size: u8,
    pub package_length_offset: u16,
    pub package_body_offset: u16,
    pub package_max_length: u32,
    pub private_data_1: *mut c_void,
    pub private_data_2: *mut c_void,
    pub on_package: *mut c_void,
    pub get_package_length: *mut c_void,
    pub get_package_length_size: *mut c_void,
}

/// `coroutine::Socket`'s scalar fields. Native objects (`String`, `Allocator`,
/// `NameResolver::Context`) are opaque here.
#[repr(C)]
pub struct CoSocket {
    pub err_code: c_int,
    pub err_msg: *const c_char,
    pub open_length_check: bool,
    pub open_eof_check: bool,
    pub http2: bool,
    pub protocol: Protocol,
    /// `swSocketType`.
    pub kind: c_int,
    pub socket: *mut SocketRecord,
    pub sock_domain: c_int,
    pub sock_type: c_int,
    pub sock_protocol: c_int,
    pub sock_fd: c_int,
    pub connect_port: c_int,
    pub backlog: c_int,
    pub buffer_allocator: *const c_void,
    pub buffer_init_size: usize,
    pub read_buffer: *mut c_void,
    pub write_buffer: *mut c_void,
    pub ssl_is_server: bool,
    pub ssl_handshaked: bool,
    pub zero_copy: bool,
    pub resolve_context: *mut c_void,
    /// The coroutines, barriers and timers bound to each direction.
    pub state: *mut SocketState,
}

/// Field offsets native code asserts too (`swoole_rs_co_socket.h`).
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(
        size_of::<Protocol>() == 64
            && offset_of!(Protocol, package_max_length) == 16
            && offset_of!(Protocol, private_data_1) == 24,
        "swoole::Protocol's layout moved"
    );
    assert!(
        offset_of!(CoSocket, protocol) == 24
            && offset_of!(CoSocket, kind) == 88
            && offset_of!(CoSocket, socket) == 96
            && offset_of!(CoSocket, backlog) == 124
            && offset_of!(CoSocket, buffer_allocator) == 128
            && offset_of!(CoSocket, ssl_is_server) == 160
            && offset_of!(CoSocket, resolve_context) == 168
            && size_of::<CoSocket>() == 184,
        "coroutine::Socket's layout moved"
    );
};

/// `SW_FD_CO_SOCKET`, `SW_SOCK_RAW`, `SW_SOCK_NONBLOCK | SW_SOCK_CLOEXEC`,
/// `SW_BUFFER_SIZE_BIG` and `SW_INPUT_BUFFER_SIZE` (asserted in socket.cc).
const FD_CO_SOCKET: c_int = 6;
const SOCK_RAW: c_int = 7;
const OPEN_FLAGS: c_int = (1 << 2) | (1 << 3);
const BUFFER_SIZE_BIG: usize = 65536;
const INPUT_BUFFER_SIZE: u32 = 2 * 1024 * 1024;

/// How a constructor makes its `network::Socket` (`swr_v1_hook_co_socket_new`).
pub const NEW_BY_FAMILY: c_int = 0;
pub const NEW_BY_TYPE: c_int = 1;
pub const NEW_FD_BY_TYPE: c_int = 2;
pub const NEW_FD_BY_FAMILY: c_int = 3;

/// What a constructor did, for native code: `OPENED` set unless socket(2) failed
/// (errno holds why); `NODELAY_FAILED` when `TCP_NODELAY` could not be set (native
/// warns); bits 8 and up are `swr_v1_socket_fd_option`'s warnings from `set_nonblock`.
pub const OPENED: u32 = 1;
pub const NODELAY_FAILED: u32 = 2;
pub const FD_WARNINGS_SHIFT: u32 = 8;

impl CoSocket {
    fn new(state: *mut SocketState) -> Self {
        Self {
            err_code: 0,
            err_msg: c"".as_ptr(),
            open_length_check: false,
            open_eof_check: false,
            http2: false,
            // SAFETY: every field is an integer, a bool, a byte array or a raw pointer.
            protocol: unsafe { core::mem::zeroed() },
            kind: 0,
            socket: ptr::null_mut(),
            sock_domain: 0,
            sock_type: 0,
            sock_protocol: 0,
            sock_fd: -1,
            connect_port: 0,
            backlog: 0,
            buffer_allocator: ptr::null(),
            buffer_init_size: BUFFER_SIZE_BIG,
            read_buffer: ptr::null_mut(),
            write_buffer: ptr::null_mut(),
            ssl_is_server: false,
            ssl_handshaked: false,
            zero_copy: false,
            resolve_context: ptr::null_mut(),
            state,
        }
    }

    /// `init_sock`: a nonblocking, close-on-exec socket of the record's type.
    fn open(&mut self, owner: *mut c_void) -> bool {
        let socket = swoole_rs_reactor::swr_v1_socket_make_open(
            self.kind,
            FD_CO_SOCKET,
            self.sock_domain,
            self.sock_type,
            self.sock_protocol,
            OPEN_FLAGS,
        );
        if socket.is_null() {
            return false;
        }
        self.attach(socket, owner);
        true
    }

    /// The fields `init_sock` and `init_reactor_socket` set on a new socket.
    fn attach(&mut self, socket: *mut SocketRecord, owner: *mut c_void) {
        self.socket = socket;
        // SAFETY: a live record the caller just made.
        let record = unsafe { &mut *socket };
        self.sock_fd = record.fd;
        record.object = owner;
        record.info.kind = self.kind;
    }

    /// `init_reactor_socket`: wrap `fd`, which the caller made nonblocking.
    fn adopt(&mut self, fd: c_int, owner: *mut c_void) {
        let socket = swoole_rs_reactor::swr_v1_socket_make(fd, FD_CO_SOCKET);
        // SAFETY: the record just made.
        let record = unsafe { &mut *socket };
        record.socket_type = self.kind;
        record.output.nonblock = 1;
        record.cloexec = 1;
        self.attach(socket, owner);
    }

    /// `Socket::set_nonblock`: the `fd_option` warnings, shifted for the result.
    fn set_nonblock(&mut self) -> u32 {
        // SAFETY: the live record `adopt` made.
        let record = unsafe { &mut *self.socket };
        let result = swoole_rs_reactor::swr_v1_socket_fd_option(record.fd, 1, -1);
        if result.ok {
            record.output.nonblock = 1;
            record.cloexec = u8::MAX;
        }
        result.warnings << FD_WARNINGS_SHIFT
    }

    /// `init_options`: `TCP_NODELAY` for TCP, the default buffer size for UDP, and
    /// the default length protocol.
    fn init_options(&mut self) -> u32 {
        // SAFETY: the live record the constructor made.
        let record = unsafe { &*self.socket };
        let mut result = 0;
        if record.is_tcp() {
            let on: c_int = 1;
            // SAFETY: an int option of the size given.
            let set = unsafe {
                libc::setsockopt(
                    record.fd,
                    libc::IPPROTO_TCP,
                    libc::TCP_NODELAY,
                    (&raw const on).cast(),
                    socklen::<c_int>(),
                )
            };
            if set < 0 {
                result |= NODELAY_FAILED;
            }
        }
        if matches!(record.socket_type, 2 | 4) {
            // SAFETY: a plain read of the default, which only the owning thread assigns.
            let size =
                unsafe { (*ptr::addr_of!(swoole_rs_reactor::swr_v1_socket_defaults)).buffer_size };
            swoole_rs_reactor::swr_v1_socket_buffer_size(record.fd, true, true, size);
        }
        self.protocol.package_length_type = b'N';
        self.protocol.package_length_size = 4;
        self.protocol.package_length_offset = 0;
        self.protocol.package_body_offset = 0;
        self.protocol.package_max_length = INPUT_BUFFER_SIZE;
        result
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "option and address sizes fit socklen_t"
)]
const fn socklen<T>() -> libc::socklen_t {
    size_of::<T>() as libc::socklen_t
}

/// `domain` and `kind` for `socket_type`, left alone for a type with none.
fn domain_and_type(socket_type: c_int, domain: &mut c_int, kind: &mut c_int) {
    // SAFETY: two live ints.
    unsafe { swoole_rs_reactor::swr_v1_socket_domain_and_type(socket_type, domain, kind) };
}

/// A socket record's defaults (not open, no buffers, nothing bound), which every
/// constructor writes first.
///
/// # Safety
/// `socket` is writable storage for one record.
pub(crate) unsafe fn init(socket: *mut CoSocket) -> &'static mut CoSocket {
    // SAFETY: as the caller promises.
    unsafe { socket.write(CoSocket::new(swr_v1_hook_socket_state_new())) };
    // SAFETY: just initialised; the native object owns it for its lifetime.
    unsafe { &mut *socket }
}

/// Release what a constructor made. Native code frees the buffers
/// and the `network::Socket`.
///
/// # Safety
/// `socket` is a live record, not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_fini(socket: *mut CoSocket) {
    // SAFETY: as the caller promises.
    let socket = unsafe { &mut *socket };
    // SAFETY: the record's own handle, released once.
    unsafe { swr_v1_hook_socket_state_free(socket.state) };
    socket.state = ptr::null_mut();
}

/// The public constructors, which initialise the record first:
/// [`NEW_BY_FAMILY`] `Socket(domain, type, protocol)`, [`NEW_BY_TYPE`]
/// `Socket(SocketType)`, [`NEW_FD_BY_TYPE`] `Socket(fd, SocketType)` and
/// [`NEW_FD_BY_FAMILY`] `Socket(fd, domain, type, protocol)`. `owner` is the native
/// `Socket`, which the new `network::Socket` points back to. Returns the bits above.
///
/// # Safety
/// `socket` is writable storage for one record; `owner` is its native object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_new(
    socket: *mut CoSocket,
    owner: *mut c_void,
    how: c_int,
    fd: c_int,
    socket_type: c_int,
    domain: c_int,
    kind: c_int,
    protocol: c_int,
) -> u32 {
    // SAFETY: as the caller promises.
    let socket = unsafe { init(socket) };
    let mut result = OPENED;
    match how {
        NEW_BY_FAMILY | NEW_FD_BY_FAMILY => {
            socket.sock_domain = domain;
            socket.sock_type = kind;
            socket.sock_protocol = protocol;
            socket.kind = swoole_rs_reactor::swr_v1_socket_type(domain, kind);
        }
        _ => {
            socket.kind = socket_type;
            domain_and_type(socket_type, &mut socket.sock_domain, &mut socket.sock_type);
        }
    }
    if how == NEW_BY_FAMILY || how == NEW_BY_TYPE {
        if !socket.open(owner) {
            return 0;
        }
    } else {
        socket.adopt(fd, owner);
        if how == NEW_FD_BY_TYPE && socket_type == SOCK_RAW {
            return result;
        }
        result |= socket.set_nonblock();
    }
    result | socket.init_options()
}

/// `reinit_sock(type)`: a new socket of `socket_type` replaces the record's, whose
/// old `network::Socket` is written to `old` for native code to free. False, with
/// errno set and nothing changed, if socket(2) failed.
///
/// # Safety
/// `socket` is a live record with a `network::Socket`; `owner` is its native
/// object; `old` points to writable storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_reinit(
    socket: *mut CoSocket,
    owner: *mut c_void,
    socket_type: c_int,
    old: *mut *mut SocketRecord,
) -> bool {
    // SAFETY: as the caller promises.
    let socket = unsafe { &mut *socket };
    let (mut domain, mut kind) = (0, 0);
    domain_and_type(socket_type, &mut domain, &mut kind);
    let made = swoole_rs_reactor::swr_v1_socket_make_open(
        socket_type,
        FD_CO_SOCKET,
        domain,
        kind,
        socket.sock_protocol,
        OPEN_FLAGS,
    );
    if made.is_null() {
        return false;
    }
    // SAFETY: as the caller promises.
    unsafe { old.write(socket.socket) };
    // Upstream records the old type on the new socket before taking the new one.
    socket.attach(made, owner);
    socket.sock_domain = domain;
    socket.sock_type = kind;
    socket.kind = socket_type;
    true
}

/// The accepting constructor: `conn`, accepted on `server`, with the server's
/// type, framing and timeouts, connected. Native code shares the TLS context.
/// Returns [`swr_v1_hook_co_socket_new`]'s bits.
///
/// # Safety
/// `socket` is writable storage for one record, `server` a live one, `conn` a live accepted
/// `network::Socket` the record takes; `owner` is the record's native object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_accepted(
    socket: *mut CoSocket,
    owner: *mut c_void,
    conn: *mut SocketRecord,
    server: *const CoSocket,
) -> u32 {
    // SAFETY: as the caller promises.
    let socket = unsafe { init(socket) };
    // SAFETY: as the caller promises.
    let server = unsafe { &*server };
    socket.kind = server.kind;
    socket.sock_domain = server.sock_domain;
    socket.sock_type = server.sock_type;
    socket.sock_protocol = server.sock_protocol;
    socket.socket = conn;
    {
        // SAFETY: the live accepted record.
        let record = unsafe { &mut *conn };
        socket.sock_fd = record.fd;
        record.object = owner;
        record.socket_type = server.kind;
        record.fd_type = FD_CO_SOCKET;
    }
    let result = OPENED | socket.init_options();
    // SAFETY: the server's live record.
    let timeouts = unsafe { (*server.socket).timeouts };
    // SAFETY: the live accepted record.
    unsafe { (*conn).timeouts = timeouts };
    socket.open_length_check = server.open_length_check;
    socket.open_eof_check = server.open_eof_check;
    socket.http2 = server.http2;
    socket.protocol = server.protocol;
    // SAFETY: the record's own handle.
    unsafe { swr_v1_hook_socket_connect(socket.state) };
    socket.ssl_is_server = server.ssl_is_server;
    result
}

/// `pop_read_buffer` / `pop_write_buffer`: the buffer, which the caller now owns, or null.
///
/// # Safety
/// `socket` is a live record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_pop_buffer(
    socket: *mut CoSocket,
    write: bool,
) -> *mut c_void {
    // SAFETY: as the caller promises.
    let socket = unsafe { &mut *socket };
    let slot = if write {
        &mut socket.write_buffer
    } else {
        &mut socket.read_buffer
    };
    core::mem::replace(slot, ptr::null_mut())
}

/// `getpeername(sa)`: the peer's address and the record's type into `address`; 0,
/// or the errno to set.
///
/// # Safety
/// `socket` is a live record; `address` points to a writable address.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_peer_name(
    socket: *const CoSocket,
    address: *mut SocketAddress,
) -> c_int {
    // SAFETY: as the caller promises.
    let socket = unsafe { &*socket };
    // SAFETY: as the caller promises.
    let address = unsafe { &mut *address };
    address.len = socklen::<swoole_rs_reactor::AddressStorage>();
    // SAFETY: the address's own storage and length.
    let failed = unsafe {
        libc::getpeername(
            socket.sock_fd,
            (&raw mut address.addr).cast(),
            &raw mut address.len,
        )
    } != 0;
    if failed {
        return std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    }
    address.kind = socket.kind;
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::MaybeUninit;

    fn record() -> Box<CoSocket> {
        let mut storage = Box::new(MaybeUninit::<CoSocket>::uninit());
        unsafe { init(storage.as_mut_ptr()) };
        unsafe { Box::from_raw(Box::into_raw(storage).cast::<CoSocket>()) }
    }

    #[expect(
        clippy::boxed_local,
        reason = "the records live in boxes so their addresses stay put"
    )]
    fn release(mut socket: Box<CoSocket>) {
        if !socket.socket.is_null() {
            unsafe {
                libc::close((*socket.socket).fd);
                swoole_rs_reactor::swr_v1_socket_dealloc(socket.socket);
            }
        }
        unsafe { swr_v1_hook_co_socket_fini(&raw mut *socket) };
        assert!(socket.state.is_null());
    }

    fn new(socket: &mut CoSocket, how: c_int, fd: c_int, ty: c_int, d: c_int, k: c_int) -> u32 {
        let owner = (&raw mut *socket).cast();
        // The constructor writes the record again.
        unsafe { swr_v1_hook_co_socket_fini(socket) };
        unsafe { swr_v1_hook_co_socket_new(socket, owner, how, fd, ty, d, k, 0) }
    }

    fn option(fd: c_int, level: c_int, name: c_int) -> c_int {
        let mut value: c_int = 0;
        let mut length = socklen::<c_int>();
        let got =
            unsafe { libc::getsockopt(fd, level, name, (&raw mut value).cast(), &raw mut length) };
        assert_eq!(got, 0);
        value
    }

    #[test]
    fn defaults_are_upstreams_member_initialisers() {
        let socket = record();
        assert_eq!(socket.sock_fd, -1);
        assert_eq!(socket.buffer_init_size, 65536);
        assert_eq!(unsafe { *socket.err_msg }, 0);
        assert!(!socket.state.is_null());
        assert_eq!(socket.protocol.package_length_type, 0);
        release(socket);
    }

    #[test]
    fn a_tcp_socket_by_type_is_open_nonblocking_with_nodelay_and_the_length_protocol() {
        let mut socket = record();
        assert_eq!(new(&mut socket, NEW_BY_TYPE, 0, 1, 0, 0), OPENED);
        assert_eq!(
            (socket.sock_domain, socket.sock_type),
            (libc::AF_INET, libc::SOCK_STREAM)
        );
        let fd = socket.sock_fd;
        let record = unsafe { &*socket.socket };
        assert_eq!(
            (
                record.fd,
                record.fd_type,
                record.socket_type,
                record.info.kind
            ),
            (fd, 6, 1, 1)
        );
        assert_eq!(record.object, (&raw const *socket).cast_mut().cast());
        assert_eq!(record.output.nonblock, 1);
        assert_ne!(
            unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_NONBLOCK,
            0
        );
        assert_ne!(option(fd, libc::IPPROTO_TCP, libc::TCP_NODELAY), 0);
        assert_eq!(socket.protocol.package_length_type, b'N');
        assert_eq!(socket.protocol.package_length_size, 4);
        assert_eq!(socket.protocol.package_max_length, 2 * 1024 * 1024);
        release(socket);
    }

    #[test]
    fn a_family_maps_to_its_type() {
        let mut socket = record();
        assert_eq!(
            new(
                &mut socket,
                NEW_BY_FAMILY,
                0,
                0,
                libc::AF_INET,
                libc::SOCK_DGRAM
            ),
            OPENED
        );
        assert_eq!(socket.kind, 2);
        assert_eq!(unsafe { (*socket.socket).socket_type }, 2);
        release(socket);
    }

    #[test]
    fn an_unknown_type_fails_to_open() {
        let mut socket = record();
        assert_eq!(new(&mut socket, NEW_BY_TYPE, 0, 99, 0, 0), 0);
        assert!(socket.socket.is_null());
        assert_eq!(socket.sock_fd, -1);
        release(socket);
    }

    #[test]
    fn an_adopted_fd_is_made_nonblocking_but_a_raw_one_is_not() {
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        let mut socket = record();
        assert_eq!(
            new(
                &mut socket,
                NEW_FD_BY_FAMILY,
                fd,
                0,
                libc::AF_INET,
                libc::SOCK_STREAM
            ),
            OPENED
        );
        assert_eq!(socket.sock_fd, fd);
        assert_ne!(
            unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_NONBLOCK,
            0
        );
        assert_eq!(unsafe { (*socket.socket).fd_type }, 6);
        release(socket);

        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        let mut socket = record();
        assert_eq!(new(&mut socket, NEW_FD_BY_TYPE, fd, SOCK_RAW, 0, 0), OPENED);
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_NONBLOCK,
            0
        );
        assert_eq!(socket.protocol.package_length_type, 0);
        release(socket);
    }

    #[test]
    fn reinit_swaps_the_socket_and_hands_back_the_old_one() {
        let mut socket = record();
        new(&mut socket, NEW_BY_TYPE, 0, 1, 0, 0);
        let first = socket.socket;
        let mut old = ptr::null_mut();
        let owner = (&raw mut *socket).cast();
        assert!(unsafe { swr_v1_hook_co_socket_reinit(&raw mut *socket, owner, 3, &raw mut old) });
        assert_eq!(old, first);
        assert_eq!((socket.kind, socket.sock_domain), (3, libc::AF_INET6));
        assert_eq!(unsafe { (*socket.socket).info.kind }, 1);
        unsafe {
            libc::close((*old).fd);
            swoole_rs_reactor::swr_v1_socket_dealloc(old);
        }
        release(socket);
    }

    #[test]
    fn an_accepted_socket_inherits_the_servers_settings() {
        let mut server = record();
        new(&mut server, NEW_BY_TYPE, 0, 1, 0, 0);
        server.open_eof_check = true;
        server.http2 = true;
        server.ssl_is_server = true;
        server.protocol.package_eof_len = 2;
        unsafe { (*server.socket).timeouts.read = 1.5 };
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        let conn = swoole_rs_reactor::swr_v1_socket_make(fd, 0);
        let mut socket = record();
        let owner = (&raw mut *socket).cast();
        unsafe { swr_v1_hook_co_socket_fini(&raw mut *socket) };
        let bits = unsafe {
            swr_v1_hook_co_socket_accepted(&raw mut *socket, owner, conn, &raw const *server)
        };
        assert_eq!(bits, OPENED);
        assert_eq!((socket.kind, socket.sock_fd), (1, fd));
        assert!(socket.open_eof_check && socket.http2 && socket.ssl_is_server);
        assert_eq!(socket.protocol.package_eof_len, 2);
        let record = unsafe { &*conn };
        assert_eq!((record.fd_type, record.socket_type), (6, 1));
        assert!((record.timeouts.read - 1.5).abs() < f64::EPSILON);
        assert!(unsafe { crate::shutdown::swr_v1_hook_socket_flags(socket.state) }.connected);
        release(socket);
        release(server);
    }

    #[test]
    fn popping_a_buffer_takes_it() {
        let mut socket = record();
        let mut marker = 0u8;
        socket.read_buffer = (&raw mut marker).cast();
        let popped = unsafe { swr_v1_hook_co_socket_pop_buffer(&raw mut *socket, false) };
        assert_eq!(popped, (&raw mut marker).cast());
        assert!(socket.read_buffer.is_null());
        assert!(unsafe { swr_v1_hook_co_socket_pop_buffer(&raw mut *socket, true) }.is_null());
        release(socket);
    }

    #[test]
    fn peer_name_of_an_unconnected_socket_is_an_error() {
        let mut socket = record();
        new(&mut socket, NEW_BY_TYPE, 0, 1, 0, 0);
        let mut address: SocketAddress = unsafe { core::mem::zeroed() };
        let error =
            unsafe { swr_v1_hook_co_socket_peer_name(&raw const *socket, &raw mut address) };
        assert_eq!(error, libc::ENOTCONN);
        assert_eq!(address.kind, 0);
        release(socket);
    }
}
