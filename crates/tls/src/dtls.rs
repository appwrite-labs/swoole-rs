//! `dtls::Session`: its datagram queue, the BIO OpenSSL reads it through, and
//! `init`/`listen`, independent of native `SSLContext` lifetime.
use crate::handle::CText;
use core::cell::{Cell, RefCell};
use core::ffi::{c_char, c_int, c_long, c_void};
use core::marker::{PhantomData, PhantomPinned};
use core::ptr;
use openssl_sys::{BIO_METHOD, BIO_meth_free, SSL_set_bio};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicPtr, Ordering};
use swoole_rs_corebase::log;
use swoole_rs_reactor::{Socket, SocketRecord};

#[repr(C)]
struct Buffer {
    length: u16,
    data: [u8; 0],
}

/// One queued datagram: an allocation that begins with an initialized `Buffer` header,
/// as `swr_v1_tls_dtls_append` makes and `swr_v1_tls_dtls_append_buffer`'s caller promises.
struct Datagram(*mut Buffer);
impl Datagram {
    /// The payload's length, from the header.
    fn length(&self) -> u16 {
        // SAFETY: the datagram owns its allocation, whose header is initialized.
        unsafe { (*self.0).length }
    }
}

/// Where a datagram's payload begins: right after its header. Computing it reads nothing.
fn payload(buffer: *mut Buffer) -> *mut u8 {
    buffer
        .wrapping_byte_add(core::mem::offset_of!(Buffer, data))
        .cast()
}
impl Drop for Datagram {
    fn drop(&mut self) {
        // SAFETY: each queued datagram owns one sw_malloc/libc-compatible allocation.
        unsafe { libc::free(self.0.cast()) };
    }
}

pub struct DtlsSession {
    socket: *mut SocketRecord,
    listened: Cell<bool>,
    peek: Cell<bool>,
    queue: RefCell<VecDeque<Datagram>>,
}

impl DtlsSession {
    /// The session's socket.
    #[must_use]
    pub fn socket(&self) -> *mut SocketRecord {
        self.socket
    }

    /// Whether `listen` has published the session.
    #[must_use]
    pub fn listened(&self) -> bool {
        self.listened.get()
    }
}

#[repr(C)]
pub struct DtlsReadView {
    pub length: u16,
    pub empty: bool,
    pub peek: bool,
}

/// `new dtls::Session(socket, ctx)` before `init`: an empty queue over `socket`.
#[must_use]
pub fn swr_v1_tls_dtls_new(socket: *mut SocketRecord) -> *mut DtlsSession {
    Box::into_raw(Box::new(DtlsSession {
        socket,
        listened: Cell::new(false),
        peek: Cell::new(false),
        queue: RefCell::new(VecDeque::new()),
    }))
}

/// # Safety
/// owner came from `swr_v1_tls_dtls_new` and is returned once, when nothing uses it.
pub unsafe extern "C" fn swr_v1_tls_dtls_free(owner: *mut DtlsSession) {
    if !owner.is_null() {
        // SAFETY: the caller returns its unique allocation.
        unsafe { drop(Box::from_raw(owner)) };
    }
}

/// # Safety
/// buffer is an exclusively transferred sw_malloc-compatible datagram, its header
/// initialized.
/// On allocation failure ownership remains with the caller.
pub unsafe fn swr_v1_tls_dtls_append_buffer(owner: &DtlsSession, buffer: *mut c_void) -> c_int {
    let mut queue = owner.queue.borrow_mut();
    if queue.try_reserve(1).is_err() {
        return -1;
    }
    queue.push_back(Datagram(buffer.cast()));
    0
}

/// A copy of `data` queued as one datagram.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the public native Buffer length is uint16_t"
)]
pub fn swr_v1_tls_dtls_append(owner: &DtlsSession, data: &[u8]) -> c_int {
    let length = data.len();
    let mut queue = owner.queue.borrow_mut();
    if queue.try_reserve(1).is_err() {
        return -1;
    }
    let Some(size) = core::mem::size_of::<Buffer>().checked_add(length) else {
        return -1;
    };
    // SAFETY: malloc accepts any size and returns null on failure.
    let buffer = unsafe { libc::malloc(size) }.cast::<Buffer>();
    if buffer.is_null() {
        return -1;
    }
    let length = length as u16;
    // SAFETY: malloc provided at least the Buffer header plus length payload bytes.
    unsafe { (*buffer).length = length };
    let out = payload(buffer);
    if length > 0 {
        // SAFETY: the source covers `length` bytes, and malloc provided as many after the
        // header, which fresh storage cannot overlap.
        unsafe { ptr::copy_nonoverlapping(data.as_ptr(), out, usize::from(length)) };
    }
    queue.push_back(Datagram(buffer));
    0
}

/// The front datagram's length, whether the queue is empty, and `peek_mode`.
pub fn swr_v1_tls_dtls_view(owner: &DtlsSession) -> DtlsReadView {
    let queue = owner.queue.borrow();
    let peek = owner.peek.get();
    let length = queue.front().map_or(0, Datagram::length);
    DtlsReadView {
        length,
        empty: queue.is_empty(),
        peek,
    }
}

/// # Safety
/// data covers requested writable bytes for a nonnegative request.
pub unsafe fn swr_v1_tls_dtls_read(
    owner: &DtlsSession,
    data: *mut u8,
    requested: c_int,
) -> crate::ReadPlan {
    let peek = owner.peek.get();
    let mut queue = owner.queue.borrow_mut();
    let Some(front) = queue.front() else {
        return crate::ReadPlan {
            length: -1,
            consume: 0,
            retry: 1,
        };
    };
    let length = front.length();
    let plan = crate::read_plan(0, length, requested, u8::from(peek));
    if let Ok(length) = usize::try_from(plan.length) {
        let source = payload(front.0);
        if length > 0 {
            // SAFETY: the requested output is writable and the owned front's payload, after
            // its header, covers length bytes; memmove permits overlap.
            unsafe { ptr::copy(source, data, length) };
        }
        if plan.consume != 0 {
            queue.pop_front();
        }
    }
    plan
}

/// The queued datagrams' total length.
pub fn swr_v1_tls_dtls_length(owner: &DtlsSession) -> usize {
    owner.queue.borrow().iter().fold(0usize, |total, buffer| {
        total.wrapping_add(usize::from(buffer.length()))
    })
}

/// Native decodes its actual OpenSSL macro values into the existing stable tags.
/// Shutdown and diagnostic effects are returned to native, outside this Rust frame.
/// # Safety
/// ptrval is null or writable for one address union for the peer command.
#[expect(
    clippy::cast_possible_wrap,
    reason = "native BIO_CTRL_PENDING casts wrapping size_t total to long"
)]
pub unsafe fn swr_v1_tls_dtls_control(
    owner: &DtlsSession,
    tag: u32,
    observed: c_long,
    value: c_long,
    ptrval: *mut c_void,
) -> crate::ControlPlan {
    let observed = match tag {
        0 => c_long::from(owner.queue.borrow().is_empty()),
        2 => swr_v1_tls_dtls_length(owner) as c_long,
        _ => observed,
    };
    if tag == 8 {
        owner.peek.set(value != 0);
        return crate::ControlPlan {
            result: 1,
            effect: 0,
        };
    }
    if tag == 5 {
        if !ptrval.is_null() {
            // SAFETY: the enclosing native Session retains its live network socket.
            if let Some(socket) = unsafe { Socket::from_ptr(owner.socket) } {
                let address = socket.read(|record| record.info.addr);
                // SAFETY: native's peer destination is writable for the exact native address union.
                unsafe {
                    ptrval
                        .cast::<swoole_rs_reactor::AddressStorage>()
                        .write_unaligned(address);
                };
            }
        }
        return crate::ControlPlan {
            result: 1,
            effect: 0,
        };
    }
    crate::control_plan(tag, observed)
}

/// `listen`'s action for `DTLSv1_listen`'s result, marking the session listened.
pub fn swr_v1_tls_dtls_listen(owner: &DtlsSession, result: c_int) -> u32 {
    let action = crate::listen_plan(result);
    if action == 2 {
        owner.listened.set(true);
    }
    action
}

/// A BIO of the table `methods` makes, which OpenSSL lends each callback for the
/// call. It is zero-sized, so a reference to it claims none of the bytes OpenSSL
/// changes behind it.
#[repr(C)]
struct DtlsBio {
    _opaque: [u8; 0],
    _not_send: PhantomData<(*mut u8, PhantomPinned)>,
}

type Write = unsafe extern "C" fn(&DtlsBio, *const c_char, c_int) -> c_int;
type Read = unsafe extern "C" fn(&DtlsBio, *mut c_char, c_int) -> c_int;
type Control = unsafe extern "C" fn(&DtlsBio, c_int, c_long, *mut c_void) -> c_long;
type Hook = extern "C" fn(&DtlsBio) -> c_int;

/// A method table `BIO_meth_new` just made, which no BIO uses yet. It is zero-sized,
/// so a reference to it claims none of the bytes OpenSSL changes behind it.
#[repr(C)]
struct NewTable {
    _opaque: [u8; 0],
    _not_send: PhantomData<(*mut u8, PhantomPinned)>,
}

// SAFETY: libcrypto's declarations, typed so that a live BIO is a reference: the
// `safe` ones read or write only the BIO's own flags and shutdown fields (data and
// init stay unsafe: a method reads data as the session once init is set), and
// OpenSSL calls each method of a table with the live BIO it is reading or writing.
// `BIO_meth_new` copies the name and returns a table nothing else holds (null on
// failure), and the `safe` setters store a hook any BIO may be given into the table
// their caller holds exclusively.
unsafe extern "C" {
    safe fn BIO_get_new_index() -> c_int;
    safe fn BIO_meth_new(kind: c_int, name: CText<'_>) -> Option<&'static mut NewTable>;
    safe fn BIO_meth_set_create(table: &mut NewTable, create: Option<Hook>) -> c_int;
    safe fn BIO_meth_set_destroy(table: &mut NewTable, destroy: Option<Hook>) -> c_int;
    safe fn BIO_get_shutdown(bio: &DtlsBio) -> c_int;
    safe fn BIO_set_shutdown(bio: &DtlsBio, shutdown: c_int);
    safe fn BIO_set_flags(bio: &DtlsBio, flags: c_int);
    safe fn BIO_clear_flags(bio: &DtlsBio, flags: c_int);
    safe fn BIO_get_data(bio: &DtlsBio) -> *mut c_void;
    fn BIO_set_init(bio: &DtlsBio, init: c_int);
    fn BIO_set_data(bio: &DtlsBio, data: *mut c_void);
    fn BIO_new(table: *const BIO_METHOD) -> Option<&'static DtlsBio>;
    fn BIO_meth_set_write(table: *mut BIO_METHOD, write: Option<Write>) -> c_int;
    fn BIO_meth_set_read(table: *mut BIO_METHOD, read: Option<Read>) -> c_int;
    fn BIO_meth_set_ctrl(table: *mut BIO_METHOD, control: Option<Control>) -> c_int;
}

/// OpenSSL 3's `BIO_TYPE_SOURCE_SINK`, `BIO_FLAGS_READ`, `BIO_FLAGS_RWS` and
/// `BIO_FLAGS_SHOULD_RETRY`, and `SW_ERROR_SSL_CREATE_SESSION_FAILED`.
const BIO_TYPE_SOURCE_SINK: c_int = 0x0400;
const BIO_FLAGS_READ: c_int = 0x01;
const BIO_FLAGS_RWS: c_int = 0x07;
const BIO_FLAGS_SHOULD_RETRY: c_int = 0x08;
const SSL_CREATE_SESSION_FAILED: c_int = 1020;

/// OpenSSL 3's `BIO_CTRL_*` command as `swr_v1_tls_dtls_control`'s tag.
fn control_tag(command: c_int) -> u32 {
    match command {
        2 => 0,                         // EOF
        8 => 1,                         // GET_CLOSE
        10 => 2,                        // PENDING
        9 => 3,                         // SET_CLOSE
        11 | 32 | 44 => 4,              // FLUSH, DGRAM_SET_CONNECTED, DGRAM_SET_PEER
        46 => 5,                        // DGRAM_GET_PEER
        40 | 47 => 6,                   // DGRAM_QUERY_MTU, DGRAM_GET_FALLBACK_MTU
        49 => 7,                        // DGRAM_GET_MTU_OVERHEAD
        71 => 8,                        // DGRAM_SET_PEEK_MODE
        6 | 7 | 13 | 45 | 73 | 76 => 9, // PUSH, POP, WPENDING, DGRAM_SET_NEXT_TIMEOUT, GET_KTLS_*
        _ => u32::MAX,
    }
}

/// The session `swr_v1_tls_dtls_init` made the BIO for.
///
/// # Safety
/// `bio`'s session is live for `'a`.
unsafe fn session_of<'a>(bio: &DtlsBio) -> &'a DtlsSession {
    let session = BIO_get_data(bio).cast::<DtlsSession>();
    // SAFETY: as the caller promises: `swr_v1_tls_dtls_init` set it to the live session.
    unsafe { &*session }
}

/// `dtls::BIO_write`: `Socket::write`, a write(2) on the session's socket.
unsafe extern "C" fn bio_write(bio: &DtlsBio, data: *const c_char, length: c_int) -> c_int {
    // SAFETY: OpenSSL calls this for the session's BIO, whose session outlives its SSL.
    let session = unsafe { session_of(bio) };
    // SAFETY: a live session, whose socket is a live record.
    let Some(socket) = (unsafe { Socket::from_ptr(session.socket) }) else {
        return -1;
    };
    let fd = socket.fd();
    // SAFETY: OpenSSL lends `length` readable bytes.
    let written = unsafe { libc::write(fd, data.cast(), usize::try_from(length).unwrap_or(0)) };
    c_int::try_from(written).unwrap_or(-1)
}

/// `dtls::BIO_read`: the next queued datagram, or a read retry when there is none.
unsafe extern "C" fn bio_read(bio: &DtlsBio, data: *mut c_char, length: c_int) -> c_int {
    BIO_clear_flags(bio, BIO_FLAGS_RWS | BIO_FLAGS_SHOULD_RETRY);
    // SAFETY: OpenSSL calls this for the session's BIO, whose session outlives its SSL.
    let session = unsafe { session_of(bio) };
    // SAFETY: OpenSSL lends `length` writable bytes.
    let plan = unsafe { swr_v1_tls_dtls_read(session, data.cast(), length) };
    if plan.retry != 0 {
        BIO_set_flags(bio, BIO_FLAGS_READ | BIO_FLAGS_SHOULD_RETRY);
    }
    plan.length
}

/// `dtls::BIO_ctrl`.
unsafe extern "C" fn bio_ctrl(
    bio: &DtlsBio,
    command: c_int,
    value: c_long,
    pointer: *mut c_void,
) -> c_long {
    let tag = control_tag(command);
    let observed = if tag == 1 {
        c_long::from(BIO_get_shutdown(bio))
    } else {
        0
    };
    // SAFETY: OpenSSL calls this for the session's BIO, whose session outlives its SSL.
    let session = unsafe { session_of(bio) };
    // SAFETY: OpenSSL lends `pointer` as the command defines.
    let plan = unsafe { swr_v1_tls_dtls_control(session, tag, observed, value, pointer) };
    match plan.effect {
        #[expect(clippy::cast_possible_truncation, reason = "upstream's (int) lval")]
        1 => BIO_set_shutdown(bio, value as c_int),
        4 => log::warning(c"dtls::BIO_ctrl", format_args!("unknown cmd: {command}")),
        _ => {}
    }
    plan.result
}

extern "C" fn bio_create(_bio: &DtlsBio) -> c_int {
    1
}

extern "C" fn bio_destroy(_bio: &DtlsBio) -> c_int {
    1
}

static METHODS: AtomicPtr<BIO_METHOD> = AtomicPtr::new(ptr::null_mut());

/// `dtls::BIO_get_methods`: the process's one table, made on first use.
fn methods() -> *mut BIO_METHOD {
    let current = METHODS.load(Ordering::Acquire);
    if !current.is_null() {
        return current;
    }
    let index = BIO_get_new_index();
    let name = c"swoole_dtls_bio".into();
    let Some(table) = BIO_meth_new(index | BIO_TYPE_SOURCE_SINK, name) else {
        return ptr::null_mut();
    };
    BIO_meth_set_create(table, Some(bio_create));
    BIO_meth_set_destroy(table, Some(bio_destroy));
    let table: *mut BIO_METHOD = ptr::from_mut(table).cast();
    // SAFETY: the fresh table, and a callback of the slot's type.
    unsafe { BIO_meth_set_write(table, Some(bio_write)) };
    // SAFETY: as above.
    unsafe { BIO_meth_set_read(table, Some(bio_read)) };
    // SAFETY: as above.
    unsafe { BIO_meth_set_ctrl(table, Some(bio_ctrl)) };
    match METHODS.compare_exchange(ptr::null_mut(), table, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => table,
        Err(winner) => {
            // SAFETY: the losing table, which no BIO uses.
            unsafe { BIO_meth_free(table) };
            winner
        }
    }
}

/// `bool dtls::Session::init()`: the socket's server SSL, over a BIO that writes
/// to the socket and reads the session's datagram queue.
///
/// # Safety
/// `owner` is live, its socket a live record, and `context` the session's live `SSLContext`.
pub unsafe extern "C" fn swr_v1_tls_dtls_init(
    owner: *mut DtlsSession,
    context: *const crate::OwnedTlsContext,
) -> bool {
    // SAFETY: as the caller promises.
    let socket = unsafe { (*owner).socket };
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(socket) }) else {
        return false;
    };
    if !socket.read(swoole_rs_reactor::SocketFields::ssl).is_null() {
        return false;
    }
    let server = crate::SslCreateFlag::SW_SSL_SERVER as c_int;
    // SAFETY: as the caller promises.
    if unsafe { crate::upstream::socket_ssl_create(socket, context, server) } < 0 {
        log::set_last_error(SSL_CREATE_SESSION_FAILED);
        return false;
    }
    let size = swoole_rs_reactor::SOCKET_DEFAULTS.buffer_size();
    swoole_rs_reactor::swr_v1_socket_buffer_size(socket.fd(), true, true, size);
    let ssl = socket.edit(|record| {
        record.dtls = 1;
        record.ssl()
    });
    let table = methods();
    if table.is_null() {
        return false;
    }
    // SAFETY: a BIO of the live table above; OpenSSL returns null on failure.
    let Some(bio) = (unsafe { BIO_new(table) }) else {
        return false;
    };
    // SAFETY: the session outlives its SSL, which owns the BIO.
    unsafe { BIO_set_data(bio, owner.cast()) };
    // SAFETY: the fresh BIO, whose data is now the session its methods read.
    unsafe { BIO_set_init(bio, 1) };
    let bio = ptr::from_ref(bio).cast_mut().cast();
    // SAFETY: the SSL ssl_create made takes the BIO for both directions.
    unsafe { SSL_set_bio(ssl.cast(), bio, bio) };
    true
}

/// `bool dtls::Session::listen()`: once, `DTLSv1_listen` on the socket's SSL;
/// a pending listen counts as success and a failure logs why.
///
/// # Safety
/// The session's socket is a live record.
pub unsafe fn swr_v1_tls_dtls_session_listen(owner: &DtlsSession) -> bool {
    if owner.listened() {
        return false;
    }
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(owner.socket) }) else {
        return false;
    };
    crate::clear_errors();
    let Some(ssl) = crate::Ssl::of(socket) else {
        return false;
    };
    let result = ssl.dtls_listen();
    if swr_v1_tls_dtls_listen(owner, result) != 0 {
        return true;
    }
    let reason = crate::swr_v1_socket_ssl_peek_reason();
    let (host, port) = socket.read(|record| crate::upstream::peer(&record.info));
    let error = crate::error_line();
    log::warning(
        c"Session::listen",
        format_args!(
            "DTLSv1_listen() failed, client[{host}:{port}], reason={reason}, error_string={error}"
        ),
    );
    false
}

const _: () = assert!(
    core::mem::size_of::<Buffer>() == 2,
    "native datagram header is one uint16_t"
);

#[cfg(test)]
mod tests {
    use super::*;
    use openssl_sys::BIO;
    struct Owner(*mut DtlsSession);
    impl Owner {
        fn new(socket: *mut SocketRecord) -> Self {
            Self(swr_v1_tls_dtls_new(socket))
        }
        fn handle(&self) -> *mut DtlsSession {
            self.0
        }
        fn session(&self) -> &DtlsSession {
            unsafe { &*self.0 }
        }
        fn append(&self, bytes: &[u8]) {
            assert_eq!(swr_v1_tls_dtls_append(self.session(), bytes), 0);
        }
        fn read(&self, count: usize) -> (crate::ReadPlan, Vec<u8>) {
            let mut bytes = vec![0x55; count];
            let result = unsafe {
                swr_v1_tls_dtls_read(
                    self.session(),
                    bytes.as_mut_ptr(),
                    count.try_into().unwrap(),
                )
            };
            (result, bytes)
        }
        fn length(&self) -> usize {
            swr_v1_tls_dtls_length(unsafe { &*self.handle() })
        }
    }
    impl Drop for Owner {
        fn drop(&mut self) {
            unsafe { swr_v1_tls_dtls_free(self.handle()) };
        }
    }

    #[test]
    fn the_bio_reads_queued_datagrams_and_retries_when_empty() {
        unsafe extern "C" {
            fn BIO_test_flags(bio: *const BIO, flags: c_int) -> c_int;
        }
        let owner = Owner::new(ptr::null_mut());
        let table = methods();
        assert!(!table.is_null());
        assert_eq!(methods(), table);
        unsafe {
            let lent = BIO_new(table).unwrap();
            BIO_set_data(lent, owner.handle().cast());
            BIO_set_init(lent, 1);
            let bio: *mut BIO = ptr::from_ref(lent).cast_mut().cast();
            let mut bytes = [0u8; 8];
            assert_eq!(openssl_sys::BIO_read(bio, bytes.as_mut_ptr().cast(), 8), -1);
            assert_ne!(BIO_test_flags(bio, BIO_FLAGS_SHOULD_RETRY), 0);
            assert_eq!(openssl_sys::BIO_ctrl(bio, 2, 0, ptr::null_mut()), 1);
            owner.append(b"abc");
            assert_eq!(openssl_sys::BIO_ctrl(bio, 10, 0, ptr::null_mut()), 3);
            assert_eq!(openssl_sys::BIO_ctrl(bio, 40, 0, ptr::null_mut()), 1500);
            assert_eq!(openssl_sys::BIO_read(bio, bytes.as_mut_ptr().cast(), 8), 3);
            assert_eq!(&bytes[..3], b"abc");
            assert_eq!(BIO_test_flags(bio, BIO_FLAGS_SHOULD_RETRY), 0);
            openssl_sys::BIO_free_all(bio);
        }
    }

    #[test]
    fn whole_datagrams_peek_short_zero_read_and_retry_use_actual_queue_storage() {
        let owner = Owner::new(ptr::null_mut());
        owner.append(b"abcdef");
        owner.append(b"xyz");
        assert_eq!(owner.length(), 9);
        owner.session().peek.set(true);
        let (plan, bytes) = owner.read(2);
        assert_eq!((plan.length, plan.consume, plan.retry), (2, 0, 0));
        assert_eq!(bytes, b"ab");
        assert_eq!(owner.length(), 9);
        owner.session().peek.set(false);
        assert_eq!(owner.read(2).1, b"ab");
        assert_eq!(owner.length(), 3);
        let (plan, bytes) = owner.read(10);
        assert_eq!(plan.length, 3);
        assert_eq!(&bytes[..3], b"xyz");
        assert_eq!(owner.length(), 0);
        let (plan, _) = owner.read(1);
        assert_eq!((plan.length, plan.retry), (-1, 1));
        owner.append(b"");
        assert!(!swr_v1_tls_dtls_view(unsafe { &*owner.handle() }).empty);
        assert_eq!(owner.read(0).0.consume, 1);
        assert!(swr_v1_tls_dtls_view(unsafe { &*owner.handle() }).empty);
    }

    #[test]
    fn taken_native_datagram_and_scalar_aliases_have_one_canonical_owner() {
        let mut socket = SocketRecord::new();
        let owner = Owner::new(&raw mut socket);
        let raw = unsafe { libc::malloc(5) }.cast::<Buffer>();
        assert!(!raw.is_null());
        unsafe { (*raw).length = 3 };
        let data = unsafe { ptr::addr_of_mut!((*raw).data).cast::<u8>() };
        unsafe { ptr::copy_nonoverlapping(b"abc".as_ptr(), data, 3) };
        assert_eq!(
            unsafe { swr_v1_tls_dtls_append_buffer(owner.session(), raw.cast()) },
            0
        );
        assert_eq!(owner.read(3).1, b"abc");
        assert_eq!(unsafe { swr_v1_tls_dtls_listen(&*owner.handle(), 0) }, 1);
        assert!(!owner.session().listened());
        assert_eq!(unsafe { swr_v1_tls_dtls_listen(&*owner.handle(), 1) }, 2);
        assert!(owner.session().listened());
        unsafe { swr_v1_tls_dtls_control(&*owner.handle(), 8, 0, -7, ptr::null_mut()) };
        assert!(owner.session().peek.get());
        owner.append(b"abc");
        assert_eq!(
            unsafe { swr_v1_tls_dtls_control(&*owner.handle(), 2, 0, 0, ptr::null_mut()) }.result,
            3
        );
        owner.session().peek.set(false);
        assert_eq!(owner.read(3).1, b"abc");
        assert_eq!(
            unsafe { swr_v1_tls_dtls_control(&*owner.handle(), 0, 0, 0, ptr::null_mut()) }.result,
            1
        );
    }
}
