//! `dtls::Session`: its datagram queue, the BIO OpenSSL reads it through, and
//! `init`/`listen`. The BIO owns its session: the socket's SSL holds the BIO, and the
//! session goes when OpenSSL destroys it, so a session is reached through its socket.
use crate::handle::{BioMethod, CText, SslFailure};
use core::cell::Cell;
use core::ffi::{c_char, c_int, c_long, c_void};
use core::marker::{PhantomData, PhantomPinned};
use core::mem::MaybeUninit;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, PoisonError};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::log;
use swoole_rs_reactor::{AddressStorage, Fill, Socket};

/// One queued datagram, upstream's `dtls::Buffer`: the bytes it was received or copied
/// into, and the `uint16_t` length its header counts of them as the payload.
struct Datagram {
    bytes: Box<[MaybeUninit<u8>]>,
    length: u16,
}
type Queue = VecDeque<Datagram>;
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
    /// The socket's descriptor and peer, which are its own for as long as its SSL.
    fd: c_int,
    peer: AddressStorage,
    listened: Cell<bool>,
    peek: Cell<bool>,
    /// Taken out and put back around a push, pop or scan, never around a call into
    /// OpenSSL or one that yields.
    queue: Cell<Queue>,
    /// The port's context, which upstream's session holds a reference to.
    _context: Option<std::sync::Arc<crate::TlsSettings>>,
}

impl DtlsSession {
    /// An empty queue for the socket on `fd` connected to `peer`.
    fn new(
        fd: c_int,
        peer: AddressStorage,
        context: Option<std::sync::Arc<crate::TlsSettings>>,
    ) -> Self {
        Self {
            fd,
            peer,
            listened: Cell::new(false),
            peek: Cell::new(false),
            queue: Cell::new(Queue::new()),
            _context: context,
        }
    }

    /// The session `dtls_open` made for `socket`, borrowed as its SSL is; none without
    /// one.
    #[must_use]
    pub fn of(socket: Socket<'_>) -> Option<&DtlsSession> {
        let kind = SESSION_TYPE.load(Ordering::Acquire);
        let bio = crate::Ssl::of(socket)?.read_bio()?;
        BIO_get_data(BIO_find_type(
            bio,
            (kind != 0).then_some(SessionType(kind))?,
        )?)
    }

    /// `edit` over the queue, taken out and put back.
    fn with_queue<R>(&self, edit: impl FnOnce(&mut Queue) -> R) -> R {
        let mut queue = self.queue.take();
        let result = edit(&mut queue);
        self.queue.set(queue);
        result
    }

    /// Whether `listen` has published the session.
    #[must_use]
    pub fn listened(&self) -> bool {
        self.listened.get()
    }
}

/// One read(2) of at most `size` bytes from `fd` into a new `dtls::Buffer`, queued as a
/// datagram whose length is the read's result as upstream's `uint16_t` header takes it;
/// false when the queue can't grow.
pub fn dtls_session_receive(owner: &DtlsSession, fd: c_int, size: usize) -> bool {
    let mut bytes = Box::new_uninit_slice(size);
    let read = swoole_rs_corebase::sys::read(fd, &mut bytes);
    owner.with_queue(|queue| {
        if queue.try_reserve(1).is_err() {
            return false;
        }
        queue.push_back(Datagram {
            bytes,
            length: read.c_cast::<u16>(),
        });
        true
    })
}

/// A copy of `data` queued as one datagram: as many of its bytes as upstream's `u16`
/// header counts; false when it can't allocate.
pub fn swr_v1_tls_dtls_append(owner: &DtlsSession, data: &[u8]) -> bool {
    owner.with_queue(|queue| {
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
    })
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
    let peek = owner.peek.get();
    owner.with_queue(|queue| {
        let front = queue.front()?;
        // No more than `buffer` holds, so a datagram is never popped uncopied.
        let length = c_int::from(front.length())
            .min(requested)
            .min(c_int::try_from(buffer.len()).unwrap_or(c_int::MAX));
        if let Ok(copied) = usize::try_from(length) {
            if let (Some(out), Some(source)) =
                (buffer.get_mut(..copied), front.payload().get(..copied))
            {
                out.copy_from_slice(source);
            }
            if !peek {
                queue.pop_front();
            }
        }
        Some(length)
    })
}

/// The queued datagrams' total length.
pub fn swr_v1_tls_dtls_length(owner: &DtlsSession) -> usize {
    owner.with_queue(|queue| {
        queue.iter().fold(0usize, |total, buffer| {
            total.wrapping_add(usize::from(buffer.length()))
        })
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
        Command::Eof => c_long::from(owner.with_queue(|queue| queue.is_empty())),
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

/// Whether `DTLSv1_listen`'s `result` succeeded, marking the session listened unless
/// it is still pending (zero).
pub fn swr_v1_tls_dtls_listen(owner: &DtlsSession, result: Result<bool, SslFailure>) -> bool {
    if result == Ok(true) {
        owner.listened.set(true);
    }
    result.is_ok()
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
/// OpenSSL calls the create hook with a BIO it is making and the destroy hook with one
/// it is freeing, which nothing else uses meanwhile: the only `&mut DtlsBio`s there are.
type Hook = extern "C" fn(&mut DtlsBio) -> c_int;

/// The type of the table's BIOs, unique to it in the process: made only from
/// [`SESSION_TYPE`].
#[derive(Clone, Copy)]
#[repr(transparent)]
struct SessionType(c_int);

/// A pointer type that is always null.
enum Never {}

// libcrypto's declarations, typed so that a live BIO of the table is a reference: the
// `safe` ones read or write only the BIO's own flags, init and shutdown fields, or its
// data, which is null or the session the BIO owns (set only by `dtls_open`, on a fresh
// BIO; freed only by the destroy hook, which holds the BIO exclusively; and each method
// takes a BIO without a session). `BIO_find_type` answers a BIO of the table's type
// from the chain it is given. `BIO_meth_new` copies the name and returns a table
// nothing else holds (null on failure), which this side never frees, and the `safe`
// setters store a hook into the table their caller holds exclusively: storing one runs
// nothing. OpenSSL calls the hooks with the live BIO and the bytes it lends them.
#[expect(
    improper_ctypes,
    reason = "OpenSSL stores the session's address as the BIO's data and never reads it"
)]
#[expect(
    clashing_extern_declarations,
    reason = "the data's getter and setter, typed for the borrow each call site holds"
)]
unsafe extern "C" {
    safe fn BIO_get_new_index() -> c_int;
    safe fn BIO_meth_new(kind: c_int, name: CText<'_>) -> Option<&'static mut BioMethod>;
    safe fn BIO_meth_set_create(table: &mut BioMethod, create: Option<Hook>) -> c_int;
    safe fn BIO_meth_set_destroy(table: &mut BioMethod, destroy: Option<Hook>) -> c_int;
    safe fn BIO_meth_set_write(table: &mut BioMethod, write: Option<Write>) -> c_int;
    safe fn BIO_meth_set_read(table: &mut BioMethod, read: Option<Read>) -> c_int;
    safe fn BIO_meth_set_ctrl(table: &mut BioMethod, control: Option<Control>) -> c_int;
    safe fn BIO_get_shutdown(bio: &DtlsBio) -> c_int;
    safe fn BIO_set_shutdown(bio: &DtlsBio, shutdown: c_int);
    safe fn BIO_set_flags(bio: &DtlsBio, flags: c_int);
    safe fn BIO_clear_flags(bio: &DtlsBio, flags: c_int);
    safe fn BIO_get_data(bio: &DtlsBio) -> Option<&DtlsSession>;
    #[link_name = "BIO_get_data"]
    safe fn BIO_take_data(bio: &mut DtlsBio) -> *mut DtlsSession;
    #[link_name = "BIO_set_data"]
    safe fn BIO_clear_data(bio: &mut DtlsBio, data: Option<&'static Never>);
    safe fn BIO_set_init(bio: &DtlsBio, init: c_int);
    safe fn BIO_find_type<'a>(bio: crate::Bio<'a>, kind: SessionType) -> Option<&'a DtlsBio>;
    safe fn BIO_set_data(bio: FreshBio<'_>, data: Box<DtlsSession>);
}

/// A BIO of the table `BIO_new` just made, which holds no data yet: only
/// [`dtls_open`] makes one, once, so the session it gives the BIO is the BIO's only
/// data, which the destroy hook frees.
#[repr(transparent)]
struct FreshBio<'a>(crate::Bio<'a>);

/// OpenSSL 3's `BIO_TYPE_SOURCE_SINK`, `BIO_FLAGS_READ`, `BIO_FLAGS_RWS` and
/// `BIO_FLAGS_SHOULD_RETRY`, and `SW_ERROR_SSL_CREATE_SESSION_FAILED`.
const BIO_TYPE_SOURCE_SINK: c_int = 0x0400;
const BIO_FLAGS_READ: c_int = 0x01;
const BIO_FLAGS_RWS: c_int = 0x07;
const BIO_FLAGS_SHOULD_RETRY: c_int = 0x08;
const SSL_CREATE_SESSION_FAILED: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_SSL_CREATE_SESSION_FAILED as c_int;

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

/// `dtls::BIO_write`: `Socket::write`, a write(2) on the session's socket.
unsafe extern "C" fn bio_write(bio: &DtlsBio, data: *const c_char, length: c_int) -> c_int {
    let Some(session) = BIO_get_data(bio) else {
        return -1;
    };
    let fd = session.fd;
    // SAFETY: OpenSSL lends `length` readable bytes.
    let written = unsafe { libc::write(fd, data.cast(), usize::try_from(length).unwrap_or(0)) };
    c_int::try_from(written).unwrap_or(-1)
}

/// `dtls::BIO_read`: the next queued datagram, or a read retry when there is none.
unsafe extern "C" fn bio_read(bio: &DtlsBio, data: *mut c_char, length: c_int) -> c_int {
    BIO_clear_flags(bio, BIO_FLAGS_RWS | BIO_FLAGS_SHOULD_RETRY);
    let Some(session) = BIO_get_data(bio) else {
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
    let Some(session) = BIO_get_data(bio) else {
        return 0;
    };
    let Some(known) = Command::of(command) else {
        log::warning(c"dtls::BIO_ctrl", format_args!("unknown cmd: {command}"));
        return 0;
    };
    if known == Command::SetClose {
        BIO_set_shutdown(bio, value.c_cast::<c_int>());
    }
    if known == Command::GetPeer && !pointer.is_null() {
        let out = pointer.cast::<AddressStorage>();
        // SAFETY: OpenSSL lends the peer command a destination writable for the native
        // address union.
        unsafe { out.write_unaligned(session.peer) };
    }
    let shutdown = c_long::from(BIO_get_shutdown(bio));
    swr_v1_tls_dtls_control(session, known, shutdown, value)
}

/// A new BIO, initialized: its methods take one without a session.
extern "C" fn bio_create(bio: &mut DtlsBio) -> c_int {
    BIO_set_init(bio, 1);
    1
}

/// Frees the BIO's session, if it has one.
extern "C" fn bio_destroy(bio: &mut DtlsBio) -> c_int {
    let session = BIO_take_data(bio);
    BIO_clear_data(bio, None);
    if !session.is_null() {
        // SAFETY: the box `dtls_open` gave the BIO, now out of it; with the BIO held
        // exclusively, no borrow of it is live.
        drop(unsafe { Box::from_raw(session) });
    }
    1
}

/// The process's one table, once made; a thread making it holds the lock, so no other
/// makes a second one.
static METHODS: Mutex<Option<&'static BioMethod>> = Mutex::new(None);
/// The type of the table's BIOs once it is made, else 0: only `methods` stores it.
static SESSION_TYPE: AtomicI32 = AtomicI32::new(0);

/// `dtls::BIO_get_methods`: the process's one table, made on first use; none when
/// OpenSSL can't make it, which a later call tries again.
fn methods() -> Option<&'static BioMethod> {
    let mut made = METHODS.lock().unwrap_or_else(PoisonError::into_inner);
    if made.is_none() {
        let kind = BIO_get_new_index() | BIO_TYPE_SOURCE_SINK;
        let name = c"swoole_dtls_bio".into();
        let table = BIO_meth_new(kind, name)?;
        BIO_meth_set_create(table, Some(bio_create));
        BIO_meth_set_destroy(table, Some(bio_destroy));
        BIO_meth_set_write(table, Some(bio_write));
        BIO_meth_set_read(table, Some(bio_read));
        BIO_meth_set_ctrl(table, Some(bio_ctrl));
        SESSION_TYPE.store(kind, Ordering::Release);
        *made = Some(table);
    }
    *made
}

/// `new dtls::Session(socket, ctx)` and `bool dtls::Session::init()`: a session over
/// `socket`, sharing `context`, whose server SSL reads and writes through a BIO that
/// writes to the socket and reads the session's datagram queue; false when `init`
/// fails. The session goes with the SSL.
#[must_use]
pub fn dtls_open(socket: Socket<'_>, context: &std::sync::Arc<crate::TlsSettings>) -> bool {
    if !socket.read(swoole_rs_reactor::SocketFields::ssl).is_null() {
        return false;
    }
    if !context.create_session(socket) {
        log::set_last_error(SSL_CREATE_SESSION_FAILED);
        return false;
    }
    let size = swoole_rs_reactor::SOCKET_DEFAULTS.buffer_size();
    swoole_rs_reactor::swr_v1_socket_buffer_size(socket.fd(), true, true, size);
    socket.edit(|record| record.dtls = true);
    let Some(ssl) = crate::Ssl::of(socket) else {
        return false;
    };
    let Some(bio) = methods().and_then(|table| crate::handle::BIO_new(table)) else {
        return false;
    };
    let (fd, peer) = socket.read(|record| (record.fd, record.info.addr));
    let session = Box::new(DtlsSession::new(fd, peer, Some(context.clone())));
    // The fresh BIO of the table takes the session: its destroy hook frees it.
    BIO_set_data(FreshBio(bio.bio()), session);
    // SAFETY: the SSL ssl_create just made takes the BIO for both directions and frees
    // the descriptor BIOs `SSL_set_fd` gave it, which no handle borrows.
    _ = unsafe { ssl.set_bio(bio, None) };
    true
}

/// `bool dtls::Session::listen()` of `socket`'s session: once, `DTLSv1_listen` on its
/// SSL; a pending listen counts as success and a failure logs why. False without a
/// session.
#[must_use]
pub fn dtls_session_listen(socket: Socket<'_>) -> bool {
    let Some(owner) = DtlsSession::of(socket) else {
        return false;
    };
    if owner.listened() {
        return false;
    }
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
    use core::ptr;
    use openssl_sys::BIO;
    /// A session, freed with the owner unless a BIO took it.
    struct Owner(*mut DtlsSession, Cell<bool>);
    impl Owner {
        fn new() -> Self {
            let peer = swoole_rs_reactor::SocketAddress::default().addr;
            let session = Box::new(DtlsSession::new(-1, peer, None));
            Self(Box::into_raw(session), Cell::new(false))
        }
        fn handle(&self) -> *mut DtlsSession {
            self.0
        }
        /// The session, which a BIO's destroy hook then frees.
        fn give(&self) -> *mut DtlsSession {
            self.1.set(true);
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
            if !self.1.get() {
                drop(unsafe { Box::from_raw(self.handle()) });
            }
        }
    }

    #[test]
    fn the_bio_reads_queued_datagrams_and_retries_when_empty() {
        unsafe extern "C" {
            fn BIO_test_flags(bio: *const BIO, flags: c_int) -> c_int;
        }
        let owner = Owner::new();
        let table = methods().unwrap();
        assert!(ptr::eq(methods().unwrap(), table));
        unsafe {
            let lent = crate::handle::BIO_new(table).unwrap();
            BIO_set_data(FreshBio(lent.bio()), Box::from_raw(owner.give()));
            let bio: *mut BIO = lent.bio().as_ptr();
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
            drop(lent);
        }
    }

    #[test]
    fn whole_datagrams_peek_short_zero_read_and_retry_use_actual_queue_storage() {
        let owner = Owner::new();
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
        assert!(!owner.session().with_queue(|queue| queue.is_empty()));
        assert_eq!(owner.read(0).0, Some(0));
        assert!(owner.session().with_queue(|queue| queue.is_empty()));
    }

    #[test]
    fn taken_native_datagram_and_scalar_aliases_have_one_canonical_owner() {
        let owner = Owner::new();
        let [read, write] = swoole_rs_corebase::sys::pipe().unwrap();
        assert_eq!(swoole_rs_corebase::sys::write(write, b"abc"), 3);
        assert!(dtls_session_receive(owner.session(), read, 5));
        _ = swoole_rs_corebase::sys::close(read);
        _ = swoole_rs_corebase::sys::close(write);
        assert_eq!(owner.read(3).1, b"abc");
        let fatal = SslFailure {
            returned: -1,
            code: openssl_sys::SSL_ERROR_SSL,
        };
        assert!(!unsafe { swr_v1_tls_dtls_listen(&*owner.handle(), Err(fatal)) });
        assert!(unsafe { swr_v1_tls_dtls_listen(&*owner.handle(), Ok(false)) });
        assert!(!owner.session().listened());
        assert!(unsafe { swr_v1_tls_dtls_listen(&*owner.handle(), Ok(true)) });
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

    /// The session lives in the socket's SSL: found through the socket, not through a
    /// socket whose SSL reads another BIO, and freed with the SSL.
    #[test]
    #[cfg_attr(miri, ignore = "OpenSSL and sockets")]
    fn the_session_is_its_sockets_ssls() {
        use std::os::fd::AsRawFd;
        crate::swr_v1_tls_init().unwrap();
        let context = crate::TlsContext::new();
        let mut lent = context.borrow_configuration();
        lent.set_value(crate::Value::Protocols(
            crate::SslVersion::SW_SSL_DTLS as i64,
        ));
        let context = std::sync::Arc::new(lent.freeze(None).unwrap());
        let (_peer, stream) = std::os::unix::net::UnixDatagram::pair().unwrap();
        let raw = swoole_rs_reactor::SocketRecord::with_fd(stream.as_raw_fd(), 6).into_raw();
        let socket = unsafe { Socket::from_ptr(raw) }.unwrap();
        assert!(DtlsSession::of(socket).is_none());
        assert!(dtls_open(socket, &context));
        assert!(!dtls_open(socket, &context), "one session per socket");
        let session = DtlsSession::of(socket).unwrap();
        assert!(swr_v1_tls_dtls_append(session, b"abc"));
        assert_eq!(swr_v1_tls_dtls_length(DtlsSession::of(socket).unwrap()), 3);
        unsafe { crate::upstream::socket_ssl_close(socket) };
        assert!(DtlsSession::of(socket).is_none());
        unsafe { swoole_rs_reactor::SocketRecord::free(raw) };
    }
}
