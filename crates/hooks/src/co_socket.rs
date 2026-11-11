//! `coroutine::Socket`'s record, which is the whole socket: made, opened, accepted,
//! released and freed here. Native `Socket` derives from the mirror
//! (`native/hooks/swoole_rs_co_socket.h`) and adds no fields; both sides assert the
//! offsets below. The `network::Socket` it wraps is the reactor's record, and the
//! strings, proxies, TLS context and destructor callback are its
//! [`SocketExtra`](crate::socket_extra::SocketExtra).
use crate::shutdown::SocketState;
use crate::socket_extra::SocketExtra;
use core::ffi::{c_char, c_int, c_void};
use core::mem::{offset_of, size_of};
use core::ptr;
use swoole_rs_buffer::string::{Allocator, CppString, delete_string, new_sized_string};
use swoole_rs_corebase::constants::{
    FdType, SW_BUFFER_SIZE_BIG, SW_INPUT_BUFFER_SIZE, SocketFlag, SocketType,
};
use swoole_rs_reactor::{Socket, SocketAddress, SocketFields, SocketRecord};
use swoole_rs_tls::upstream::{socket_free, socket_ssl_close};

pub use swoole_rs_corebase::net_types::Protocol;

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
    /// The record's metadata, which `init` makes and `fini` releases.
    pub extra: *mut crate::socket_extra::SocketExtra,
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
            && size_of::<CoSocket>() == 192,
        "coroutine::Socket's layout moved"
    );
};

const FD_CO_SOCKET: c_int = FdType::SW_FD_CO_SOCKET as c_int;
const SOCK_RAW: c_int = SocketType::SW_SOCK_RAW as c_int;
const OPEN_FLAGS: c_int =
    SocketFlag::SW_SOCK_NONBLOCK as c_int | SocketFlag::SW_SOCK_CLOEXEC as c_int;
const BUFFER_SIZE_BIG: usize = SW_BUFFER_SIZE_BIG as usize;

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

/// A live coroutine socket record: the only way to reach one's fields. The handle is
/// `Copy`, and a coroutine parked in one member shares the record with another
/// running a second, so each read and write goes through the pointer to that one
/// field and no reference to the whole record outlives a call.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Co(*mut CoSocket);

macro_rules! fields {
    ($($(#[$doc:meta])* $field:ident, $set:ident: $ty:ty;)*) => {$(
        $(#[$doc])*
        #[must_use]
        pub fn $field(self) -> $ty {
            // SAFETY: `new`'s contract: a live record; only the field is read.
            unsafe { (*self.0).$field }
        }
        $(#[$doc])*
        pub fn $set(self, value: $ty) {
            // SAFETY: `new`'s contract: a live record; only the field is written.
            unsafe { (*self.0).$field = value };
        }
    )*};
}

impl Co {
    /// The record at `raw`.
    ///
    /// # Safety
    /// `raw` is a live record (a native `Socket` is one) for as long as the handle or
    /// a copy is used; its `socket`, when not null, is a live `network::Socket`, and
    /// its `state` and `extra` are live.
    #[must_use]
    pub const unsafe fn new(raw: *mut CoSocket) -> Self {
        Self(raw)
    }

    /// The record the native `Socket` at `this` is.
    ///
    /// # Safety
    /// As [`Co::new`].
    #[must_use]
    pub const unsafe fn of(this: *const c_void) -> Self {
        Self(this.cast_mut().cast())
    }

    #[must_use]
    pub const fn raw(self) -> *mut CoSocket {
        self.0
    }

    /// The native object, which is the record.
    #[must_use]
    pub const fn this(self) -> *mut c_void {
        self.0.cast()
    }

    fields! {
        err_code, set_err_code: c_int;
        err_msg, set_err_msg: *const c_char;
        open_length_check, set_open_length_check: bool;
        open_eof_check, set_open_eof_check: bool;
        http2, set_http2: bool;
        kind, set_kind: c_int;
        /// The `network::Socket`, null before the socket opens.
        socket, set_socket: *mut SocketRecord;
        sock_domain, set_sock_domain: c_int;
        sock_type, set_sock_type: c_int;
        sock_protocol, set_sock_protocol: c_int;
        sock_fd, set_sock_fd: c_int;
        connect_port, set_connect_port: c_int;
        backlog, set_backlog: c_int;
        read_buffer, set_read_buffer: *mut c_void;
        write_buffer, set_write_buffer: *mut c_void;
        ssl_is_server, set_ssl_is_server: bool;
        ssl_handshaked, set_ssl_handshaked: bool;
        zero_copy, set_zero_copy: bool;
        resolve_context, set_resolve_context: *mut c_void;
        state, set_state: *mut SocketState;
        extra, set_extra: *mut crate::socket_extra::SocketExtra;
    }

    /// `is_closed`.
    #[must_use]
    pub fn is_closed(self) -> bool {
        self.sock_fd() == -1
    }

    /// `get_read_buffer()` / `get_write_buffer()`: the buffer, made on first use.
    #[must_use]
    pub fn buffer(self, write: bool) -> *mut CppString {
        let slot = if write {
            self.write_buffer()
        } else {
            self.read_buffer()
        };
        if !slot.is_null() {
            return slot.cast();
        }
        // SAFETY: `new`'s contract: a live record, whose allocator is null or live.
        let allocator = unsafe { (*self.0).buffer_allocator }.cast::<Allocator>();
        // SAFETY: as above.
        let made = new_sized_string(BUFFER_SIZE_BIG, unsafe { allocator.as_ref() }).cast();
        if write {
            self.set_write_buffer(made);
        } else {
            self.set_read_buffer(made);
        }
        made.cast()
    }

    /// `f` over [`Co::buffer`]'s String. `f` is immediate.
    pub fn with_buffer<T>(self, write: bool, f: impl FnOnce(&mut CppString) -> T) -> T {
        // SAFETY: the record's live String, borrowed only for the call.
        f(unsafe { &mut *self.buffer(write) })
    }

    /// `f` over the `network::Socket`, or `None` before the socket opens. `f` is
    /// immediate: it neither yields nor reaches the record through another handle.
    pub fn net<T>(self, f: impl FnOnce(&mut SocketFields) -> T) -> Option<T> {
        // SAFETY: `new`'s contract: null or a live network::Socket, borrowed only
        // for the immediate call.
        unsafe { self.socket().as_mut() }.map(|record| f(record))
    }

    /// `f` over the `network::Socket`'s handle, or `None` before the socket opens. The
    /// handle lives only for the call, so it cannot outlive the record.
    pub fn with_network<T>(self, f: impl FnOnce(Socket<'_>) -> T) -> Option<T> {
        // SAFETY: `new`'s contract: null or a live network::Socket, for the call.
        unsafe { Socket::from_ptr(self.socket()) }.map(f)
    }

    /// `f` over the whole record. `f` is immediate: it neither yields nor reaches the
    /// record through another handle.
    pub fn with<T>(self, f: impl FnOnce(&mut CoSocket) -> T) -> T {
        // SAFETY: `new`'s contract: a live record, borrowed only for the call.
        f(unsafe { &mut *self.0 })
    }

    /// The record's metadata, `None` once released. `f` is immediate.
    pub fn with_extra<T>(self, f: impl FnOnce(&mut SocketExtra) -> T) -> Option<T> {
        // SAFETY: `new`'s contract: null or the live metadata, borrowed only for the call.
        unsafe { self.extra().as_mut() }.map(f)
    }

    /// The bound coroutines, barriers and timers. `f` is immediate.
    pub fn with_state<T>(self, f: impl FnOnce(&mut SocketState) -> T) -> T {
        // SAFETY: `new`'s contract: the live state, borrowed only for the call.
        f(unsafe { &mut *self.state() })
    }

    /// `errno` classified by the open socket: `SW_WAIT` to wait, as
    /// `catch_read_error`/`catch_write_error` say.
    #[must_use]
    pub fn catch_error(self, error: c_int) -> c_int {
        self.net(|record| record.catch_error(error)).unwrap_or(0)
    }

    /// The socket's timeout of `kind`, or -1 when it has none.
    #[must_use]
    pub fn timeout(self, kind: c_int) -> f64 {
        self.net(|record| record.timeouts.get(kind))
            .flatten()
            .unwrap_or(-1.0)
    }

    /// Sets the socket's timeouts selected by `kind`.
    pub fn set_timeout(self, timeout: f64, kind: c_int) {
        self.net(|record| record.timeouts.set(timeout, kind));
    }
}

impl CoSocket {
    /// `is_closed`.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.sock_fd == -1
    }

    /// The `network::Socket` record.
    ///
    /// # Safety
    /// The socket is open (constructed with a `network::Socket`) and no other
    /// reference to that record is live.
    #[cfg(test)]
    #[must_use]
    #[expect(
        clippy::mut_from_ref,
        reason = "the network::Socket record is its own allocation, which this record only points to"
    )]
    pub unsafe fn record(&self) -> &mut SocketFields {
        // SAFETY: as the caller promises.
        unsafe { &mut (*self.socket).fields }
    }

    /// The socket's proxies' owners: SOCKS5, then HTTP. Each stays put until a
    /// setter replaces it or the socket goes.
    #[must_use]
    pub fn proxies(&self) -> [Option<&swoole_rs_corebase::ProxyState>; 2] {
        // SAFETY: the record's live metadata.
        let owners = unsafe { (*self.extra).proxies };
        // SAFETY: each null or a live owner.
        owners.map(|owner| unsafe { owner.as_ref() })
    }

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
            extra: SocketExtra::alloc(),
        }
    }

    /// `init_sock`: a nonblocking, close-on-exec socket of the record's type.
    fn open(&mut self, owner: *mut c_void) -> Option<Socket<'static>> {
        let socket = SocketRecord::open(
            self.kind,
            FD_CO_SOCKET,
            self.sock_domain,
            self.sock_type,
            self.sock_protocol,
            OPEN_FLAGS,
        );
        self.attach(socket, owner)
    }

    /// The fields `init_sock` and `init_reactor_socket` set on a new socket; `None`,
    /// with the record left alone, for null.
    fn attach(&mut self, socket: *mut SocketRecord, owner: *mut c_void) -> Option<Socket<'static>> {
        // SAFETY: null or a live record the caller just made, which this record owns now.
        let network = unsafe { Socket::from_ptr(socket) }?;
        self.socket = socket;
        self.sock_fd = network.edit(|record| {
            record.object = owner;
            record.info.kind = self.kind;
            record.fd
        });
        Some(network)
    }

    /// `init_reactor_socket`: wrap `fd`, which the caller made nonblocking.
    fn adopt(&mut self, fd: c_int, owner: *mut c_void) -> Option<Socket<'static>> {
        let mut record = SocketRecord::with_fd(fd, FD_CO_SOCKET);
        record.socket_type = self.kind;
        record.output.nonblock = 1;
        record.cloexec = 1;
        self.attach(record.into_raw(), owner)
    }

    /// `init_options`: `TCP_NODELAY` for TCP, the default buffer size for UDP, and
    /// the default length protocol.
    fn init_options(&mut self, network: Socket<'_>) -> u32 {
        let (tcp, fd, socket_type) =
            network.read(|record| (record.is_tcp(), record.fd, record.socket_type));
        let mut result = 0;
        if tcp {
            let on: c_int = 1;
            // SAFETY: an int option of the size given.
            let set = unsafe {
                libc::setsockopt(
                    fd,
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
        if matches!(socket_type, 2 | 4) {
            // SAFETY: a plain read of the default, which only the owning thread assigns.
            let size =
                unsafe { (*ptr::addr_of!(swoole_rs_reactor::swr_v1_socket_defaults)).buffer_size };
            swoole_rs_reactor::swr_v1_socket_buffer_size(fd, true, true, size);
        }
        self.protocol.package_length_type = b'N';
        self.protocol.package_length_size = 4;
        self.protocol.package_length_offset = 0;
        self.protocol.package_body_offset = 0;
        self.protocol.package_max_length = SW_INPUT_BUFFER_SIZE;
        result
    }
}

/// `Socket::set_nonblock`: the `fd_option` warnings, shifted for the result.
fn set_nonblock(network: Socket<'_>) -> u32 {
    let result = swoole_rs_reactor::swr_v1_socket_fd_option(network.fd(), 1, -1);
    if result.ok {
        network.edit(|record| {
            record.output.nonblock = 1;
            record.cloexec = u8::MAX;
        });
    }
    result.warnings << FD_WARNINGS_SHIFT
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
    swoole_rs_reactor::swr_v1_socket_domain_and_type(socket_type, domain, kind);
}

/// A socket record's defaults (not open, no buffers, nothing bound), which every
/// constructor writes first.
///
/// # Safety
/// `socket` is writable storage for one record.
pub(crate) unsafe fn init(socket: *mut CoSocket) -> &'static mut CoSocket {
    // SAFETY: as the caller promises.
    unsafe { socket.write(CoSocket::new(SocketState::alloc())) };
    // SAFETY: just initialised; the native object owns it for its lifetime.
    unsafe { &mut *socket }
}

/// Release the bound state and the metadata a constructor made.
///
/// # Safety
/// `socket` is a live record, not used afterwards.
pub unsafe fn swr_v1_hook_co_socket_fini(socket: *mut CoSocket) {
    // SAFETY: as the caller promises.
    let socket = unsafe { &mut *socket };
    // SAFETY: the record's own state, released once.
    unsafe { SocketState::free(socket.state) };
    socket.state = ptr::null_mut();
    // SAFETY: the record's own metadata, released once.
    unsafe { SocketExtra::free(socket.extra) };
    socket.extra = ptr::null_mut();
}

/// `~Socket()`: the destructor callback if it has not run, the buffers, the
/// `network::Socket` (its TLS session closed first), then the bound state and the
/// metadata, whose TLS context outlived the session.
///
/// # Safety
/// `socket` is a live record, not used afterwards; its buffers came from `new String`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_release(socket: *mut CoSocket) {
    // SAFETY: as the caller promises; the callback runs before anything is freed.
    crate::socket_extra::run_close_hook(unsafe { Co::new(socket) });
    // SAFETY: as the caller promises.
    let record = unsafe { &mut *socket };
    for buffer in [record.read_buffer, record.write_buffer] {
        if !buffer.is_null() {
            // SAFETY: the socket's own `new String`, deleted once.
            unsafe { delete_string(buffer.cast()) };
        }
    }
    let network = record.socket;
    // SAFETY: the socket's live network record, if any.
    if let Some(socket) = unsafe { Socket::from_ptr(network) }
        && !socket.read(|record| record.ssl.is_null())
    {
        socket_ssl_close(socket);
    }
    if !network.is_null() {
        // SAFETY: the live record, which nothing uses afterwards.
        unsafe { socket_free(network) };
    }
    // SAFETY: as the caller promises.
    unsafe { swr_v1_hook_co_socket_fini(socket) };
}

/// `new SocketImpl(...)` for the sockets Rust hands out: by `domain`, `kind` and
/// `protocol` with `family`, else by the `swSocketType` `kind`; over `fd` unless it is
/// negative. A `UringSocket` when the build says so. Never null; a failed socket(2)
/// leaves it without an fd.
#[must_use]
pub fn make(fd: c_int, domain: c_int, kind: c_int, protocol: c_int, family: bool) -> Co {
    let (socket, result) = make_quiet(fd, domain, kind, protocol, family);
    warn_created(socket, result);
    socket
}

/// [`make`] before its warnings, whose PHP error handler may bail out: the socket
/// and the bits [`warn_created`] reports, so the caller can own the socket first.
#[must_use]
pub fn make_quiet(
    fd: c_int,
    domain: c_int,
    kind: c_int,
    protocol: c_int,
    family: bool,
) -> (Co, u32) {
    let how = match (family, fd < 0) {
        (true, true) => NEW_BY_FAMILY,
        (true, false) => NEW_FD_BY_FAMILY,
        (false, true) => NEW_BY_TYPE,
        (false, false) => NEW_FD_BY_TYPE,
    };
    let mut socket = Box::new(CoSocket::new(SocketState::alloc()));
    let owner = ptr::from_mut(&mut *socket).cast();
    let result = socket.construct(owner, how, fd, kind, domain, kind, protocol);
    // SAFETY: the record just made, with its state and metadata, which the caller owns.
    let socket = unsafe { Co::new(Box::into_raw(socket)) };
    if crate::stdio::host().is_some_and(|host| host.uring_socket) {
        crate::socket_extra::swr_v1_hook_co_socket_uring(socket);
    }
    (socket, result)
}

/// The warnings [`make_quiet`]'s `result` holds, over its socket.
pub fn warn_created(socket: Co, result: u32) {
    socket.with(|socket| swr_v1_hook_co_socket_created(socket, result));
}

/// `delete socket` of a socket Rust made or accepted: `~Socket`, then its storage.
///
/// # Safety
/// `socket` came from [`make`] or `accept` and is not used
/// afterwards.
pub unsafe extern "C" fn swr_v1_hook_co_socket_free(socket: *mut c_void) {
    let socket = socket.cast::<CoSocket>();
    // SAFETY: as the caller promises.
    unsafe { swr_v1_hook_co_socket_release(socket) };
    // SAFETY: the record `make` or `accept` boxed, released once.
    drop(unsafe { Box::from_raw(socket) });
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
    unsafe { init(socket) }.construct(owner, how, fd, socket_type, domain, kind, protocol)
}

impl CoSocket {
    /// The public constructors' work over a fresh record (see
    /// [`swr_v1_hook_co_socket_new`]).
    #[expect(clippy::too_many_arguments, reason = "the constructors' union")]
    fn construct(
        &mut self,
        owner: *mut c_void,
        how: c_int,
        fd: c_int,
        socket_type: c_int,
        domain: c_int,
        kind: c_int,
        protocol: c_int,
    ) -> u32 {
        let socket = self;
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
        let opened = if how == NEW_BY_FAMILY || how == NEW_BY_TYPE {
            socket.open(owner)
        } else {
            socket.adopt(fd, owner)
        };
        let Some(network) = opened else {
            return 0;
        };
        if how != NEW_BY_FAMILY && how != NEW_BY_TYPE {
            if how == NEW_FD_BY_TYPE && socket_type == SOCK_RAW {
                return result;
            }
            result |= set_nonblock(network);
        }
        result | socket.init_options(network)
    }
}

/// `reinit_sock(type)`: a new socket of `socket_type` replaces the record's, whose
/// old `network::Socket` is returned for the caller to free. `None`, with errno set
/// and nothing changed, if socket(2) failed.
pub(crate) fn reinit(socket: Co, socket_type: c_int) -> Option<*mut SocketRecord> {
    let owner = socket.this();
    socket.with(|socket| socket.reinit(owner, socket_type))
}

impl CoSocket {
    fn reinit(&mut self, owner: *mut c_void, socket_type: c_int) -> Option<*mut SocketRecord> {
        let socket = self;
        let (mut domain, mut kind) = (0, 0);
        domain_and_type(socket_type, &mut domain, &mut kind);
        let made = SocketRecord::open(
            socket_type,
            FD_CO_SOCKET,
            domain,
            kind,
            socket.sock_protocol,
            OPEN_FLAGS,
        );
        let old = socket.socket;
        // Upstream records the old type on the new socket before taking the new one.
        socket.attach(made, owner)?;
        socket.sock_domain = domain;
        socket.sock_type = kind;
        socket.kind = socket_type;
        Some(old)
    }
}

/// `new Socket(conn, server)`, the accepting constructor: `conn`, accepted on
/// `server`, with the server's type, framing and timeouts, connected; `accept`
/// shares the TLS context after. [`swr_v1_hook_co_socket_free`] releases it.
///
/// # Safety
/// `conn` is a live accepted `network::Socket` the new socket takes.
pub(crate) unsafe fn accepted(server: Co, conn: *mut SocketRecord) -> Co {
    let mut socket = Box::new(CoSocket::new(SocketState::alloc()));
    let owner = ptr::from_mut(&mut *socket).cast::<c_void>();
    let timeouts = server.net(|record| record.timeouts);
    // SAFETY: the server's live record, read here only.
    let server = unsafe { &*server.raw() };
    socket.kind = server.kind;
    socket.sock_domain = server.sock_domain;
    socket.sock_type = server.sock_type;
    socket.sock_protocol = server.sock_protocol;
    socket.socket = conn;
    let mut result = OPENED;
    // SAFETY: as the caller promises.
    if let Some(network) = unsafe { Socket::from_ptr(conn) } {
        socket.sock_fd = network.edit(|record| {
            record.object = owner;
            record.socket_type = server.kind;
            record.fd_type = FD_CO_SOCKET;
            record.fd
        });
        result |= socket.init_options(network);
        if let Some(timeouts) = timeouts {
            network.edit(|record| record.timeouts = timeouts);
        }
    }
    socket.open_length_check = server.open_length_check;
    socket.open_eof_check = server.open_eof_check;
    socket.http2 = server.http2;
    socket.protocol = server.protocol;
    // SAFETY: the record's own state.
    unsafe { (*socket.state).flags.connected = true };
    socket.ssl_is_server = server.ssl_is_server;
    swr_v1_hook_co_socket_created(&socket, result);
    // SAFETY: the record just made, which the caller owns.
    unsafe { Co::new(Box::into_raw(socket)) }
}

/// What a constructor's bits report (`Socket::created`): a failed socket(2) becomes
/// the last error; the fd option and `TCP_NODELAY` failures are warnings.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_co_socket_created(socket: &CoSocket, result: u32) {
    use swoole_rs_corebase::log;
    if result & OPENED == 0 {
        log::set_last_error(crate::co_io::errno());
        return;
    }
    let fd = socket.sock_fd;
    let warnings = result >> FD_WARNINGS_SHIFT;
    if warnings & 1 != 0 {
        log::sys_warning(
            c"Socket::created",
            format_args!("fcntl({fd}, GETFL) failed"),
        );
    }
    if warnings & 2 != 0 {
        log::sys_warning(
            c"Socket::created",
            format_args!("fcntl({fd}, SETFL, opts) failed"),
        );
    }
    if result & NODELAY_FAILED != 0 {
        log::sys_warning(
            c"Socket::created",
            format_args!(
                "setsockopt({fd}, {}, {}, {}) failed",
                libc::IPPROTO_TCP,
                libc::TCP_NODELAY,
                size_of::<c_int>()
            ),
        );
    }
}

/// `getpeername(sa)`: the peer's address and the record's type into `address`; 0,
/// or the errno to set.
pub(crate) fn peer_name(socket: Co, address: &mut SocketAddress) -> c_int {
    address.len = socklen::<swoole_rs_reactor::AddressStorage>();
    // SAFETY: the address's own storage and length.
    let failed = unsafe {
        libc::getpeername(
            socket.sock_fd(),
            (&raw mut address.addr).cast(),
            &raw mut address.len,
        )
    } != 0;
    if failed {
        return std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    }
    address.kind = socket.kind();
    0
}

/// An open record over a `network::Socket` record without a descriptor, for the
/// operations' tests.
#[cfg(test)]
pub(crate) struct TestSocket(pub Box<CoSocket>);

#[cfg(test)]
impl TestSocket {
    pub(crate) fn new() -> Self {
        let mut storage = Box::new(core::mem::MaybeUninit::<CoSocket>::uninit());
        unsafe { init(storage.as_mut_ptr()) };
        let mut socket = unsafe { Box::from_raw(Box::into_raw(storage).cast::<CoSocket>()) };
        socket.socket = SocketRecord::with_fd(-1, FD_CO_SOCKET).into_raw();
        socket.sock_fd = 100;
        Self(socket)
    }

    pub(crate) fn ptr(&mut self) -> *mut CoSocket {
        &raw mut *self.0
    }

    pub(crate) fn co(&mut self) -> Co {
        unsafe { Co::new(self.ptr()) }
    }

    pub(crate) fn state(&self) -> *mut SocketState {
        self.0.state
    }
}

#[cfg(test)]
impl Drop for TestSocket {
    fn drop(&mut self) {
        unsafe {
            SocketRecord::free(self.0.socket);
            swr_v1_hook_co_socket_fini(self.ptr());
        }
    }
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
                libc::close((*socket.socket).fields.fd);
                SocketRecord::free(socket.socket);
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
        assert_eq!(unsafe { (*socket.socket).fields.socket_type }, 2);
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
        assert_eq!(unsafe { (*socket.socket).fields.fd_type }, 6);
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
        let old = reinit(unsafe { Co::new(&raw mut *socket) }, 3).unwrap();
        assert_eq!(old, first);
        assert_eq!((socket.kind, socket.sock_domain), (3, libc::AF_INET6));
        assert_eq!(unsafe { (*socket.socket).fields.info.kind }, 1);
        unsafe {
            libc::close((*old).fields.fd);
            SocketRecord::free(old);
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
        unsafe { (*server.socket).fields.timeouts.read = 1.5 };
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        let conn = SocketRecord::with_fd(fd, 0).into_raw();
        let socket = unsafe { accepted(Co::new(&raw mut *server), conn) };
        let socket = unsafe { Box::from_raw(socket.raw()) };
        assert_eq!(
            unsafe { (*conn).fields.object },
            (&raw const *socket).cast_mut().cast()
        );
        assert_eq!((socket.kind, socket.sock_fd), (1, fd));
        assert!(socket.open_eof_check && socket.http2 && socket.ssl_is_server);
        assert_eq!(socket.protocol.package_eof_len, 2);
        let record = unsafe { &*conn };
        assert_eq!((record.fd_type, record.socket_type), (6, 1));
        assert!((record.timeouts.read - 1.5).abs() < f64::EPSILON);
        assert!(unsafe { (*socket.state).flags.connected });
        release(socket);
        release(server);
    }

    #[test]
    fn peer_name_of_an_unconnected_socket_is_an_error() {
        let mut socket = record();
        new(&mut socket, NEW_BY_TYPE, 0, 1, 0, 0);
        let mut address: SocketAddress = unsafe { core::mem::zeroed() };
        let error = peer_name(unsafe { Co::new(&raw mut *socket) }, &mut address);
        assert_eq!(error, libc::ENOTCONN);
        assert_eq!(address.kind, 0);
        release(socket);
    }

    /// Upstream's constructors: a failed socket(2) leaves its errno as the last error;
    /// warnings leave it alone.
    #[test]
    fn created_publishes_a_failed_socket() {
        let socket = TestSocket::new();
        swoole_rs_corebase::log::set_last_error(0);
        crate::co_io::set_errno(libc::EMFILE);
        swr_v1_hook_co_socket_created(&socket.0, 0);
        assert_eq!(swoole_rs_corebase::log::last_error(), libc::EMFILE);
        swoole_rs_corebase::log::set_last_error(0);
        swr_v1_hook_co_socket_created(&socket.0, OPENED | NODELAY_FAILED | 3 << FD_WARNINGS_SHIFT);
        assert_eq!(swoole_rs_corebase::log::last_error(), 0);
    }
}
