//! `coroutine::Socket`'s record, which is the whole socket: made, opened, accepted,
//! released and freed here. Native `Socket` derives from the mirror
//! (`native/hooks/swoole_rs_co_socket.h`) and adds no fields; both sides assert the
//! offsets below. The `network::Socket` it wraps is the reactor's record, and the
//! strings, proxies, TLS context and destructor callback are its
//! [`SocketExtra`](crate::socket_extra::SocketExtra).
use crate::shutdown::SocketState;
use crate::socket_extra::SocketExtra;
use core::cell::{Cell, UnsafeCell};
use core::ffi::{CStr, c_char, c_int, c_void};
use core::mem::{MaybeUninit, offset_of, size_of};
use core::ptr;
use std::ffi::CString;
use swoole_rs_buffer::string::{Allocator, CppString, new_sized_string};
use swoole_rs_corebase::constants::{
    FdType, SW_BUFFER_SIZE_BIG, SW_INPUT_BUFFER_SIZE, SocketFlag, SocketType,
};
use swoole_rs_reactor::{Socket, SocketAddress, SocketFields, SocketRecord};
use swoole_rs_tls::upstream::{socket_free, socket_ssl_close};

pub use swoole_rs_corebase::net_types::Protocol;

/// `coroutine::Socket`'s scalar fields, each a cell: a coroutine parked in one member
/// shares the record with another running a second, and native code writes the fields
/// too, so the record is only ever borrowed shared and every access is one field's
/// load or store. Native objects (`String`, `Allocator`, `NameResolver::Context`) are
/// opaque here. The pointers are crate-private: other crates read them through [`Co`]
/// and set them only through its `unsafe` setters, so what `Co`'s safe methods
/// dereference stays live.
#[repr(C)]
pub struct CoSocket {
    pub err_code: Cell<c_int>,
    pub(crate) err_msg: Cell<*const c_char>,
    pub open_length_check: Cell<bool>,
    pub open_eof_check: Cell<bool>,
    pub http2: Cell<bool>,
    pub protocol: Cell<Protocol>,
    /// `swSocketType`.
    pub kind: Cell<c_int>,
    /// The `network::Socket`, none before the socket opens: the record's own from
    /// `into_socket`, live until `~Socket()` frees it, when no handle reaches this
    /// record any more.
    pub(crate) socket: Cell<Option<Socket<'static>>>,
    pub sock_domain: Cell<c_int>,
    pub sock_type: Cell<c_int>,
    pub sock_protocol: Cell<c_int>,
    pub sock_fd: Cell<c_int>,
    pub connect_port: Cell<c_int>,
    pub backlog: Cell<c_int>,
    /// The buffers' allocator: none for the default.
    pub(crate) buffer_allocator: Cell<Option<&'static Allocator>>,
    pub buffer_init_size: Cell<usize>,
    pub(crate) read_buffer: Cell<*mut c_void>,
    pub(crate) write_buffer: Cell<*mut c_void>,
    pub ssl_is_server: Cell<bool>,
    pub ssl_handshaked: Cell<bool>,
    pub zero_copy: Cell<bool>,
    pub(crate) resolve_context: Cell<*mut c_void>,
    /// The coroutines, barriers and timers bound to each direction, and the parts
    /// lent out, which `init` makes and `fini` releases.
    pub(crate) state: Option<Box<Bound>>,
    /// The record's metadata, which `init` makes and `fini` releases.
    pub(crate) extra: UnsafeCell<Option<Box<SocketExtra>>>,
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

/// How a constructor makes its `network::Socket` (`swr_v1_hook_co_socket_new`); the
/// discriminants are `swoole_rs_co_socket.h`'s `SWR_CO_SOCKET_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum New {
    /// `Socket(domain, type, protocol)`.
    ByFamily = 0,
    /// `Socket(SocketType)`.
    ByType = 1,
    /// `Socket(fd, SocketType)`.
    FdByType = 2,
    /// `Socket(fd, domain, type, protocol)`.
    FdByFamily = 3,
}

impl New {
    const fn from_code(code: c_int) -> Option<Self> {
        Some(match code {
            0 => Self::ByFamily,
            1 => Self::ByType,
            2 => Self::FdByType,
            3 => Self::FdByFamily,
            _ => return None,
        })
    }

    /// Whether it opens a new descriptor rather than adopting `fd`.
    const fn opens(self) -> bool {
        matches!(self, Self::ByFamily | Self::ByType)
    }
}

/// What a constructor did, for native code: `OPENED` set unless socket(2) failed
/// (errno holds why); `NODELAY_FAILED` when `TCP_NODELAY` could not be set (native
/// warns); bits 8 and up are `swr_v1_socket_fd_option`'s warnings from `set_nonblock`.
pub const OPENED: u32 = 1;
pub const NODELAY_FAILED: u32 = 2;
pub const FD_WARNINGS_SHIFT: u32 = 8;

/// A live coroutine socket record: the only way to reach one's fields. The handle is
/// `Copy`, and a coroutine parked in one member shares the record with another
/// running a second, so each read and write is of one field's cell and no borrow of
/// the record outlives a call.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Co(*mut CoSocket);

/// What a record's `state` points to: the bound state, first, so the pointer is the
/// state's too, and which parts of the record a [`Co`] `with_*` call has lent out.
#[repr(C)]
pub struct Bound {
    state: UnsafeCell<SocketState>,
    lent: Cell<u8>,
}

impl Bound {
    /// A new record's: nothing bound, nothing lent. `fini` frees it.
    fn new() -> Self {
        Self {
            state: UnsafeCell::new(SocketState::new()),
            lent: Cell::new(0),
        }
    }

    /// The bound state's address, which [`Co::with_state`] borrows.
    fn state(&self) -> *mut SocketState {
        self.state.get()
    }
}

/// The parts `with_*` lend as `&mut`, one bit each in [`Bound`]'s `lent`.
const STATE: u8 = 1;
const EXTRA: u8 = 2;
const READ_BUFFER: u8 = 4;
const WRITE_BUFFER: u8 = 8;

/// One part on loan, returned when dropped. The records are per thread, so a plain
/// cell does what `Socket::edit`'s count does: a second loan of the part panics.
struct Loan<'a>(&'a Cell<u8>, u8);

impl<'a> Loan<'a> {
    #[track_caller]
    fn take(lent: &'a Cell<u8>, part: u8) -> Self {
        assert!(
            lent.get() & part == 0,
            "a coroutine socket's borrow overlaps another of the same part"
        );
        lent.set(lent.get() | part);
        Self(lent, part)
    }
}

impl Drop for Loan<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() & !self.1);
    }
}

macro_rules! fields {
    (get $(#[$doc:meta])* $field:ident: $ty:ty) => {
        $(#[$doc])*
        #[must_use]
        pub fn $field(self) -> $ty {
            self.record().$field.get()
        }
    };
    ($($(#[$doc:meta])* $field:ident, $set:ident: $ty:ty;)*) => {$(
        fields!(get $(#[$doc])* $field: $ty);
        $(#[$doc])*
        pub fn $set(self, value: $ty) {
            self.record().$field.set(value);
        }
    )*};
}

/// The pointer fields, which `Co`'s methods and native code dereference: read freely,
/// set only by a caller that vouches for the new value.
macro_rules! pointers {
    ($($(#[$doc:meta])* $field:ident, $set:ident: $ty:ty;)*) => {$(
        fields!(get $(#[$doc])* $field: $ty);
        $(#[$doc])*
        ///
        /// # Safety
        /// `value` is what the field's doc says, for as long as the record holds it.
        pub unsafe fn $set(self, value: $ty) {
            self.record().$field.set(value);
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

    #[must_use]
    pub const fn raw(self) -> *mut CoSocket {
        self.0
    }

    /// The native object, which is the record.
    #[must_use]
    pub const fn this(self) -> *mut c_void {
        self.0.cast()
    }

    /// The record, borrowed shared for one access: its fields are cells, which native
    /// code and a coroutine in another member may write between two of them.
    #[inline]
    fn record(&self) -> &CoSocket {
        // SAFETY: `new`'s contract: a live record. Nothing borrows one mutably while a
        // handle reaches it: the constructors run before the first handle and
        // `~Socket()` after the last use.
        unsafe { &*self.0 }
    }

    fields! {
        err_code, set_err_code: c_int;
        /// The length and EOF protocol.
        protocol, set_protocol: Protocol;
        open_length_check, set_open_length_check: bool;
        open_eof_check, set_open_eof_check: bool;
        http2, set_http2: bool;
        kind, set_kind: c_int;
        sock_domain, set_sock_domain: c_int;
        sock_type, set_sock_type: c_int;
        sock_protocol, set_sock_protocol: c_int;
        sock_fd, set_sock_fd: c_int;
        connect_port, set_connect_port: c_int;
        backlog, set_backlog: c_int;
        buffer_init_size, set_buffer_init_size: usize;
        ssl_is_server, set_ssl_is_server: bool;
        ssl_handshaked, set_ssl_handshaked: bool;
        zero_copy, set_zero_copy: bool;
    }

    pointers! {
        /// `errMsg`: a NUL-terminated message, static, `swoole_strerror`'s, the thread's
        /// last error message or the record's own error string.
        err_msg, set_err_msg: *const c_char;
        /// Null, or a live `NameResolver::Context` that outlives the record.
        resolve_context, set_resolve_context: *mut c_void;
    }

    fields!(get
        /// Null, or the live `String` the record owns: a `new String`, a Box.
        read_buffer: *mut c_void);
    fields!(get
        /// Null, or the live `String` the record owns: a `new String`, a Box.
        write_buffer: *mut c_void);

    /// The buffers' allocator: none for the default.
    #[must_use]
    pub fn buffer_allocator(self) -> Option<&'static Allocator> {
        self.record().buffer_allocator.get()
    }

    /// The buffers' allocator from now on: none for the default.
    pub fn set_buffer_allocator(self, allocator: Option<&'static Allocator>) {
        self.record().buffer_allocator.set(allocator);
    }

    /// The `network::Socket`'s address: null before the socket opens.
    #[must_use]
    pub fn socket(self) -> *mut SocketRecord {
        self.record().socket_ptr()
    }

    /// A copy of `errMsg`, which the socket may replace or free; `None` when null.
    #[must_use]
    pub fn err_message(self) -> Option<CString> {
        self.with_err_message(|message| message.map(CStr::to_owned))
    }

    /// `errMsg = message`, a static message.
    pub fn set_static_err_msg(self, message: &'static CStr) {
        // A NUL-terminated message that lives for the process, as `err_msg` asks.
        self.record().err_msg.set(message.as_ptr());
    }

    /// `f` over a copy of `errMsg` (`None` when null), taken before `f` runs, so `f`
    /// may reach the socket again. A message that fits is copied to the stack: the
    /// clients read it after every send and receive.
    pub fn with_err_message<T>(self, f: impl FnOnce(Option<&CStr>) -> T) -> T {
        let message = self.err_msg();
        if message.is_null() {
            return f(None);
        }
        // SAFETY: `set_err_msg`'s contract: a NUL-terminated message the record holds.
        let message = unsafe { CStr::from_ptr(message) };
        let bytes = message.to_bytes_with_nul();
        let mut inline = [0u8; 128];
        if let Some(copy) = inline.get_mut(..bytes.len()) {
            copy.copy_from_slice(bytes);
            return f(CStr::from_bytes_with_nul(copy).ok());
        }
        let owned = message.to_owned();
        f(Some(&owned))
    }

    /// `f` over a copy of the protocol, written back after it: nothing borrows the
    /// record meanwhile, so `f` may reach the socket again.
    pub fn edit_protocol<T>(self, f: impl FnOnce(&mut Protocol) -> T) -> T {
        let mut protocol = self.protocol();
        let result = f(&mut protocol);
        self.set_protocol(protocol);
        result
    }

    /// The protocol's address, for the native length callbacks.
    #[must_use]
    pub fn protocol_ptr(self) -> *const Protocol {
        self.0
            .wrapping_byte_add(offset_of!(CoSocket, protocol))
            .cast()
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
        let made = new_sized_string(BUFFER_SIZE_BIG, self.buffer_allocator());
        // A `new String`, which the record owns now.
        self.buffer_slot(write).set(made.cast());
        made
    }

    /// The record's read or write buffer slot.
    fn buffer_slot(&self, write: bool) -> &Cell<*mut c_void> {
        let record = self.record();
        if write {
            &record.write_buffer
        } else {
            &record.read_buffer
        }
    }

    /// The read or write buffer, which the record gives up, as before its first use;
    /// none when it has none.
    /// # Panics
    /// Inside a `with_buffer` over the same buffer.
    #[track_caller]
    #[must_use]
    pub(crate) fn take_buffer(self, write: bool) -> Option<Box<CppString>> {
        let part = if write { WRITE_BUFFER } else { READ_BUFFER };
        self.lend(part, |_| {
            let buffer = self.buffer_slot(write).replace(ptr::null_mut());
            let buffer = buffer.cast::<CppString>();
            // SAFETY: null or the `new String` the record owned, which nothing else
            // reaches: the record no longer does and no `with_buffer` lends it.
            (!buffer.is_null()).then(|| unsafe { Box::from_raw(buffer) })
        })
    }

    /// [`Co::take_buffer`] of the write buffer. The read buffer is not given up
    /// outside this crate: a coroutine HTTP server reads requests out of it.
    /// # Panics
    /// Inside a `with_buffer` over the write buffer.
    #[track_caller]
    #[must_use]
    pub fn take_write_buffer(self) -> Option<Box<CppString>> {
        self.take_buffer(true)
    }

    /// The bound state's address; [`Co::with_state`] borrows it.
    #[must_use]
    pub fn state(self) -> *mut SocketState {
        let bound = self.record().state.as_deref();
        bound.map_or(ptr::null_mut(), Bound::state)
    }

    /// `f` with `part` lent out: the loan, of the [`Bound`] `new`'s contract keeps
    /// live, ends with the call.
    #[track_caller]
    fn lend<T>(self, part: u8, f: impl FnOnce(&Bound) -> T) -> T {
        let bound = self.record().state.as_deref();
        let bound =
            bound.unwrap_or_else(|| swoole_rs_corebase::fatal("a hooked socket without its state"));
        let _loan = Loan::take(&bound.lent, part);
        f(bound)
    }

    /// `f` over [`Co::buffer`]'s String.
    /// # Panics
    /// Inside another `with_buffer` over the same buffer.
    #[track_caller]
    pub fn with_buffer<T>(self, write: bool, f: impl FnOnce(&mut CppString) -> T) -> T {
        let part = if write { WRITE_BUFFER } else { READ_BUFFER };
        // SAFETY: the record's live String; the loan makes this its only reference.
        self.lend(part, |_| f(unsafe { &mut *self.buffer(write) }))
    }

    /// `f` over the `network::Socket`, or `None` before the socket opens: an
    /// [`edit`](Socket::edit), so a nested borrow of the same record panics.
    #[track_caller]
    pub fn net<T>(self, f: impl FnOnce(&mut SocketFields) -> T) -> Option<T> {
        self.with_network(|network| network.edit(f))
    }

    /// `f` over the `network::Socket`'s handle, or `None` before the socket opens. The
    /// handle lives only for the call, so it cannot outlive the record.
    pub fn with_network<T>(self, f: impl FnOnce(Socket<'_>) -> T) -> Option<T> {
        self.record().socket.get().map(f)
    }

    /// `Reactor::change`'s add (`add`) or set of the open socket for `events`, on the
    /// thread's reactor: false without a socket or a reactor. The record's own
    /// network socket, which `~Socket()` takes off the reactor as it frees it.
    #[must_use]
    pub fn register(self, add: bool, events: c_int) -> bool {
        use swoole_rs_reactor::event_api::{event_add, event_set};
        let change = if add { event_add } else { event_set };
        self.record()
            .socket
            .get()
            .is_some_and(|network| change(network, events))
    }

    /// `Reactor::interest(socket, kind, 0)` of the open socket on the thread's reactor,
    /// registered as [`Co::register`] registers it: whether the change worked, true when
    /// nothing changes, false without a socket or a reactor.
    #[must_use]
    pub fn interest(self, kind: swoole_rs_reactor::Interest) -> bool {
        let interest = swoole_rs_reactor::event_api::event_interest;
        self.record()
            .socket
            .get()
            .is_some_and(|network| interest(network, kind, 0))
    }

    /// The record's metadata, `None` once released.
    /// # Panics
    /// Inside another `with_extra` over the same record.
    #[track_caller]
    pub fn with_extra<T>(self, f: impl FnOnce(&mut SocketExtra) -> T) -> Option<T> {
        self.lend(EXTRA, |_| {
            // SAFETY: the live record's metadata, which the loan makes this its only
            // reference to.
            let extra = unsafe { &mut *self.record().extra.get() };
            extra.as_deref_mut().map(f)
        })
    }

    /// The bound coroutines, barriers and timers.
    /// # Panics
    /// Inside another `with_state` over the same record.
    #[track_caller]
    pub fn with_state<T>(self, f: impl FnOnce(&mut SocketState) -> T) -> T {
        // SAFETY: the live state; the loan makes this its only reference.
        self.lend(STATE, |bound| f(unsafe { &mut *bound.state.get() }))
    }

    /// `errno` classified by the open socket, as `catch_read_error`/`catch_write_error`
    /// say; `None` without one.
    #[must_use]
    pub fn catch_error(self, error: c_int) -> Option<swoole_rs_reactor::Caught> {
        self.net(|record| record.catch_error(error))
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
        self.sock_fd.get() == -1
    }

    /// The `network::Socket`'s address: null before the socket opens.
    pub(crate) fn socket_ptr(&self) -> *mut SocketRecord {
        self.socket.get().map_or(ptr::null_mut(), Socket::as_ptr)
    }

    /// The handle of a record this module boxed, which the caller owns from here:
    /// [`swr_v1_hook_co_socket_free`] releases it.
    fn into_co(self: Box<Self>) -> Co {
        // SAFETY: a live record until it is freed, with the state and metadata `new`
        // made; its `socket` is null or the network::Socket this module attached.
        unsafe { Co::new(Box::into_raw(self)) }
    }

    fn new() -> Self {
        Self {
            err_code: Cell::new(0),
            err_msg: Cell::new(c"".as_ptr()),
            open_length_check: Cell::new(false),
            open_eof_check: Cell::new(false),
            http2: Cell::new(false),
            protocol: Cell::new(Protocol::EMPTY),
            kind: Cell::new(0),
            socket: Cell::new(None),
            sock_domain: Cell::new(0),
            sock_type: Cell::new(0),
            sock_protocol: Cell::new(0),
            sock_fd: Cell::new(-1),
            connect_port: Cell::new(0),
            backlog: Cell::new(0),
            buffer_allocator: Cell::new(None),
            buffer_init_size: Cell::new(BUFFER_SIZE_BIG),
            read_buffer: Cell::new(ptr::null_mut()),
            write_buffer: Cell::new(ptr::null_mut()),
            ssl_is_server: Cell::new(false),
            ssl_handshaked: Cell::new(false),
            zero_copy: Cell::new(false),
            resolve_context: Cell::new(ptr::null_mut()),
            state: Some(Box::new(Bound::new())),
            extra: UnsafeCell::new(Some(SocketExtra::boxed())),
        }
    }

    /// `init_sock`: a nonblocking, close-on-exec socket of the record's type.
    fn open(&self, owner: *mut c_void) -> Option<Socket<'static>> {
        let socket = SocketRecord::opened(
            self.kind.get(),
            FD_CO_SOCKET,
            self.sock_domain.get(),
            self.sock_type.get(),
            self.sock_protocol.get(),
            OPEN_FLAGS,
        );
        self.attach(socket, owner)
    }

    /// The fields `init_sock` and `init_reactor_socket` set on a new socket, which
    /// this record owns from here; `None`, with the record left alone, for none.
    fn attach(
        &self,
        socket: Option<Box<SocketRecord>>,
        owner: *mut c_void,
    ) -> Option<Socket<'static>> {
        let network = socket?.into_socket();
        self.socket.set(Some(network));
        self.sock_fd.set(network.edit(|record| {
            record.object = owner;
            record.info.kind = self.kind.get();
            record.fd
        }));
        Some(network)
    }

    /// `init_reactor_socket`: wrap `fd`, which the caller made nonblocking.
    fn adopt(&self, fd: c_int, owner: *mut c_void) -> Option<Socket<'static>> {
        let mut record = SocketRecord::with_fd(fd, FD_CO_SOCKET);
        record.socket_type = self.kind.get();
        record.output.nonblock = true;
        record.cloexec = true;
        self.attach(Some(Box::new(record)), owner)
    }

    /// `init_options`: `TCP_NODELAY` for TCP, the default buffer size for UDP, and
    /// the default length protocol.
    fn init_options(&mut self, record: &SocketFields) -> u32 {
        let (tcp, fd, socket_type) = (record.is_tcp(), record.fd, record.socket_type);
        let mut result = 0;
        if tcp {
            let on: c_int = 1;
            let set =
                swoole_rs_corebase::sys::setsockopt(fd, libc::IPPROTO_TCP, libc::TCP_NODELAY, &on);
            if set < 0 {
                result |= NODELAY_FAILED;
            }
        }
        if matches!(socket_type, 2 | 4) {
            let size = swoole_rs_reactor::SOCKET_DEFAULTS.buffer_size();
            swoole_rs_reactor::swr_v1_socket_buffer_size(fd, true, true, size);
        }
        let protocol = self.protocol.get_mut();
        protocol.package_length_type = b'N';
        protocol.package_length_size = 4;
        protocol.package_length_offset = 0;
        protocol.package_body_offset = 0;
        protocol.package_max_length = SW_INPUT_BUFFER_SIZE;
        result
    }
}

/// `Socket::set_nonblock`: the `fd_option` warnings, shifted for the result.
fn set_nonblock(network: Socket<'_>) -> u32 {
    let result = swoole_rs_reactor::swr_v1_socket_fd_option(network.fd(), Some(true), None);
    if result.ok {
        network.edit(|record| {
            record.output.nonblock = true;
            record.cloexec = true;
        });
    }
    result.warnings << FD_WARNINGS_SHIFT
}

#[cfg(test)]
use swoole_rs_corebase::sys::socklen;

/// `domain` and `kind` for `socket_type`, left alone for a type with none.
fn domain_and_type(socket_type: c_int, domain: &mut c_int, kind: &mut c_int) {
    swoole_rs_reactor::swr_v1_socket_domain_and_type(socket_type, domain, kind);
}

/// `~Socket()`: the destructor callback if it has not run, the buffers, the
/// `network::Socket` (its TLS session closed first), then the bound state and the
/// metadata, whose TLS context outlived the session.
///
/// # Safety
/// `socket` is a live record, not used afterwards; its buffers came from `new String`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_release(socket: *mut CoSocket) {
    // SAFETY: as the caller promises.
    let co = unsafe { Co::new(socket) };
    // The callback runs before anything is freed.
    crate::socket_extra::run_close_hook(co);
    for write in [false, true] {
        // `delete` of the socket's own `new String`.
        if let Some(mut buffer) = co.take_buffer(write) {
            buffer.dispose();
        }
    }
    co.with_network(|network| {
        if !network.read(|record| record.ssl().is_null()) {
            // SAFETY: handles to a session live only for the operation that took them,
            // and the socket being released runs none.
            unsafe { socket_ssl_close(network) };
        }
    });
    let network = co.socket();
    if !network.is_null() {
        // SAFETY: the live record, which nothing uses afterwards.
        unsafe { socket_free(network) };
    }
    // SAFETY: as the caller promises; no handle reaches the record any more.
    unsafe { &mut *socket }.fini();
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
        (true, true) => New::ByFamily,
        (true, false) => New::FdByFamily,
        (false, true) => New::ByType,
        (false, false) => New::FdByType,
    };
    let mut socket = Box::new(CoSocket::new());
    let owner = ptr::from_mut(&mut *socket).cast();
    let result = socket.construct(owner, how, fd, kind, domain, kind, protocol);
    let socket = socket.into_co();
    if crate::stdio::host().is_some_and(|host| host.uring_socket) {
        crate::socket_extra::swr_v1_hook_co_socket_uring(socket);
    }
    (socket, result)
}

/// The warnings [`make_quiet`]'s `result` holds, over its socket.
pub fn warn_created(socket: Co, result: u32) {
    swr_v1_hook_co_socket_created(socket, result);
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

/// The public constructors, which initialise the record first, by the [`New`] whose
/// code `how` is (0, nothing opened, for any other). `owner` is the native
/// `Socket`, which the new `network::Socket` points back to. Returns the bits above.
/// `storage` holds one record, which needn't be initialized yet (a record already
/// there is overwritten, not dropped); the native object owns it for its lifetime.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_co_socket_new(
    socket: &mut MaybeUninit<CoSocket>,
    owner: *mut c_void,
    how: c_int,
    fd: c_int,
    socket_type: c_int,
    domain: c_int,
    kind: c_int,
    protocol: c_int,
) -> u32 {
    let record = socket.write(CoSocket::new());
    let Some(how) = New::from_code(how) else {
        return 0;
    };
    record.construct(owner, how, fd, socket_type, domain, kind, protocol)
}

impl CoSocket {
    /// Release the bound state and the metadata [`CoSocket::new`] made, as a
    /// constructor's undo or the end of `~Socket()`.
    pub(crate) fn fini(&mut self) {
        self.state = None;
        *self.extra.get_mut() = None;
    }

    /// The public constructors' work over a fresh record (see
    /// [`swr_v1_hook_co_socket_new`]).
    #[expect(clippy::too_many_arguments, reason = "the constructors' union")]
    fn construct(
        &mut self,
        owner: *mut c_void,
        how: New,
        fd: c_int,
        socket_type: c_int,
        domain: c_int,
        kind: c_int,
        protocol: c_int,
    ) -> u32 {
        let socket = self;
        let mut result = OPENED;
        match how {
            New::ByFamily | New::FdByFamily => {
                socket.sock_domain.set(domain);
                socket.sock_type.set(kind);
                socket.sock_protocol.set(protocol);
                socket
                    .kind
                    .set(swoole_rs_reactor::swr_v1_socket_type(domain, kind));
            }
            New::ByType | New::FdByType => {
                socket.kind.set(socket_type);
                domain_and_type(
                    socket_type,
                    socket.sock_domain.get_mut(),
                    socket.sock_type.get_mut(),
                );
            }
        }
        let opened = if how.opens() {
            socket.open(owner)
        } else {
            socket.adopt(fd, owner)
        };
        let Some(network) = opened else {
            return 0;
        };
        if !how.opens() {
            if how == New::FdByType && socket_type == SOCK_RAW {
                return result;
            }
            result |= set_nonblock(network);
        }
        result | network.read(|record| socket.init_options(record))
    }
}

/// `reinit_sock(type)`: a new socket of `socket_type` replaces the record's, whose
/// old `network::Socket` is returned for the caller to free. `None`, with errno set
/// and nothing changed, if socket(2) failed.
pub(crate) fn reinit(socket: Co, socket_type: c_int) -> Option<*mut SocketRecord> {
    socket.record().reinit(socket.this(), socket_type)
}

impl CoSocket {
    fn reinit(&self, owner: *mut c_void, socket_type: c_int) -> Option<*mut SocketRecord> {
        let socket = self;
        let (mut domain, mut kind) = (0, 0);
        domain_and_type(socket_type, &mut domain, &mut kind);
        let made = SocketRecord::opened(
            socket_type,
            FD_CO_SOCKET,
            domain,
            kind,
            socket.sock_protocol.get(),
            OPEN_FLAGS,
        );
        let old = socket.socket_ptr();
        // Upstream records the old type on the new socket before taking the new one.
        socket.attach(made, owner)?;
        socket.sock_domain.set(domain);
        socket.sock_type.set(kind);
        socket.kind.set(socket_type);
        Some(old)
    }
}

/// `new Socket(conn, server)`, the accepting constructor: `conn`, accepted on
/// `server`, with the server's type, framing and timeouts, connected; `accept`
/// shares the TLS context after. [`swr_v1_hook_co_socket_free`] releases it.
pub(crate) fn accepted(server: Co, mut conn: Box<SocketRecord>) -> Co {
    let mut socket = Box::new(CoSocket::new());
    let owner = ptr::from_mut(&mut *socket).cast::<c_void>();
    let timeouts = server.net(|record| record.timeouts);
    let kind = server.kind();
    socket.kind.set(kind);
    socket.sock_domain.set(server.sock_domain());
    socket.sock_type.set(server.sock_type());
    socket.sock_protocol.set(server.sock_protocol());
    // The accepted record, still this function's own: the new socket owns it below.
    conn.object = owner;
    conn.socket_type = kind;
    conn.fd_type = FD_CO_SOCKET;
    socket.sock_fd.set(conn.fd);
    let result = OPENED | socket.init_options(&conn);
    if let Some(timeouts) = timeouts {
        conn.timeouts = timeouts;
    }
    socket.socket.set(Some(conn.into_socket()));
    socket.open_length_check.set(server.open_length_check());
    socket.open_eof_check.set(server.open_eof_check());
    socket.http2.set(server.http2());
    socket.protocol.set(server.protocol());
    socket.ssl_is_server.set(server.ssl_is_server());
    let socket = socket.into_co();
    swr_v1_hook_co_socket_created(socket, result);
    socket.with_state(|state| state.flags.connected = true);
    socket
}

/// What a constructor's bits report (`Socket::created`): a failed socket(2) becomes
/// the last error; the fd option and `TCP_NODELAY` failures are warnings.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_co_socket_created(socket: Co, result: u32) {
    use swoole_rs_corebase::log;
    if result & OPENED == 0 {
        log::set_last_error(crate::co_io::errno());
        return;
    }
    let fd = socket.sock_fd();
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
    if address.peer_name(socket.sock_fd()) != 0 {
        return std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    }
    address.kind = socket.kind();
    0
}

/// The tests' records, here and in the operations' modules.
#[cfg(test)]
mod test_support {
    use super::*;

    impl CoSocket {
        /// The `network::Socket` record.
        ///
        /// # Safety
        /// The socket is open (constructed with a `network::Socket`) and no other
        /// reference to that record is live.
        #[must_use]
        #[expect(
            clippy::mut_from_ref,
            reason = "the network::Socket record is its own allocation, which this record only points to"
        )]
        pub unsafe fn record(&self) -> &mut SocketFields {
            // SAFETY: as the caller promises.
            unsafe { &mut (*self.socket_ptr()).fields }
        }
    }

    /// Writes a fresh record (not open, nothing bound) into `socket`'s storage.
    pub(crate) unsafe fn init(socket: *mut CoSocket) {
        unsafe { socket.write(CoSocket::new()) };
    }

    /// An open record over a `network::Socket` record without a descriptor, for the
    /// operations' tests.
    pub(crate) struct TestSocket(pub Box<CoSocket>);

    impl TestSocket {
        pub(crate) fn new() -> Self {
            let mut storage = Box::new(core::mem::MaybeUninit::<CoSocket>::uninit());
            unsafe { init(storage.as_mut_ptr()) };
            let socket = unsafe { Box::from_raw(Box::into_raw(storage).cast::<CoSocket>()) };
            socket.socket.set(Some(
                Box::new(SocketRecord::with_fd(-1, FD_CO_SOCKET)).into_socket(),
            ));
            socket.sock_fd.set(100);
            Self(socket)
        }

        pub(crate) fn ptr(&mut self) -> *mut CoSocket {
            &raw mut *self.0
        }

        pub(crate) fn co(&mut self) -> Co {
            unsafe { Co::new(self.ptr()) }
        }

        pub(crate) fn state(&self) -> *mut SocketState {
            self.0
                .state
                .as_deref()
                .map_or(ptr::null_mut(), Bound::state)
        }
    }

    impl Drop for TestSocket {
        fn drop(&mut self) {
            unsafe {
                for buffer in [self.0.read_buffer.get(), self.0.write_buffer.get()] {
                    if !buffer.is_null() {
                        swoole_rs_buffer::string::delete_string(buffer.cast());
                    }
                }
                SocketRecord::free(self.0.socket_ptr());
                self.0.fini();
            }
        }
    }
}
#[cfg(test)]
pub(crate) use test_support::{TestSocket, init};

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
        if !socket.socket_ptr().is_null() {
            unsafe {
                libc::close((*socket.socket_ptr()).fields.fd);
                SocketRecord::free(socket.socket_ptr());
            }
        }
        socket.fini();
        assert!(socket.state.is_none());
    }

    fn new(socket: &mut CoSocket, how: New, fd: c_int, ty: c_int, d: c_int, k: c_int) -> u32 {
        new_by_code(socket, how as c_int, fd, ty, d, k)
    }

    /// [`swr_v1_hook_co_socket_new`] with a raw `how`, as native code passes it.
    fn new_by_code(
        socket: &mut CoSocket,
        how: c_int,
        fd: c_int,
        ty: c_int,
        d: c_int,
        k: c_int,
    ) -> u32 {
        let owner = (&raw mut *socket).cast();
        // The constructor writes the record again.
        socket.fini();
        let storage = unsafe { &mut *(&raw mut *socket).cast() };
        swr_v1_hook_co_socket_new(storage, owner, how, fd, ty, d, k, 0)
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
        assert_eq!(socket.sock_fd.get(), -1);
        assert_eq!(socket.buffer_init_size.get(), 65536);
        assert_eq!(unsafe { *socket.err_msg.get() }, 0);
        assert!(socket.state.is_some());
        assert_eq!(socket.protocol.get().package_length_type, 0);
        release(socket);
    }

    #[test]
    fn err_message_is_a_copy_of_short_and_long_messages_and_none_when_null() {
        let mut socket = record();
        let co = unsafe { Co::new(&raw mut *socket) };
        let long = CString::new([b'x'; 300]).unwrap();
        for message in [c"Success", long.as_c_str()] {
            unsafe { co.set_err_msg(message.as_ptr()) };
            assert_eq!(co.err_message().as_deref(), Some(message));
            co.with_err_message(|copy| {
                // The socket may replace its message while `f` runs.
                unsafe { co.set_err_msg(c"replaced".as_ptr()) };
                assert_eq!(copy, Some(message));
            });
        }
        unsafe { co.set_err_msg(ptr::null()) };
        assert_eq!(co.err_message(), None);
        unsafe { co.set_err_msg(c"".as_ptr()) };
        release(socket);
    }

    #[test]
    fn a_tcp_socket_by_type_is_open_nonblocking_with_nodelay_and_the_length_protocol() {
        let mut socket = record();
        assert_eq!(new(&mut socket, New::ByType, 0, 1, 0, 0), OPENED);
        assert_eq!(
            (socket.sock_domain.get(), socket.sock_type.get()),
            (libc::AF_INET, libc::SOCK_STREAM)
        );
        let fd = socket.sock_fd.get();
        let record = unsafe { &*socket.socket_ptr() };
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
        assert!(record.output.nonblock);
        assert_ne!(
            unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_NONBLOCK,
            0
        );
        assert_ne!(option(fd, libc::IPPROTO_TCP, libc::TCP_NODELAY), 0);
        assert_eq!(socket.protocol.get_mut().package_length_type, b'N');
        assert_eq!(socket.protocol.get_mut().package_length_size, 4);
        assert_eq!(
            socket.protocol.get_mut().package_max_length,
            2 * 1024 * 1024
        );
        release(socket);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no SOCK_DGRAM sockets")]
    fn a_family_maps_to_its_type() {
        let mut socket = record();
        assert_eq!(
            new(
                &mut socket,
                New::ByFamily,
                0,
                0,
                libc::AF_INET,
                libc::SOCK_DGRAM
            ),
            OPENED
        );
        assert_eq!(socket.kind.get(), 2);
        assert_eq!(unsafe { (*socket.socket_ptr()).fields.socket_type }, 2);
        release(socket);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "Miri aborts on a socket domain other than AF_INET and AF_INET6"
    )]
    fn an_unknown_type_fails_to_open() {
        let mut socket = record();
        assert_eq!(new(&mut socket, New::ByType, 0, 99, 0, 0), 0);
        assert!(socket.socket_ptr().is_null());
        assert_eq!(socket.sock_fd.get(), -1);
        release(socket);
    }

    /// An unknown constructor code from native code neither opens a socket nor adopts
    /// the fd it was given.
    #[test]
    fn an_unknown_constructor_kind_opens_and_adopts_nothing() {
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        for how in [-1, 4] {
            let mut socket = record();
            assert_eq!(new_by_code(&mut socket, how, fd, 1, 0, 0), 0, "{how}");
            assert!(socket.socket_ptr().is_null(), "{how}: no network socket");
            assert_eq!(socket.sock_fd.get(), -1, "{how}: no fd");
            release(socket);
        }
        // Still the caller's, untouched: blocking and open.
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_NONBLOCK,
            0
        );
        assert_eq!(unsafe { libc::close(fd) }, 0);
    }

    #[test]
    fn an_adopted_fd_is_made_nonblocking_but_a_raw_one_is_not() {
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        let mut socket = record();
        assert_eq!(
            new(
                &mut socket,
                New::FdByFamily,
                fd,
                0,
                libc::AF_INET,
                libc::SOCK_STREAM
            ),
            OPENED
        );
        assert_eq!(socket.sock_fd.get(), fd);
        assert_ne!(
            unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_NONBLOCK,
            0
        );
        assert_eq!(unsafe { (*socket.socket_ptr()).fields.fd_type }, 6);
        release(socket);

        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        let mut socket = record();
        assert_eq!(new(&mut socket, New::FdByType, fd, SOCK_RAW, 0, 0), OPENED);
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_NONBLOCK,
            0
        );
        assert_eq!(socket.protocol.get_mut().package_length_type, 0);
        release(socket);
    }

    /// `net` borrows through the reactor's count, so a nested borrow of one record
    /// panics instead of aliasing it.
    #[test]
    #[should_panic(expected = "overlaps a read or edit of the same record")]
    fn a_nested_net_of_one_record_panics() {
        let mut socket = TestSocket::new();
        let co = socket.co();
        co.net(|_| co.net(|_| ()));
    }

    #[test]
    fn reinit_swaps_the_socket_and_hands_back_the_old_one() {
        let mut socket = record();
        new(&mut socket, New::ByType, 0, 1, 0, 0);
        let first = socket.socket_ptr();
        let old = reinit(unsafe { Co::new(&raw mut *socket) }, 3).unwrap();
        assert_eq!(old, first);
        assert_eq!(
            (socket.kind.get(), socket.sock_domain.get()),
            (3, libc::AF_INET6)
        );
        assert_eq!(unsafe { (*socket.socket_ptr()).fields.info.kind }, 1);
        unsafe {
            libc::close((*old).fields.fd);
            SocketRecord::free(old);
        }
        release(socket);
    }

    #[test]
    fn an_accepted_socket_inherits_the_servers_settings() {
        let mut server = record();
        new(&mut server, New::ByType, 0, 1, 0, 0);
        server.open_eof_check.set(true);
        server.http2.set(true);
        server.ssl_is_server.set(true);
        server.protocol.get_mut().package_eof_len = 2;
        unsafe { (*server.socket_ptr()).fields.timeouts.read = 1.5 };
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        let conn = Box::new(SocketRecord::with_fd(fd, 0));
        let socket = accepted(unsafe { Co::new(&raw mut *server) }, conn);
        let socket = unsafe { Box::from_raw(socket.raw()) };
        let conn = socket.socket_ptr();
        assert_eq!(
            unsafe { (*conn).fields.object },
            (&raw const *socket).cast_mut().cast()
        );
        assert_eq!((socket.kind.get(), socket.sock_fd.get()), (1, fd));
        assert!(socket.open_eof_check.get() && socket.http2.get() && socket.ssl_is_server.get());
        assert_eq!(socket.protocol.get().package_eof_len, 2);
        let record = unsafe { &*conn };
        assert_eq!((record.fd_type, record.socket_type), (6, 1));
        assert!((record.timeouts.read - 1.5).abs() < f64::EPSILON);
        assert!(
            socket
                .state
                .as_deref()
                .is_some_and(|bound| unsafe { (*bound.state()).flags.connected })
        );
        release(socket);
        release(server);
    }

    #[test]
    fn peer_name_of_an_unconnected_socket_is_an_error() {
        let mut socket = record();
        new(&mut socket, New::ByType, 0, 1, 0, 0);
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
        let mut socket = TestSocket::new();
        swoole_rs_corebase::log::set_last_error(0);
        crate::co_io::set_errno(libc::EMFILE);
        swr_v1_hook_co_socket_created(socket.co(), 0);
        assert_eq!(swoole_rs_corebase::log::last_error(), libc::EMFILE);
        swoole_rs_corebase::log::set_last_error(0);
        swr_v1_hook_co_socket_created(
            socket.co(),
            OPENED | NODELAY_FAILED | (3 << FD_WARNINGS_SHIFT),
        );
        assert_eq!(swoole_rs_corebase::log::last_error(), 0);
    }

    /// `Co` is `Copy`, so safe code can nest two borrows of one part of a record; the
    /// inner one panics instead of aliasing the outer `&mut` (#3178).
    #[test]
    #[should_panic(expected = "overlaps")]
    fn nested_state_borrow_panics() {
        let mut socket = TestSocket::new();
        let co = socket.co();
        co.with_state(|outer| co.with_state(|inner| inner.want = outer.want + 1));
    }

    #[test]
    #[should_panic(expected = "overlaps")]
    fn nested_extra_borrow_panics() {
        let mut socket = TestSocket::new();
        let co = socket.co();
        co.with_extra(|outer| {
            co.with_extra(|inner| inner.proxies = core::mem::take(&mut outer.proxies))
        });
    }

    #[test]
    #[should_panic(expected = "overlaps")]
    fn nested_buffer_borrow_panics() {
        let mut socket = TestSocket::new();
        let co = socket.co();
        co.with_buffer(false, |outer| {
            co.with_buffer(false, |inner| inner.size() + outer.size())
        });
    }

    /// Borrows of different parts don't overlap, and a borrow ends with its call.
    #[test]
    fn distinct_and_sequential_borrows() {
        let mut socket = TestSocket::new();
        let co = socket.co();
        co.with_state(|state| {
            co.with_buffer(false, |read| {
                co.with_buffer(true, |write| read.size() + write.size())
            });
            co.with_extra(|extra| extra.proxies.each_ref().map(Option::is_some));
            state.want = 1;
        });
        assert_eq!(co.with_state(|state| state.want), 1);
    }
}
