//! `dtls::Session`: its datagram queue, the BIO OpenSSL reads it through, and
//! `init`/`listen`, independent of native `SSLContext` lifetime.
use crate::handle::CText;
use core::cell::{Cell, RefCell};
use core::ffi::{c_char, c_int, c_long, c_void};
use core::marker::{PhantomData, PhantomPinned};
use core::mem::MaybeUninit;
use core::ptr;
use openssl_sys::{BIO_METHOD, BIO_meth_free, SSL_set_bio};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicPtr, Ordering};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::log;
use swoole_rs_reactor::{Fill, Socket, SocketRecord};

/// One queued datagram, upstream's `dtls::Buffer`: the bytes it was received or copied
/// into, and the `uint16_t` length its header counts of them as the payload.
struct Datagram {
    bytes: Box<[MaybeUninit<u8>]>,
    length: u16,
}
impl Datagram {
    /// The payload's length.
    fn length(&self) -> u16 {
        self.length
    }

    /// The payload, which nothing writes while the datagram is queued; as
    /// `MaybeUninit`, its bytes need not be initialized.
    fn payload(&self) -> &[MaybeUninit<u8>] {
        let length = usize::from(self.length);
        self.bytes.get(..length).unwrap_or(&self.bytes)
    }
}

pub struct DtlsSession {
    socket: *mut SocketRecord,
    /// The socket as a handle, once `swr_v1_tls_dtls_init`'s caller has promised it
    /// live for as long as the session.
    live: Cell<Option<Socket<'static>>>,
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

/// `new dtls::Session(socket, ctx)` before `init`: an empty queue over `socket`.
#[must_use]
pub fn swr_v1_tls_dtls_new(socket: *mut SocketRecord) -> *mut DtlsSession {
    Box::into_raw(Box::new(DtlsSession {
        socket,
        live: Cell::new(None),
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

/// One read(2) of at most `size` bytes from `fd` into a new `dtls::Buffer`, queued as a
/// datagram whose length is the read's result as upstream's `uint16_t` header takes it;
/// false when the queue can't grow.
pub fn dtls_session_receive(owner: &DtlsSession, fd: c_int, size: usize) -> bool {
    let mut bytes = Box::new_uninit_slice(size);
    let read = swoole_rs_corebase::sys::read(fd, &mut bytes);
    let mut queue = owner.queue.borrow_mut();
    if queue.try_reserve(1).is_err() {
        return false;
    }
    queue.push_back(Datagram {
        bytes,
        length: read.c_cast::<u16>(),
    });
    true
}

/// A copy of `data` queued as one datagram: as many of its bytes as upstream's `u16`
/// header counts; false when it can't allocate.
pub fn swr_v1_tls_dtls_append(owner: &DtlsSession, data: &[u8]) -> bool {
    let mut queue = owner.queue.borrow_mut();
    if queue.try_reserve(1).is_err() {
        return false;
    }
    let length = usize::from(data.len().c_cast::<u16>());
    let mut bytes = Vec::new();
    if bytes.try_reserve_exact(length).is_err() {
        return false;
    }
    bytes.extend(data.iter().take(length).map(|&byte| MaybeUninit::new(byte)));
    queue.push_back(Datagram {
        bytes: bytes.into_boxed_slice(),
        length: length.c_cast::<u16>(),
    });
    true
}

/// The front datagram's length, as much of it as `requested` bytes hold, copied into
/// `buffer` and popped unless peeking; `None`, a retry, when the queue is empty.
/// `requested` is the length the BIO was asked for, which `buffer` holds when it is
/// not negative; a negative one is returned as it is, copying nothing.
pub fn swr_v1_tls_dtls_read(
    owner: &DtlsSession,
    buffer: &mut [MaybeUninit<u8>],
    requested: c_int,
) -> Option<c_int> {
    let mut queue = owner.queue.borrow_mut();
    let front = queue.front()?;
    // No more than `buffer` holds, so a datagram is never popped uncopied.
    let length = c_int::from(front.length())
        .min(requested)
        .min(c_int::try_from(buffer.len()).unwrap_or(c_int::MAX));
    if let Ok(copied) = usize::try_from(length) {
        if let (Some(out), Some(source)) = (buffer.get_mut(..copied), front.payload().get(..copied))
        {
            out.copy_from_slice(source);
        }
        if !owner.peek.get() {
            queue.pop_front();
        }
    }
    Some(length)
}

/// The queued datagrams' total length.
pub fn swr_v1_tls_dtls_length(owner: &DtlsSession) -> usize {
    owner.queue.borrow().iter().fold(0usize, |total, buffer| {
        total.wrapping_add(usize::from(buffer.length()))
    })
}

/// The answer to `command` for a BIO whose shutdown flag is `shutdown`, setting the
/// session's peek mode to `value` for [`Command::SetPeek`].
fn swr_v1_tls_dtls_control(
    owner: &DtlsSession,
    command: Command,
    shutdown: c_long,
    value: c_long,
) -> c_long {
    match command {
        Command::Eof => c_long::from(owner.queue.borrow().is_empty()),
        Command::GetClose => shutdown,
        Command::Pending => swr_v1_tls_dtls_length(owner).c_cast::<c_long>(),
        Command::SetClose | Command::Ignore => 0,
        Command::Accept | Command::GetPeer => 1,
        Command::Mtu => 1500,
        Command::MtuOverhead => 96,
        Command::SetPeek => {
            owner.peek.set(value != 0);
            1
        }
    }
}

/// The peer the session's socket names, which `BIO_CTRL_DGRAM_GET_PEER` answers with;
/// none before `swr_v1_tls_dtls_init`.
fn peer(owner: &DtlsSession) -> Option<swoole_rs_reactor::AddressStorage> {
    let socket = owner.live.get()?;
    Some(socket.read(|record| record.info.addr))
}

/// Whether `DTLSv1_listen`'s `result` succeeded, marking the session listened unless
/// it is still pending (zero).
pub fn swr_v1_tls_dtls_listen(owner: &DtlsSession, result: c_int) -> bool {
    if result > 0 {
        owner.listened.set(true);
    }
    result >= 0
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

// libcrypto's declarations, typed so that a live BIO is a reference: the
// `safe` ones read or write only the BIO's own flags and shutdown fields (data and
// init stay unsafe: a method reads data as the session once init is set), and
// OpenSSL calls each method of a table with the live BIO it is reading or writing.
// `BIO_meth_new` copies the name and returns a table nothing else holds (null on
// failure), and the `safe` setters store a hook into the table their caller holds
// exclusively: storing one runs nothing. The create and destroy hooks suit any BIO; the
// write, read and control hooks are `unsafe` to call, and only a BIO runs them, which
// only the unsafe `BIO_new` makes of a table, so its caller answers for them.
unsafe extern "C" {
    safe fn BIO_get_new_index() -> c_int;
    safe fn BIO_meth_new(kind: c_int, name: CText<'_>) -> Option<&'static mut NewTable>;
    safe fn BIO_meth_set_create(table: &mut NewTable, create: Option<Hook>) -> c_int;
    safe fn BIO_meth_set_destroy(table: &mut NewTable, destroy: Option<Hook>) -> c_int;
    safe fn BIO_meth_set_write(table: &mut NewTable, write: Option<Write>) -> c_int;
    safe fn BIO_meth_set_read(table: &mut NewTable, read: Option<Read>) -> c_int;
    safe fn BIO_meth_set_ctrl(table: &mut NewTable, control: Option<Control>) -> c_int;
    safe fn BIO_get_shutdown(bio: &DtlsBio) -> c_int;
    safe fn BIO_set_shutdown(bio: &DtlsBio, shutdown: c_int);
    safe fn BIO_set_flags(bio: &DtlsBio, flags: c_int);
    safe fn BIO_clear_flags(bio: &DtlsBio, flags: c_int);
    safe fn BIO_get_data(bio: &DtlsBio) -> *mut c_void;
    fn BIO_set_init(bio: &DtlsBio, init: c_int);
    fn BIO_set_data(bio: &DtlsBio, data: *mut c_void);
    fn BIO_new(table: *const BIO_METHOD) -> Option<&'static DtlsBio>;
}

/// OpenSSL 3's `BIO_TYPE_SOURCE_SINK`, `BIO_FLAGS_READ`, `BIO_FLAGS_RWS` and
/// `BIO_FLAGS_SHOULD_RETRY`, and `SW_ERROR_SSL_CREATE_SESSION_FAILED`.
const BIO_TYPE_SOURCE_SINK: c_int = 0x0400;
const BIO_FLAGS_READ: c_int = 0x01;
const BIO_FLAGS_RWS: c_int = 0x07;
const BIO_FLAGS_SHOULD_RETRY: c_int = 0x08;
const SSL_CREATE_SESSION_FAILED: c_int = 1020;

/// The OpenSSL 3 `BIO_CTRL_*` commands the BIO answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    /// `EOF`: whether the queue is empty.
    Eof,
    /// `GET_CLOSE`: the BIO's shutdown flag.
    GetClose,
    /// `PENDING`: the queued bytes.
    Pending,
    /// `SET_CLOSE`: sets the BIO's shutdown flag.
    SetClose,
    /// `FLUSH`, `DGRAM_SET_CONNECTED`, `DGRAM_SET_PEER`: accepted.
    Accept,
    /// `DGRAM_GET_PEER`: copies out the session's peer.
    GetPeer,
    /// `DGRAM_QUERY_MTU`, `DGRAM_GET_FALLBACK_MTU`.
    Mtu,
    /// `DGRAM_GET_MTU_OVERHEAD`.
    MtuOverhead,
    /// `DGRAM_SET_PEEK_MODE`.
    SetPeek,
    /// `PUSH`, `POP`, `WPENDING`, `DGRAM_SET_NEXT_TIMEOUT`, `GET_KTLS_*`: ignored.
    Ignore,
}

impl Command {
    /// `command`'s control, or `None` for one the BIO does not know.
    const fn of(command: c_int) -> Option<Self> {
        Some(match command {
            2 => Self::Eof,
            8 => Self::GetClose,
            10 => Self::Pending,
            9 => Self::SetClose,
            11 | 32 | 44 => Self::Accept,
            46 => Self::GetPeer,
            40 | 47 => Self::Mtu,
            49 => Self::MtuOverhead,
            71 => Self::SetPeek,
            6 | 7 | 13 | 45 | 73 | 76 => Self::Ignore,
            _ => return None,
        })
    }
}

/// The session `swr_v1_tls_dtls_init` made the BIO for, or `None` for a BIO it has not
/// given one yet (`BIO_new` runs the create hook first).
fn session_of(bio: &DtlsBio) -> Option<&DtlsSession> {
    let session = BIO_get_data(bio).cast::<DtlsSession>();
    // SAFETY: a BIO of this table has no data until the unsafe `BIO_set_data`, whose one
    // caller, `swr_v1_tls_dtls_init`, sets the live session, which outlives the SSL
    // that owns the BIO and so every borrow of the BIO OpenSSL lends.
    unsafe { session.as_ref() }
}

/// `dtls::BIO_write`: `Socket::write`, a write(2) on the session's socket.
unsafe extern "C" fn bio_write(bio: &DtlsBio, data: *const c_char, length: c_int) -> c_int {
    let Some(socket) = session_of(bio).and_then(|session| session.live.get()) else {
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
    let Some(session) = session_of(bio) else {
        return -1;
    };
    // SAFETY: OpenSSL lends `length` writable bytes.
    let mut buffer = unsafe { Fill::from_raw(data.cast(), usize::try_from(length).unwrap_or(0)) };
    let buffer = buffer.slots();
    swr_v1_tls_dtls_read(session, buffer, length).unwrap_or_else(|| {
        BIO_set_flags(bio, BIO_FLAGS_READ | BIO_FLAGS_SHOULD_RETRY);
        -1
    })
}

/// `dtls::BIO_ctrl`.
unsafe extern "C" fn bio_ctrl(
    bio: &DtlsBio,
    command: c_int,
    value: c_long,
    pointer: *mut c_void,
) -> c_long {
    let Some(session) = session_of(bio) else {
        return 0;
    };
    let Some(known) = Command::of(command) else {
        log::warning(c"dtls::BIO_ctrl", format_args!("unknown cmd: {command}"));
        return 0;
    };
    if known == Command::SetClose {
        BIO_set_shutdown(bio, value.c_cast::<c_int>());
    }
    if known == Command::GetPeer
        && !pointer.is_null()
        && let Some(address) = peer(session)
    {
        let out = pointer.cast::<swoole_rs_reactor::AddressStorage>();
        // SAFETY: OpenSSL lends the peer command a destination writable for the native
        // address union.
        unsafe { out.write_unaligned(address) };
    }
    let shutdown = c_long::from(BIO_get_shutdown(bio));
    swr_v1_tls_dtls_control(session, known, shutdown, value)
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
    BIO_meth_set_write(table, Some(bio_write));
    BIO_meth_set_read(table, Some(bio_read));
    BIO_meth_set_ctrl(table, Some(bio_ctrl));
    let table: *mut BIO_METHOD = ptr::from_mut(table).cast();
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
/// `owner` is live, its socket null or a live record that stays one until the session
/// is freed, and `context` the session's live `SSLContext`.
pub unsafe extern "C" fn swr_v1_tls_dtls_init(
    owner: *mut DtlsSession,
    context: *const crate::OwnedTlsContext,
) -> bool {
    // SAFETY: as the caller promises.
    let session = unsafe { &*owner };
    // SAFETY: as the caller promises, for as long as the session.
    let Some(socket) = (unsafe { Socket::from_ptr(session.socket) }) else {
        return false;
    };
    session.live.set(Some(socket));
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
        record.dtls = true;
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

/// `bool dtls::Session::listen()`: once, `DTLSv1_listen` on the socket's SSL; a pending
/// listen counts as success and a failure logs why. The socket is the handle
/// `swr_v1_tls_dtls_init` took, whose caller promised it live for as long as the
/// session, so a session lent here needs no further promise.
#[must_use]
pub fn dtls_session_listen(owner: &DtlsSession) -> bool {
    if owner.listened() {
        return false;
    }
    // The socket `swr_v1_tls_dtls_init` took; a session it never set up has no SSL.
    let Some(socket) = owner.live.get() else {
        return false;
    };
    crate::clear_errors();
    let Some(ssl) = crate::Ssl::of(socket) else {
        return false;
    };
    let result = ssl.dtls_listen();
    if swr_v1_tls_dtls_listen(owner, result) {
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
            assert!(swr_v1_tls_dtls_append(self.session(), bytes));
        }
        fn read(&self, count: usize) -> (Option<c_int>, Vec<u8>) {
            let mut bytes = vec![0x55; count];
            let lent = unsafe { std::slice::from_raw_parts_mut(bytes.as_mut_ptr().cast(), count) };
            let result = swr_v1_tls_dtls_read(self.session(), lent, count.try_into().unwrap());
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
        let (length, bytes) = owner.read(2);
        assert_eq!(length, Some(2));
        assert_eq!(bytes, b"ab");
        assert_eq!(owner.length(), 9);
        owner.session().peek.set(false);
        assert_eq!(owner.read(2).1, b"ab");
        assert_eq!(owner.length(), 3);
        let (length, bytes) = owner.read(10);
        assert_eq!(length, Some(3));
        assert_eq!(&bytes[..3], b"xyz");
        assert_eq!(owner.length(), 0);
        assert_eq!(owner.read(1).0, None);
        owner.append(b"");
        assert!(!owner.session().queue.borrow().is_empty());
        assert_eq!(owner.read(0).0, Some(0));
        assert!(owner.session().queue.borrow().is_empty());
    }

    #[test]
    fn taken_native_datagram_and_scalar_aliases_have_one_canonical_owner() {
        let mut socket = SocketRecord::new();
        let owner = Owner::new(&raw mut socket);
        let [read, write] = swoole_rs_corebase::sys::pipe().unwrap();
        assert_eq!(swoole_rs_corebase::sys::write(write, b"abc"), 3);
        assert!(dtls_session_receive(owner.session(), read, 5));
        swoole_rs_corebase::sys::close(read);
        swoole_rs_corebase::sys::close(write);
        assert_eq!(owner.read(3).1, b"abc");
        assert!(!unsafe { swr_v1_tls_dtls_listen(&*owner.handle(), -1) });
        assert!(unsafe { swr_v1_tls_dtls_listen(&*owner.handle(), 0) });
        assert!(!owner.session().listened());
        assert!(unsafe { swr_v1_tls_dtls_listen(&*owner.handle(), 1) });
        assert!(owner.session().listened());
        let control = |control, shutdown| unsafe {
            swr_v1_tls_dtls_control(&*owner.handle(), control, shutdown, -7)
        };
        assert_eq!(control(Command::SetPeek, 0), 1);
        assert!(owner.session().peek.get());
        owner.append(b"abc");
        assert_eq!(control(Command::Pending, 0), 3);
        owner.session().peek.set(false);
        assert_eq!(owner.read(3).1, b"abc");
        assert_eq!(control(Command::Eof, 0), 1);
        for (each, answer) in [
            (Command::GetClose, 5),
            (Command::SetClose, 0),
            (Command::Ignore, 0),
            (Command::Accept, 1),
            (Command::GetPeer, 1),
            (Command::Mtu, 1500),
            (Command::MtuOverhead, 96),
        ] {
            assert_eq!(control(each, 5), answer);
        }
        assert_eq!(Command::of(71), Some(Command::SetPeek));
        assert_eq!(Command::of(c_int::MAX), None);
    }
}
