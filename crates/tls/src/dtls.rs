//! `dtls::Session`: its datagram queue, the BIO OpenSSL reads it through, and
//! `init`/`listen`, independent of native `SSLContext` lifetime.
use core::cell::{Cell, RefCell};
use core::ffi::{c_char, c_int, c_long, c_void};
use core::ptr;
use openssl_sys::{
    BIO, BIO_METHOD, BIO_clear_flags, BIO_get_data, BIO_meth_free, BIO_meth_new,
    BIO_meth_set_create__fixed_rust, BIO_meth_set_ctrl__fixed_rust,
    BIO_meth_set_destroy__fixed_rust, BIO_meth_set_read__fixed_rust,
    BIO_meth_set_write__fixed_rust, BIO_new, BIO_set_data, BIO_set_flags, BIO_set_init, SSL,
    SSL_set_bio,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicPtr, Ordering};
use swoole_rs_corebase::log;
use swoole_rs_reactor::{Socket, SocketRecord};

#[repr(C)]
struct Buffer {
    length: u16,
    data: [u8; 0],
}

struct Datagram(*mut Buffer);
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
/// buffer is an exclusively transferred sw_malloc-compatible datagram.
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
    // SAFETY: the fresh header is valid and its payload immediately follows it.
    let out = unsafe { ptr::addr_of_mut!((*buffer).data).cast::<u8>() };
    if length > 0 {
        // SAFETY: the source covers `length` bytes; fresh storage cannot overlap it.
        unsafe { ptr::copy_nonoverlapping(data.as_ptr(), out, usize::from(length)) };
    }
    queue.push_back(Datagram(buffer));
    0
}

/// The front datagram's length, whether the queue is empty, and `peek_mode`.
pub fn swr_v1_tls_dtls_view(owner: &DtlsSession) -> DtlsReadView {
    let queue = owner.queue.borrow();
    let peek = owner.peek.get();
    let length = queue.front().map_or(0, |front| {
        // SAFETY: every queued pointer owns an initialized Buffer.
        unsafe { (*front.0).length }
    });
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
    // SAFETY: each queued pointer owns an initialized Buffer header.
    let length = unsafe { (*front.0).length };
    let plan = crate::read_plan(0, length, requested, u8::from(peek));
    if let Ok(length) = usize::try_from(plan.length) {
        // SAFETY: the initialized payload begins immediately after its header.
        let source = unsafe { ptr::addr_of!((*front.0).data).cast::<u8>() };
        if length > 0 {
            // SAFETY: the requested output is writable and the owned front covers length bytes; memmove permits overlap.
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
        // SAFETY: each queued pointer owns an initialized Buffer.
        total.wrapping_add(usize::from(unsafe { (*buffer.0).length }))
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
            if let Some(socket) = unsafe { socket_of(owner) } {
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

// SAFETY: libssl/libcrypto's own declarations; openssl-sys binds none of them.
unsafe extern "C" {
    fn BIO_get_new_index() -> c_int;
    fn BIO_get_shutdown(bio: *mut BIO) -> c_int;
    fn BIO_set_shutdown(bio: *mut BIO, shutdown: c_int);
    fn DTLSv1_listen(ssl: *mut SSL, peer: *mut c_void) -> c_int;
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
/// `bio` is one of those BIOs.
unsafe fn session_of(bio: *mut BIO) -> *mut DtlsSession {
    // SAFETY: as the caller promises.
    unsafe { BIO_get_data(bio) }.cast()
}

/// The session's socket, or None for a null one.
///
/// # Safety
/// `owner` is live, and its socket a live record while the handle is used.
unsafe fn socket_of<'a>(owner: *const DtlsSession) -> Option<Socket<'a>> {
    // SAFETY: as the caller promises.
    let socket = unsafe { (*owner).socket };
    // SAFETY: as the caller promises.
    unsafe { Socket::from_ptr(socket) }
}

/// `dtls::BIO_write`: `Socket::write`, a write(2) on the session's socket.
unsafe extern "C" fn bio_write(bio: *mut BIO, data: *const c_char, length: c_int) -> c_int {
    // SAFETY: OpenSSL calls this for the session's BIO.
    let session = unsafe { session_of(bio) };
    // SAFETY: a live session, whose socket is a live record.
    let Some(socket) = (unsafe { socket_of(session) }) else {
        return -1;
    };
    let fd = socket.fd();
    // SAFETY: OpenSSL lends `length` readable bytes.
    let written = unsafe { libc::write(fd, data.cast(), usize::try_from(length).unwrap_or(0)) };
    c_int::try_from(written).unwrap_or(-1)
}

/// `dtls::BIO_read`: the next queued datagram, or a read retry when there is none.
unsafe extern "C" fn bio_read(bio: *mut BIO, data: *mut c_char, length: c_int) -> c_int {
    // SAFETY: OpenSSL's live BIO.
    unsafe { BIO_clear_flags(bio, BIO_FLAGS_RWS | BIO_FLAGS_SHOULD_RETRY) };
    // SAFETY: OpenSSL calls this for the session's BIO.
    let session = unsafe { session_of(bio) };
    // SAFETY: the BIO's live session.
    let session = unsafe { &*session };
    // SAFETY: OpenSSL lends `length` writable bytes.
    let plan = unsafe { swr_v1_tls_dtls_read(session, data.cast(), length) };
    if plan.retry != 0 {
        // SAFETY: OpenSSL's live BIO.
        unsafe { BIO_set_flags(bio, BIO_FLAGS_READ | BIO_FLAGS_SHOULD_RETRY) };
    }
    plan.length
}

/// `dtls::BIO_ctrl`.
unsafe extern "C" fn bio_ctrl(
    bio: *mut BIO,
    command: c_int,
    value: c_long,
    pointer: *mut c_void,
) -> c_long {
    let tag = control_tag(command);
    let observed = if tag == 1 {
        // SAFETY: OpenSSL's live BIO.
        c_long::from(unsafe { BIO_get_shutdown(bio) })
    } else {
        0
    };
    // SAFETY: OpenSSL calls this for the session's BIO.
    let session = unsafe { session_of(bio) };
    // SAFETY: the BIO's live session.
    let session = unsafe { &*session };
    // SAFETY: OpenSSL lends `pointer` as the command defines.
    let plan = unsafe { swr_v1_tls_dtls_control(session, tag, observed, value, pointer) };
    match plan.effect {
        #[expect(clippy::cast_possible_truncation, reason = "upstream's (int) lval")]
        // SAFETY: OpenSSL's live BIO.
        1 => unsafe { BIO_set_shutdown(bio, value as c_int) },
        4 => log::warning(c"dtls::BIO_ctrl", format_args!("unknown cmd: {command}")),
        _ => {}
    }
    plan.result
}

unsafe extern "C" fn bio_create(_bio: *mut BIO) -> c_int {
    1
}

unsafe extern "C" fn bio_destroy(_bio: *mut BIO) -> c_int {
    1
}

static METHODS: AtomicPtr<BIO_METHOD> = AtomicPtr::new(ptr::null_mut());

/// `dtls::BIO_get_methods`: the process's one table, made on first use.
fn methods() -> *mut BIO_METHOD {
    let current = METHODS.load(Ordering::Acquire);
    if !current.is_null() {
        return current;
    }
    // SAFETY: allocates a BIO type index.
    let index = unsafe { BIO_get_new_index() };
    // SAFETY: a static name.
    let table = unsafe { BIO_meth_new(index | BIO_TYPE_SOURCE_SINK, c"swoole_dtls_bio".as_ptr()) };
    if table.is_null() {
        return table;
    }
    // SAFETY: the fresh table, and a callback of the slot's type.
    unsafe { BIO_meth_set_write__fixed_rust(table, Some(bio_write)) };
    // SAFETY: as above.
    unsafe { BIO_meth_set_read__fixed_rust(table, Some(bio_read)) };
    // SAFETY: as above.
    unsafe { BIO_meth_set_ctrl__fixed_rust(table, Some(bio_ctrl)) };
    // SAFETY: as above.
    unsafe { BIO_meth_set_create__fixed_rust(table, Some(bio_create)) };
    // SAFETY: as above.
    unsafe { BIO_meth_set_destroy__fixed_rust(table, Some(bio_destroy)) };
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
    let Some(socket) = (unsafe { socket_of(owner) }) else {
        return false;
    };
    if !socket.read(swoole_rs_reactor::SocketFields::ssl).is_null() {
        return false;
    }
    let server = crate::SslCreateFlag::SW_SSL_SERVER as c_int;
    // SAFETY: as the caller promises.
    if unsafe { crate::upstream::socket_ssl_create(socket.as_ptr(), context, server) } < 0 {
        log::set_last_error(SSL_CREATE_SESSION_FAILED);
        return false;
    }
    // SAFETY: a plain read of the native default.
    let size = unsafe { swoole_rs_reactor::swr_v1_socket_defaults.buffer_size };
    swoole_rs_reactor::swr_v1_socket_buffer_size(socket.fd(), true, true, size);
    let ssl = socket.edit(|record| {
        record.dtls = 1;
        record.ssl()
    });
    // SAFETY: a BIO of the table above; OpenSSL returns null on failure.
    let bio = unsafe { BIO_new(methods()) };
    // SAFETY: the session outlives its SSL, which owns the BIO.
    unsafe { BIO_set_data(bio, owner.cast()) };
    // SAFETY: the fresh BIO.
    unsafe { BIO_set_init(bio, 1) };
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
    let Some(socket) = (unsafe { socket_of(owner) }) else {
        return false;
    };
    crate::socket::clear_errors();
    let Some(ssl) = crate::Ssl::of(socket) else {
        return false;
    };
    // SAFETY: the record's live SSL.
    let result = unsafe { DTLSv1_listen(ssl.as_ptr(), ptr::null_mut()) };
    if swr_v1_tls_dtls_listen(owner, result) != 0 {
        return true;
    }
    let reason = crate::swr_v1_socket_ssl_peek_reason();
    let (host, port) = socket.read(|record| crate::upstream::peer(&record.info));
    // SAFETY: this thread's NUL-terminated error text.
    let error = unsafe { crate::upstream::text(crate::swr_v1_tls_error()) };
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
            let bio = BIO_new(table);
            BIO_set_data(bio, owner.handle().cast());
            BIO_set_init(bio, 1);
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
