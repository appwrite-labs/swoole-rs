//! Rust owns the socket record. The native SSH2 hook reads its leading descriptor.
use crate::constants::{
    SW_SOCKET_BUFFER_SIZE, SW_SOCKET_DEFAULT_CONNECT_TIMEOUT, SW_SOCKET_DEFAULT_DNS_TIMEOUT,
    SW_SOCKET_DEFAULT_READ_TIMEOUT, SW_SOCKET_DEFAULT_WRITE_TIMEOUT,
};
use crate::constants::{SocketFlag, SocketType};
use crate::reactor::SocketOutput;
use crate::reactor::socket::SocketMarks;
use crate::reactor::socket_timeouts::{SocketTimeouts, TimeoutKind};
use crate::sys;
use std::ffi::{c_int, c_void};
use std::mem::offset_of;
use std::ops::{Deref, DerefMut};
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// `network::Socket`'s TLS state group, in the layout `swoole_rs_corebase::tls` works on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SocketTlsState {
    pub state: crate::constants::SslState,
    pub want_read: bool,
    pub want_write: bool,
}

pub use crate::net_types::{AddressStorage, SocketAddress};

/// An `SSL` whose one reference a socket record owns, freed when dropped: only an
/// `SSL_new` declaration (tls's) makes one, and only the unsafe
/// [`SocketFields::take_ssl`] hands one back out of a record.
#[repr(transparent)]
#[derive(Debug)]
pub struct SocketSsl(NonNull<openssl_sys::SSL>);

impl SocketSsl {
    /// The `SSL`, for calls over it while this owner holds it.
    #[must_use]
    pub fn as_non_null(&self) -> NonNull<openssl_sys::SSL> {
        self.0
    }
}

impl Drop for SocketSsl {
    fn drop(&mut self) {
        /// The reference a dropped `SocketSsl` held: only this drop makes one, once
        /// per owner, so `SSL_free` releases each `SSL` once.
        #[repr(transparent)]
        struct Released(NonNull<openssl_sys::SSL>);
        unsafe extern "C" {
            safe fn SSL_free(ssl: Released);
        }
        SSL_free(Released(self.0));
    }
}

/// The socket fields followed by the borrow state and the reactor's registration
/// serial. Access to these marks never aliases a closure's borrow of the fields.
// `repr(C)`: the bundled ssh2 hook reads `fd` through `native/hooks/swoole_rs_coroutine_socket.h`.
#[repr(C)]
pub struct SocketRecord {
    pub fields: SocketFields,
    pub(super) marks: SocketMarks,
}

impl Deref for SocketRecord {
    type Target = SocketFields;
    fn deref(&self) -> &SocketFields {
        &self.fields
    }
}

impl DerefMut for SocketRecord {
    fn deref_mut(&mut self) -> &mut SocketFields {
        &mut self.fields
    }
}

/// `network::Socket`'s fields. Pointers to native objects (`Buffer`, `TimerNode`,
/// `String`, the owner) are opaque here; the `SSL` is a [`SocketSsl`].
// `repr(C)`: the bundled ssh2 hook reads `fd` through `native/hooks/swoole_rs_coroutine_socket.h`.
#[repr(C)]
pub struct SocketFields {
    pub fd: c_int,
    /// `swFdType`.
    pub fd_type: c_int,
    /// `swSocketType`.
    pub socket_type: c_int,
    pub events: crate::reactor::Events,
    pub enable_tcp_nodelay: bool,
    pub kernel_nobufs: bool,
    pub removed: bool,
    pub silent_remove: bool,
    pub cloexec: bool,
    pub direct_send: bool,
    pub bound: bool,
    pub listened: bool,
    pub ssl_renegotiation: bool,
    pub ssl_handshake_buffer_set: bool,
    pub ssl_quiet_shutdown: bool,
    pub ssl_closed: bool,
    pub dtls: bool,
    pub tcp_nopush: bool,
    pub tcp_nodelay: bool,
    pub skip_recv: bool,
    pub recv_wait: bool,
    pub event_hup: bool,
    pub dont_restart: bool,
    pub close_notified: bool,
    pub chunk_size: u32,
    pub output: SocketOutput,
    /// The native owner (`network::Socket::object`) the readiness handlers reach:
    /// a client, port, connection, query or wait, null without one.
    object: *mut c_void,
    /// The `SSL` the record owns, freed with it: pointer-sized, null without one.
    ssl: Option<SocketSsl>,
    pub tls: SocketTlsState,
    pub timeouts: SocketTimeouts,
    pub last_received_time: f64,
    pub last_sent_time: f64,
    /// `Buffer *in_buffer`: owned as `output.out_buffer` is.
    pub in_buffer: Option<Box<crate::buffer::cpp::OutputQueue>>,
    /// The session's receive buffer between readable events.
    pub recv_buffer: Option<Box<crate::buffer::string::OwnedString>>,
    /// `TimerNode *recv_timer`, held by id.
    pub recv_timer: Option<crate::timer_key::Held>,
    pub total_recv_bytes: usize,
    pub total_send_bytes: usize,
    /// Last of native code's fields, so every offset before it is the same on each
    /// LP64 target.
    pub info: SocketAddress,
    /// What the record serves, by type, for Rust's readiness handlers to get back
    /// ([`Self::owner`]); dropped with the record. Native code only keeps its place.
    owner: Option<Box<dyn core::any::Any>>,
    /// An owner kept elsewhere, by weak handle ([`Self::shared_owner`]), which costs
    /// no allocation of its own. Zero bytes are `None` here too.
    shared_owner: Option<std::rc::Weak<dyn core::any::Any>>,
}

/// A process-wide `f64` setting, its bits in an `AtomicU64`.
pub struct DefaultTimeout(AtomicU64);

impl DefaultTimeout {
    const fn new(seconds: f64) -> Self {
        Self(AtomicU64::new(seconds.to_bits()))
    }

    #[must_use]
    pub fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, seconds: f64) {
        self.0.store(seconds.to_bits(), Ordering::Relaxed);
    }
}

/// `Socket::default_*_timeout` and `Socket::default_buffer_size`, which every new
/// record copies. `Co::set()` may assign them from any thread while others create
/// sockets, so each is an atomic. They are independent settings, nothing is published
/// through them, so `Relaxed` is enough.
pub struct SocketDefaults {
    pub dns: DefaultTimeout,
    pub connect: DefaultTimeout,
    pub read: DefaultTimeout,
    pub write: DefaultTimeout,
    buffer_size: AtomicU32,
}

impl SocketDefaults {
    /// The timeouts, as a new record takes them.
    #[must_use]
    pub fn timeouts(&self) -> SocketTimeouts {
        SocketTimeouts {
            dns: self.dns.get(),
            connect: self.connect.get(),
            read: self.read.get(),
            write: self.write.get(),
        }
    }

    #[must_use]
    pub fn buffer_size(&self) -> u32 {
        self.buffer_size.load(Ordering::Relaxed)
    }

    pub fn set_buffer_size(&self, size: u32) {
        self.buffer_size.store(size, Ordering::Relaxed);
    }
}

/// `SW_SOCKET_DEFAULT_*_TIMEOUT` and `SW_SOCKET_BUFFER_SIZE`.
pub static SOCKET_DEFAULTS: SocketDefaults = SocketDefaults {
    dns: DefaultTimeout::new(SW_SOCKET_DEFAULT_DNS_TIMEOUT as f64),
    connect: DefaultTimeout::new(SW_SOCKET_DEFAULT_CONNECT_TIMEOUT as f64),
    read: DefaultTimeout::new(SW_SOCKET_DEFAULT_READ_TIMEOUT as f64),
    write: DefaultTimeout::new(SW_SOCKET_DEFAULT_WRITE_TIMEOUT as f64),
    buffer_size: AtomicU32::new(SW_SOCKET_BUFFER_SIZE),
};

// The native SSH2 hook reads network::Socket::fd from the allocation's start.
const _: () = assert!(
    offset_of!(SocketRecord, fields) == 0 && offset_of!(SocketFields, fd) == 0,
    "native socket callers read the descriptor at offset zero"
);

/// Where a socket's descriptor stands, from [`SocketFields::descriptor`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Descriptor {
    Open(std::os::fd::RawFd),
    /// Open, but the socket waits to close once its output drains.
    Closing(std::os::fd::RawFd),
    Closed,
}

impl Descriptor {
    /// The descriptor, open or closing; -1 once closed.
    #[must_use]
    pub const fn raw_fd(self) -> std::os::fd::RawFd {
        match self {
            Self::Open(fd) | Self::Closing(fd) => fd,
            Self::Closed => -1,
        }
    }
}

impl SocketRecord {
    /// A socket as `new Socket()` makes it: all zero but the default timeouts, and
    /// not borrowed.
    #[must_use]
    pub fn new() -> Self {
        Self {
            fields: SocketFields {
                fd: 0,
                fd_type: 0,
                socket_type: 0,
                events: crate::reactor::Events::NONE,
                enable_tcp_nodelay: false,
                kernel_nobufs: false,
                removed: false,
                silent_remove: false,
                cloexec: false,
                direct_send: false,
                bound: false,
                listened: false,
                ssl_renegotiation: false,
                ssl_handshake_buffer_set: false,
                ssl_quiet_shutdown: false,
                ssl_closed: false,
                dtls: false,
                tcp_nopush: false,
                tcp_nodelay: false,
                skip_recv: false,
                recv_wait: false,
                event_hup: false,
                dont_restart: false,
                close_notified: false,
                chunk_size: 0,
                output: SocketOutput {
                    out_buffer: None,
                    send_timer: None,
                    buffer_size: 0,
                    close_wait: false,
                    nonblock: false,
                    ssl_send: false,
                    send_wait: false,
                },
                object: ptr::null_mut(),
                ssl: None,
                tls: SocketTlsState::default(),
                timeouts: SOCKET_DEFAULTS.timeouts(),
                last_received_time: 0.0,
                last_sent_time: 0.0,
                in_buffer: None,
                recv_buffer: None,
                recv_timer: None,
                total_recv_bytes: 0,
                total_send_bytes: 0,
                info: sys::zeroed(),
                owner: None,
                shared_owner: None,
            },
            marks: SocketMarks::default(),
        }
    }

    /// `network::Socket s{}; s.fd = fd;`: a record as [`Self::new`] makes it, over a
    /// descriptor the caller hands it.
    #[must_use]
    pub fn over_fd(fd: c_int) -> Self {
        let mut socket = Self::new();
        socket.fd = fd;
        socket
    }

    /// `make_socket(fd, fd_type)`'s fields.
    #[must_use]
    pub fn with_fd(fd: c_int, fd_type: c_int) -> Self {
        let mut socket = Self::over_fd(fd);
        socket.fd_type = fd_type;
        socket.removed = true;
        socket
    }

    /// The record moved to the heap, as `new Socket()`; freed by its [`crate::reactor::OwnedSocket`].
    #[must_use]
    pub fn into_raw(self) -> *mut Self {
        Box::into_raw(Box::new(self))
    }

    /// A new heap record as the socket a test holds: live until an owner taken back
    /// with the unsafe [`crate::reactor::OwnedSocket::from_socket`] frees it. Production
    /// code makes an [`crate::reactor::OwnedSocket`] instead (feature `test-records`).
    #[cfg(any(test, feature = "test-records"))]
    #[must_use]
    pub fn into_socket(self: Box<Self>) -> crate::reactor::Heap<'static> {
        crate::reactor::Heap::from_static(crate::reactor::Socket::new(Box::leak(self)))
    }

    /// base.cc's `socket_free_defer`: [`SocketFields::release`], warning when close(2)
    /// failed, then the record goes.
    pub fn close_and_free(mut self: Box<Self>) {
        let fd = self.fd;
        if !self.release() {
            crate::log::sys_warning(c"socket_free_defer", format_args!("close({fd}) failed"));
        }
    }

    /// `make_socket(type, fd_type, domain, type, protocol, flags)`: socket(2) through
    /// `swoole::socket`, then a record of `socket_type` with the nonblocking and
    /// close-on-exec flags `flags` asked for. Null, with errno set, if socket(2) failed.
    #[must_use]
    pub fn open(
        socket_type: c_int,
        fd_type: c_int,
        domain: c_int,
        kind: c_int,
        protocol: c_int,
        flags: c_int,
    ) -> *mut Self {
        Self::opened(socket_type, fd_type, domain, kind, protocol, flags)
            .map_or(ptr::null_mut(), Box::into_raw)
    }

    /// [`Self::open`]'s record, still its maker's own; none, with errno set, if
    /// socket(2) failed.
    #[must_use]
    pub fn opened(
        socket_type: c_int,
        fd_type: c_int,
        domain: c_int,
        kind: c_int,
        protocol: c_int,
        flags: c_int,
    ) -> Option<Box<Self>> {
        let fd = crate::reactor::swr_v1_socket_open(domain, kind, protocol, flags).ok()?;
        let mut socket = Self::with_fd(fd, fd_type);
        socket.open_as(socket_type, SocketFlag::from_bits(flags));
        Some(Box::new(socket))
    }

    /// `make_socket(type, fd_type, flags)`: [`Self::open`] with the domain and type
    /// `socket_type` maps to. For a type with none, null with errno
    /// `ESOCKTNOSUPPORT` and `unknown` set, so the caller warns.
    pub fn open_typed(
        socket_type: c_int,
        fd_type: c_int,
        flags: c_int,
        unknown: &mut bool,
    ) -> *mut Self {
        Self::opened_typed(socket_type, fd_type, flags, unknown)
            .map_or(ptr::null_mut(), Box::into_raw)
    }

    /// [`Self::open_typed`]'s record, still its maker's own; none where that is null.
    pub fn opened_typed(
        socket_type: c_int,
        fd_type: c_int,
        flags: c_int,
        unknown: &mut bool,
    ) -> Option<Box<Self>> {
        let Some((domain, kind)) = crate::reactor::socket_domain_and_type(socket_type) else {
            *unknown = true;
            crate::reactor::socket_sync::set_errno(libc::ESOCKTNOSUPPORT);
            return None;
        };
        Self::opened(socket_type, fd_type, domain, kind, 0, flags)
    }
}

/// The socket's timeouts, as its callers arm, read and clear them.
impl SocketFields {
    /// `Socket::set_timeout`: `timeout` for every kind `kind` names; zero changes
    /// nothing, and a negative timeout waits without limit.
    #[inline]
    pub fn set_timeout(&mut self, timeout: f64, kind: TimeoutKind) {
        self.timeouts.set(timeout, kind);
    }

    /// `Socket::get_timeout`: the timeout `kind` names, unless it names other than one.
    #[inline]
    #[must_use]
    pub fn timeout(&self, kind: TimeoutKind) -> Option<f64> {
        self.timeouts.get(kind)
    }

    /// How long a connect waits.
    #[inline]
    #[must_use]
    pub const fn connect_timeout(&self) -> f64 {
        self.timeouts.connect
    }

    /// How long name resolution waits.
    #[inline]
    #[must_use]
    pub const fn dns_timeout(&self) -> f64 {
        self.timeouts.dns
    }

    /// How long a write (`writing`) or a read waits.
    #[inline]
    #[must_use]
    pub const fn io_timeout(&self, writing: bool) -> f64 {
        if writing {
            self.timeouts.write
        } else {
            self.timeouts.read
        }
    }

    /// Read and write timeouts of zero: a socket the reactor waits on, whose own
    /// operations never wait.
    #[inline]
    pub const fn clear_io_timeouts(&mut self) {
        self.timeouts.read = 0.0;
        self.timeouts.write = 0.0;
    }

    /// Every timeout of `from`, as an accepted socket takes its server's.
    #[inline]
    pub const fn inherit_timeouts(&mut self, from: &Self) {
        self.timeouts = from.timeouts;
    }
}

impl SocketFields {
    /// The record's `SSL`, or null without one.
    #[must_use]
    pub fn ssl(&self) -> *mut c_void {
        self.ssl
            .as_ref()
            .map_or(ptr::null_mut(), |ssl| ssl.as_non_null().as_ptr().cast())
    }

    /// Gives the record `ssl`, or clears it with none. An `SSL` it held already is left
    /// unfreed, as upstream's assignment leaves it: a handle to it may still be in use.
    #[expect(
        clippy::mem_forget,
        reason = "freeing the replaced SSL is what a live handle to it forbids"
    )]
    pub fn set_ssl(&mut self, ssl: Option<SocketSsl>) {
        std::mem::forget(std::mem::replace(&mut self.ssl, ssl));
    }

    /// Release the record's SSL reference; pending operations retain theirs.
    pub fn close_ssl(&mut self) {
        self.ssl = None;
    }

    /// The record's `SSL` out of it, or none: freed when the owner drops.
    /// # Safety
    /// No handle to the `SSL` or its BIOs is used once the owner drops, and no other
    /// record takes the owner (the `SSL`'s ex data may name this one).
    pub unsafe fn take_ssl(&mut self) -> Option<SocketSsl> {
        self.ssl.take()
    }

    /// A copy of the owner [`Self::set_owner`] attached, when it is a `T`.
    #[inline]
    #[must_use]
    pub fn owner<T: core::any::Any + Clone>(&self) -> Option<T> {
        self.owner.as_deref()?.downcast_ref::<T>().cloned()
    }

    /// The live owner [`Self::set_shared_owner`] attached, when it is a `T`.
    #[inline]
    #[must_use]
    pub fn shared_owner<T: core::any::Any>(&self) -> Option<std::rc::Rc<T>> {
        self.shared_owner.as_ref()?.upgrade()?.downcast::<T>().ok()
    }

    /// Attaches `owner` in place of what was attached, which drops; none detaches.
    pub fn set_owner<T: core::any::Any>(&mut self, owner: Option<T>) {
        self.owner = owner.map(|owner| -> Box<dyn core::any::Any> { Box::new(owner) });
    }

    /// Attaches the boxed `owner` without another allocation, giving back what was
    /// attached.
    pub fn hold_owner<T: core::any::Any>(
        &mut self,
        owner: Box<T>,
    ) -> Option<Box<dyn core::any::Any>> {
        self.owner.replace(owner)
    }

    /// The owner, detached, when it is a `T`; another type's stays attached.
    pub fn take_owner<T: core::any::Any>(&mut self) -> Option<Box<T>> {
        match self.owner.take()?.downcast::<T>() {
            Ok(owner) => Some(owner),
            Err(other) => {
                self.owner = Some(other);
                None
            }
        }
    }

    /// Attaches a weak handle to `owner` in place of the shared owner attached.
    pub fn set_shared_owner<T: core::any::Any>(&mut self, owner: std::rc::Weak<T>) {
        self.shared_owner = Some(owner);
    }

    /// Attaches `object`, the native owner this socket's readiness handlers reach,
    /// in place of what was attached.
    #[inline]
    pub fn attach_object<T>(&mut self, object: *mut T) {
        self.object = object.cast();
    }

    /// The native owner [`Self::attach_object`] attached, as the `T` its attacher
    /// made it; null when none is.
    #[inline]
    #[must_use]
    pub fn attached_object<T>(&self) -> *mut T {
        self.object.cast()
    }

    /// Detaches the native owner, as one that is going away does so a close in
    /// progress knows it is gone.
    #[inline]
    pub fn detach_object(&mut self) {
        self.object = ptr::null_mut();
    }

    /// Whether a native owner is attached.
    #[inline]
    #[must_use]
    pub fn has_object(&self) -> bool {
        !self.object.is_null()
    }

    /// The socket's type, `None` for a code no `SW_SOCK_*` names.
    #[must_use]
    pub fn kind(&self) -> Option<SocketType> {
        SocketType::from_raw(self.socket_type)
    }

    /// `make_socket`'s record: a socket of `socket_type` (an `SW_SOCK_*`) whose
    /// descriptor is nonblocking and close-on-exec as `flags` says.
    #[inline]
    pub fn open_as(&mut self, socket_type: c_int, flags: SocketFlag) {
        self.output.nonblock = flags.contains(SocketFlag::SW_SOCK_NONBLOCK);
        self.cloexec = flags.contains(SocketFlag::SW_SOCK_CLOEXEC);
        self.socket_type = socket_type;
    }

    /// A socket of `kind`, and its address one of `kind` too, as a socket adopted
    /// from a descriptor learns what it is.
    #[inline]
    pub const fn type_as(&mut self, kind: SocketType) {
        self.socket_type = kind.raw();
        self.info.kind = kind.raw();
    }

    /// `socket->object = this; socket_type = type; fd_type = ...`: the record now
    /// serves `owner`, as a socket of `kind` (an `SW_SOCK_*`) on `fd_type`'s handlers.
    #[inline]
    pub fn serve<T: core::any::Any>(&mut self, owner: T, kind: c_int, fd_type: c_int) {
        self.set_owner(Some(owner));
        self.socket_type = kind;
        self.fd_type = fd_type;
    }

    /// `Socket::is_stream`: TCP, TCP6 or a Unix stream.
    #[inline]
    #[must_use]
    pub fn is_stream(&self) -> bool {
        self.kind().is_some_and(SocketType::is_stream)
    }

    /// `Socket::is_dgram`: UDP, UDP6 or a Unix datagram socket.
    #[inline]
    #[must_use]
    pub fn is_dgram(&self) -> bool {
        self.kind().is_some_and(SocketType::is_dgram)
    }

    /// `Socket::is_tcp`: a TCP stream over IPv4 or IPv6.
    #[must_use]
    pub fn is_tcp(&self) -> bool {
        self.kind().is_some_and(SocketType::is_tcp)
    }

    /// `Socket::is_inet4() || is_inet6()`: stream, datagram or raw.
    #[must_use]
    pub fn is_inet(&self) -> bool {
        self.kind()
            .is_some_and(|kind| kind.is_inet4() || kind.is_inet6())
    }

    /// `Socket::is_local`: a Unix stream or datagram socket.
    #[must_use]
    pub fn is_local(&self) -> bool {
        self.kind().is_some_and(SocketType::is_local)
    }

    /// `Socket::catch_error` (`catch_read_error`, `catch_write_error`).
    pub fn catch_error(&mut self, error: c_int) -> crate::reactor::Caught {
        crate::reactor::catch_error(error, &mut self.kernel_nobufs)
    }

    /// `Socket::set_tcp_nodelay`.
    pub fn set_tcp_nodelay(&mut self, nodelay: c_int) -> bool {
        if !crate::reactor::swr_v1_socket_tcp_nodelay(self.fd, nodelay) {
            return false;
        }
        // Upstream's 1-bit field keeps the int's low bit.
        self.tcp_nodelay = nodelay & 1 != 0;
        true
    }

    /// `socket_free_defer`'s descriptor work: a bound Unix socket's path is unlinked
    /// (from getsockname(2), as upstream), then the descriptor closed. False when
    /// close(2) failed, with errno set.
    pub fn release(&mut self) -> bool {
        if self.fd == -1 {
            return true;
        }
        if self.is_local() && self.bound && self.get_name() {
            let path = self.info.addr.unix().sun_path;
            // c_char is i8 or u8 by target; its one byte either way.
            let path = path.map(|byte| byte.to_ne_bytes()[0]);
            if let Ok(path) = std::ffi::CStr::from_bytes_until_nul(&path)
                && !path.is_empty()
            {
                _ = sys::unlink(path);
            }
        }
        self.close_descriptor()
    }

    /// close(2) of the descriptor, once: -1 then marks it closed. False when close(2)
    /// failed, with errno set.
    pub fn close_descriptor(&mut self) -> bool {
        sys::close(std::mem::replace(&mut self.fd, -1)).is_ok()
    }

    /// Where the descriptor stands: open, open but waiting to close, or closed (-1).
    #[inline]
    #[must_use]
    pub fn descriptor(&self) -> Descriptor {
        match self.fd {
            -1 => Descriptor::Closed,
            fd if self.close_deferred() => Descriptor::Closing(fd),
            fd => Descriptor::Open(fd),
        }
    }

    /// Open and not taken off a reactor: upstream's test before `swoole_event_del`.
    #[inline]
    #[must_use]
    pub const fn is_registered(&self) -> bool {
        self.fd != -1 && !self.removed
    }

    /// setsockopt(2) of `value` as option `name` at `level`, without logging a failure.
    /// # Errors
    /// The syscall's.
    #[inline]
    pub fn set_option<T: sys::Pod>(
        &self,
        level: c_int,
        name: c_int,
        value: &T,
    ) -> std::io::Result<()> {
        sys::setsockopt(self.fd, level, name, value)
    }

    /// `SO_RCVBUF` and/or `SO_SNDBUF` set to `size`.
    /// # Errors
    /// The first setsockopt(2) that failed.
    #[inline]
    pub fn set_kernel_buffer_size(&self, recv: bool, send: bool, size: u32) -> std::io::Result<()> {
        crate::reactor::set_socket_buffer_size(self.fd, recv, send, size)
    }

    /// writev(2) of `vectors`: the bytes written, or None with errno set.
    #[inline]
    #[must_use]
    pub fn writev(&self, vectors: &[std::io::IoSlice<'_>]) -> Option<usize> {
        sys::writev(self.fd, vectors)
    }

    /// `move_fd()` whose descriptor someone else now owns: the record lets go of it
    /// (-1), so freeing the record leaves it open.
    #[inline]
    pub const fn forget_descriptor(&mut self) {
        self.fd = -1;
    }

    /// `Socket::dup`: a copy of every field but the buffers and the SSL, which stay this
    /// record's own, owning `dup(fd)`, not borrowed. Upstream's copy shares the `SSL`
    /// pointer; only the core-tests port calls this, on sockets without one.
    #[must_use]
    pub fn dup(&self) -> *mut SocketRecord {
        let output = SocketOutput {
            out_buffer: None,
            ..self.output
        };
        let mut copy = SocketRecord {
            fields: Self {
                output,
                ssl: None,
                in_buffer: None,
                recv_buffer: None,
                owner: None,
                shared_owner: None,
                ..*self
            },
            marks: SocketMarks::default(),
        };
        copy.fd = sys::dup(copy.fd).unwrap_or(-1);
        copy.into_raw()
    }

    /// `Socket::accept`: a connection accepted on this listener, as a new unregistered
    /// record that inherits its type and nonblocking flag, or null (errno set).
    /// Without accept4 the listener is marked close-on-exec, as upstream's
    /// `set_fd_option` call does.
    pub fn accept(&mut self) -> *mut SocketRecord {
        self.accepted().map_or(ptr::null_mut(), Box::into_raw)
    }

    /// [`Self::accept`]'s record, still its maker's own; none (errno set) where that
    /// is null.
    pub fn accepted(&mut self) -> Option<Box<SocketRecord>> {
        let mut address = sys::zeroed();
        let (fd, listener_cloexec) =
            crate::reactor::fd_accept(self.fd, &mut address, self.output.nonblock);
        if listener_cloexec {
            self.cloexec = true;
        }
        if fd < 0 {
            return None;
        }
        let mut socket = SocketRecord::with_fd(fd, 0);
        socket.socket_type = self.socket_type;
        address.kind = self.socket_type;
        socket.info = address;
        socket.output.nonblock = self.output.nonblock;
        socket.cloexec = true;
        Some(Box::new(socket))
    }
}

impl Default for SocketRecord {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl SocketSsl {
        /// A dangling owner for a test that only asks whether a record holds an `SSL`;
        /// it leaves the record through `set_ssl(None)`, which never frees it.
        pub(crate) fn stub() -> Self {
            Self(NonNull::dangling())
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no Unix sockets")]
    fn release_unlinks_a_bound_unix_socket_and_closes() {
        let path = std::env::temp_dir().join(format!("swr-release-{}", std::process::id()));
        std::fs::remove_file(&path).unwrap_or_default();
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
        let mut socket = SocketRecord::new();
        socket.fd = std::os::fd::IntoRawFd::into_raw_fd(listener);
        socket.socket_type = 5;
        socket.bound = true;
        assert!(socket.release());
        assert!(!path.exists(), "the path is unlinked");
        assert_eq!(socket.fd, -1);
        assert!(socket.release());
        socket.fd = i32::MAX;
        assert!(!socket.release());
    }

    #[test]
    fn new_records_carry_the_default_timeouts() {
        let socket = SocketRecord::new();
        assert_eq!(socket.timeouts.connect, 10.0);
        assert_eq!(socket.timeouts.read, 60.0);
        assert_eq!(socket.fd, 0);
        assert!(!socket.has_object() && socket.output.out_buffer.is_none());
    }

    #[test]
    fn owners_come_back_only_as_their_own_type_and_kind() {
        let mut socket = SocketRecord::new();
        socket.set_owner(Some(7_u32));
        assert_eq!(socket.owner::<u32>(), Some(7));
        assert_eq!(socket.owner::<u64>(), None);
        assert!(socket.shared_owner::<u32>().is_none());
        let shared = std::rc::Rc::new(9_u32);
        socket.set_shared_owner(std::rc::Rc::downgrade(&shared));
        assert_eq!(socket.shared_owner::<u32>().as_deref(), Some(&9));
        assert!(socket.shared_owner::<u64>().is_none());
        assert_eq!(socket.owner::<u32>(), Some(7), "each kind keeps its own");
        drop(shared);
        assert!(
            socket.shared_owner::<u32>().is_none(),
            "a dropped owner is gone"
        );
        assert!(socket.take_owner::<u64>().is_none());
        assert_eq!(socket.take_owner::<u32>().as_deref(), Some(&7), "kept");
        assert!(socket.take_owner::<u32>().is_none(), "taken once");
        assert!(socket.hold_owner(Box::new(1_u8)).is_none());
        let replaced = socket.hold_owner(Box::new(2_u8));
        assert_eq!(
            replaced
                .and_then(|old| old.downcast::<u8>().ok())
                .as_deref(),
            Some(&1)
        );
        assert_eq!(socket.owner::<u8>(), Some(2));
    }

    #[test]
    fn inet_types_are_ipv4_and_ipv6() {
        let mut socket = SocketRecord::new();
        for (kind, inet) in [
            (1, true),
            (4, true),
            (5, false),
            (6, false),
            (7, true),
            (8, true),
        ] {
            socket.socket_type = kind;
            assert_eq!(socket.is_inet(), inet, "type {kind}");
            assert_eq!(socket.is_tcp(), kind == 1, "type {kind}");
        }
    }

    #[test]
    fn make_owns_the_fd_unregistered() {
        let socket = SocketRecord::with_fd(7, 3).into_raw();
        // SAFETY: the record just made.
        let record = unsafe { &*socket };
        assert_eq!((record.fd, record.fd_type, record.removed), (7, 3, true));
        // SAFETY: as above, not used again.
        unsafe { drop(Box::from_raw(socket)) };
    }

    #[test]
    fn unknown_types_fail_with_esocktnosupport() {
        let mut unknown = false;
        let socket = SocketRecord::open_typed(99, 0, 0, &mut unknown);
        assert!(socket.is_null() && unknown);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESOCKTNOSUPPORT)
        );
    }

    #[test]
    fn typed_sockets_record_their_flags() {
        let mut unknown = false;
        let socket = SocketRecord::open_typed(1, 5, 0b1100, &mut unknown);
        assert!(!socket.is_null() && !unknown);
        // SAFETY: the record just made.
        let record = unsafe { &*socket };
        assert_eq!((record.socket_type, record.fd_type), (1, 5));
        assert_eq!(
            (record.output.nonblock, record.cloexec, record.removed),
            (true, true, true)
        );
        // SAFETY: the socket's own fd.
        unsafe { libc::close(record.fd) };
        // SAFETY: not used again.
        unsafe { drop(Box::from_raw(socket)) };
    }

    #[test]
    fn dup_copies_fields_and_owns_a_new_fd() {
        let mut fds = [0; 2];
        // SAFETY: a two-int array.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let mut original = SocketRecord::with_fd(fds[0], 2);
        original.chunk_size = 42;
        let copy = original.dup();
        // SAFETY: the record just made.
        let record = unsafe { &*copy };
        assert_eq!(record.chunk_size, 42);
        assert!(record.fd >= 0 && record.fd != fds[0]);
        for fd in [record.fd, fds[0], fds[1]] {
            // SAFETY: fds this test opened.
            unsafe { libc::close(fd) };
        }
        // SAFETY: not used again.
        unsafe { drop(Box::from_raw(copy)) };
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no Unix sockets")]
    fn accept_fills_a_new_record_from_the_listener() {
        let (mut unknown, flags) = (false, 0b1100);
        let listener = SocketRecord::open_typed(5, 1, flags, &mut unknown);
        // SAFETY: the record just made.
        let record = unsafe { &mut *listener };
        // An unbound, unlistened socket: accept fails, and no record is made.
        // SAFETY: the live listener.
        assert!(record.accept().is_null());
        // SAFETY: the listener's own fd.
        unsafe { libc::close(record.fd) };
        // SAFETY: not used again.
        unsafe { drop(Box::from_raw(listener)) };
    }
}
