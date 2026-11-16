//! `network::Socket`'s record: every field, in Rust's layout, allocated, initialised,
//! duplicated and freed here. Native `Socket` derives from its mirror
//! (`native/reactor/swoole_rs_socket.h`) and adds only methods; both sides assert the
//! offsets below. The upstream 1-bit flags are bytes here, which keeps every value
//! the code assigns.
use crate::SocketOutput;
use crate::socket_timeouts::SocketTimeouts;
use std::ffi::{c_int, c_void};
use std::mem::{offset_of, size_of};
use std::ops::{Deref, DerefMut};
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use swoole_rs_corebase::constants::SocketType;
use swoole_rs_corebase::constants::{
    SW_SOCKET_BUFFER_SIZE, SW_SOCKET_DEFAULT_CONNECT_TIMEOUT, SW_SOCKET_DEFAULT_DNS_TIMEOUT,
    SW_SOCKET_DEFAULT_READ_TIMEOUT, SW_SOCKET_DEFAULT_WRITE_TIMEOUT,
};
use swoole_rs_corebase::sys;

/// `network::Socket`'s TLS state group, in the layout `swoole_rs_tls` works on.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SocketTlsState {
    pub state: swoole_rs_corebase::constants::SslState,
    pub want_read: bool,
    pub want_write: bool,
}

pub use swoole_rs_corebase::net_types::{AddressStorage, SocketAddress};

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

/// `network::Socket`: its fields, then the borrow count [`crate::Socket`]'s `read`
/// and `edit` hold while a closure has the fields. The count sits outside them, so
/// taking it never touches bytes a closure's reference covers. It is atomic because
/// threads share some records (a pool's pipe socket).
#[repr(C)]
pub struct SocketRecord {
    pub fields: SocketFields,
    pub(crate) borrows: AtomicU32,
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
#[repr(C)]
pub struct SocketFields {
    pub fd: c_int,
    /// `swFdType`.
    pub fd_type: c_int,
    /// `swSocketType`.
    pub socket_type: c_int,
    pub events: crate::Events,
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
    pub object: *mut c_void,
    /// The `SSL` the record owns, freed with it: pointer-sized, null without one.
    ssl: Option<SocketSsl>,
    pub tls: SocketTlsState,
    pub timeouts: SocketTimeouts,
    pub last_received_time: f64,
    pub last_sent_time: f64,
    /// `Buffer *in_buffer`: owned as `output.out_buffer` is.
    pub in_buffer: Option<Box<swoole_rs_buffer::cpp::CppBuffer>>,
    /// `String *recv_buffer`: the server session's receive buffer, which the socket
    /// owns. Its address is read through `Box::as_mut_ptr`, which a reader holding it
    /// across a callback and a worker taking the buffer's bytes share.
    pub recv_buffer: Option<Box<swoole_rs_buffer::string::CppString>>,
    /// `TimerNode *recv_timer`, held by id.
    pub recv_timer: Option<swoole_rs_corebase::timer_key::Held>,
    pub total_recv_bytes: usize,
    pub total_send_bytes: usize,
    /// Last, so every offset before it is the same on each LP64 target.
    pub info: SocketAddress,
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

/// Field offsets native code asserts too (`swoole_rs_socket.h`).
#[cfg(target_pointer_width = "64")]
const OFFSETS: [(usize, usize); 15] = [
    (offset_of!(SocketFields, events), 12),
    (offset_of!(SocketFields, enable_tcp_nodelay), 16),
    (offset_of!(SocketFields, removed), 18),
    (offset_of!(SocketFields, close_notified), 35),
    (offset_of!(SocketFields, chunk_size), 36),
    (offset_of!(SocketFields, output), 40),
    (offset_of!(SocketFields, object), 64),
    (offset_of!(SocketFields, ssl), 72),
    (offset_of!(SocketFields, tls), 80),
    (offset_of!(SocketFields, timeouts), 88),
    (offset_of!(SocketFields, last_received_time), 120),
    (offset_of!(SocketFields, in_buffer), 136),
    (offset_of!(SocketFields, recv_timer), 152),
    (offset_of!(SocketFields, total_send_bytes), 168),
    (offset_of!(SocketFields, info), 176),
];

#[cfg(target_pointer_width = "64")]
#[expect(
    clippy::indexing_slicing,
    reason = "const-evaluated: an out-of-range index fails the build, and const has no get()"
)]
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
        size_of::<SocketFields>() == (176 + size_of::<SocketAddress>()).next_multiple_of(8),
        "network::Socket's size moved"
    );
    assert!(
        offset_of!(SocketRecord, borrows) == 296 && size_of::<SocketRecord>() == 304,
        "network::Socket's borrow count moved"
    );
};

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
                events: crate::Events::NONE,
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
            },
            borrows: AtomicU32::new(0),
        }
    }

    /// `make_socket(fd, fd_type)`'s fields.
    #[must_use]
    pub fn with_fd(fd: c_int, fd_type: c_int) -> Self {
        let mut socket = Self::new();
        socket.fd = fd;
        socket.fd_type = fd_type;
        socket.removed = true;
        socket
    }

    /// The record moved to the heap, as `new Socket()`; freed by its [`crate::OwnedSocket`].
    #[must_use]
    pub fn into_raw(self) -> *mut Self {
        Box::into_raw(Box::new(self))
    }

    /// A new heap record as the socket its maker holds: live until an owner taken
    /// back with the unsafe [`crate::OwnedSocket::from_socket`] frees it.
    #[must_use]
    pub fn into_socket(self: Box<Self>) -> crate::Socket<'static> {
        crate::Socket::new(Box::leak(self))
    }

    /// base.cc's `socket_free_defer`: [`SocketFields::release`], warning when close(2)
    /// failed, then the record goes.
    pub fn close_and_free(mut self: Box<Self>) {
        if !self.release() {
            let fd = self.fd;
            swoole_rs_corebase::log::sys_warning(
                c"socket_free_defer",
                format_args!("close({fd}) failed"),
            );
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
        let fd = crate::swr_v1_socket_open(domain, kind, protocol, flags).ok()?;
        let mut socket = Self::with_fd(fd, fd_type);
        socket.output.nonblock = flags & crate::socket_types::NONBLOCK != 0;
        socket.cloexec = flags & crate::socket_types::CLOEXEC != 0;
        socket.socket_type = socket_type;
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
        let Some((domain, kind)) = crate::socket_domain_and_type(socket_type) else {
            *unknown = true;
            crate::socket_sync::set_errno(libc::ESOCKTNOSUPPORT);
            return None;
        };
        Self::opened(socket_type, fd_type, domain, kind, 0, flags)
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

    /// The record's `SSL` out of it, or none: freed when the owner drops.
    /// # Safety
    /// No handle to the `SSL` or its BIOs is used once the owner drops, and no other
    /// record takes the owner (the `SSL`'s ex data may name this one).
    pub unsafe fn take_ssl(&mut self) -> Option<SocketSsl> {
        self.ssl.take()
    }

    /// The socket's type, `None` for a code no `SW_SOCK_*` names.
    #[must_use]
    pub fn kind(&self) -> Option<SocketType> {
        SocketType::from_raw(self.socket_type)
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
    pub fn catch_error(&mut self, error: c_int) -> crate::Caught {
        crate::catch_error(error, &mut self.kernel_nobufs)
    }

    /// `Socket::set_tcp_nodelay`.
    pub fn set_tcp_nodelay(&mut self, nodelay: c_int) -> bool {
        if !crate::swr_v1_socket_tcp_nodelay(self.fd, nodelay) {
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
        self.fd == -1 || sys::close(self.fd).is_ok()
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
                ..*self
            },
            borrows: AtomicU32::new(0),
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
        let mut socket = SocketRecord::new();
        socket.removed = true;
        socket.socket_type = self.socket_type;
        let listener_cloexec;
        (socket.fd, listener_cloexec) =
            crate::fd_accept(self.fd, &mut socket.info, self.output.nonblock);
        if listener_cloexec {
            self.cloexec = true;
        }
        if socket.fd < 0 {
            return None;
        }
        socket.info.kind = self.socket_type;
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
        socket.fd = -1;
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
        assert!(socket.object.is_null() && socket.output.out_buffer.is_none());
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
