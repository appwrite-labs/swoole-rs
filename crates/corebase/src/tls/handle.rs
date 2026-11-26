//! Handles over OpenSSL's objects, each wrapping its calls once: [`Ssl`] (a live
//! connection the socket record owns), [`Bio`] (a live BIO an SSL owns) and
//! [`OwnedBio`] (one this side owns until an SSL takes it), [`CertRef`] and [`Cert`]
//! (a borrowed and an owned certificate), and this thread's error queue. Callers go
//! through their methods. `Ssl<'a>` and `Bio<'a>` borrow their owner for `'a`, so a
//! handle can't outlive the record or SSL it came from, and the calls that need only a
//! live handle are declared `safe`. An SSL's BIOs are set only while this side owns
//! it ([`OwnedSsl`]), before any handle exists. Socket operations retain a
//! [`HeldSsl`] reference; closing the socket slot cannot free a borrowed session.
use crate::lent::{Count, In, Out, Spare, Wrote, lend_in, lend_spare};
use crate::reactor::{Socket, SocketFields, SocketSsl};
use crate::sys::{ReadSlot, Text};
use crate::tls::context::{EvpMd, SslCtx};
use openssl_sys::{
    BIO, OPENSSL_STACK, SSL, SSL_CIPHER, SSL_CTRL_MODE, SSL_CTRL_SET_TLSEXT_HOSTNAME, SSL_CTX,
    TLSEXT_NAMETYPE_host_name, X509, stack_st_X509,
};
use std::ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::marker::PhantomData;
use std::ptr::NonNull;

// libssl's and libcrypto's own declarations, with each pointer typed as the
// handle it is: `Ssl`, `Bio` and `CertRef` are live for their lifetime, `CipherRef` is
// an entry of OpenSSL's static cipher table, `Cert` and `OwnedBio` take the reference
// OpenSSL returns, a `Text<'static>` is one of OpenSSL's static strings, and an
// `SslCtx` reference is a live context.
// The `safe` ones take only such handles and plain values, read or change only the
// object behind the handle or this thread's error queue, and return raw pointers the
// caller dereferences in its own unsafe block.
unsafe extern "C" {
    safe fn ERR_clear_error();
    safe fn ERR_peek_error() -> c_ulong;
    safe fn ERR_get_error() -> c_ulong;
    safe fn ERR_reason_error_string(error: c_ulong) -> Option<Text<'static>>;
    /// Writes at most `length` bytes, NUL-terminated, into `buffer`: the length is the
    /// buffer's own, the one value [`ErrorLength`] has.
    safe fn ERR_error_string_n(error: c_ulong, buffer: &mut [u8; 512], length: ErrorLength);
    /// Whether the certificate matches the host `name`, which OpenSSL measures up to
    /// its NUL (the length [`Measured`] gives), storing the matched name where told to.
    safe fn X509_check_host(
        cert: CertRef<'_>,
        name: Text<'_>,
        length: Measured,
        flags: c_uint,
        peername: Option<&mut *mut c_char>,
    ) -> c_int;
    /// A static digest method, or null for an unknown name.
    safe fn EVP_get_digestbyname(name: Text<'_>) -> Option<&'static EvpMd>;
    /// OpenSSL writes at most `EVP_MAX_MD_SIZE` bytes and returns their length.
    safe fn X509_digest(
        cert: CertRef<'_>,
        method: &EvpMd,
        out: &mut [u8; openssl_sys::EVP_MAX_MD_SIZE as usize],
        length: &mut c_uint,
    ) -> c_int;
    safe fn BIO_s_mem() -> &'static BioMethod;
    /// A new BIO of `method`, its one reference the caller's; null on failure. The
    /// table lives for the process: OpenSSL's own, or `dtls.rs`'s, never freed once made,
    /// whose methods take a BIO without data as none of its session's.
    pub(crate) safe fn BIO_new(method: &'static BioMethod) -> Option<OwnedBio>;
    /// `fopen`s `filename` with `mode`; null on failure.
    safe fn BIO_new_file(filename: Text<'_>, mode: Text<'_>) -> Option<OwnedBio>;
    /// `BIO_pending` as a function: the bytes queued to read.
    safe fn BIO_ctrl_pending(bio: Bio<'_>) -> usize;
    safe fn BIO_method_type(bio: Bio<'_>) -> c_int;
    safe fn PEM_write_bio_X509(bio: Bio<'_>, cert: CertRef<'_>) -> c_int;
    safe fn SSL_up_ref(ssl: Ssl<'_>) -> c_int;
    safe fn SSL_get_error(ssl: Ssl<'_>, result: c_int) -> c_int;
    /// Writes at most `length` bytes of `buffer`, the count its lending gives.
    safe fn SSL_read<'id>(
        ssl: Ssl<'_>,
        buffer: Out<'id>,
        length: Count<'id, c_int>,
    ) -> Wrote<'id, c_int>;
    /// As `SSL_read`, leaving the bytes queued.
    safe fn SSL_peek<'id>(
        ssl: Ssl<'_>,
        buffer: Out<'id>,
        length: Count<'id, c_int>,
    ) -> Wrote<'id, c_int>;
    /// Reads at most `length` bytes of `data`, the count its lending gives.
    safe fn SSL_write<'id>(ssl: Ssl<'_>, data: In<'id>, length: Count<'id, c_int>) -> c_int;
    /// As `SSL_read`, from the BIO.
    safe fn BIO_read<'id>(
        bio: Bio<'_>,
        buffer: Out<'id>,
        length: Count<'id, c_int>,
    ) -> Wrote<'id, c_int>;
    /// As `SSL_write`, to the BIO.
    safe fn BIO_write<'id>(bio: Bio<'_>, data: In<'id>, length: Count<'id, c_int>) -> c_int;
    safe fn SSL_accept(ssl: Ssl<'_>) -> c_int;
    safe fn SSL_connect(ssl: Ssl<'_>) -> c_int;
    safe fn SSL_do_handshake(ssl: Ssl<'_>) -> c_int;
    safe fn SSL_shutdown(ssl: Ssl<'_>) -> c_int;
    safe fn SSL_in_init(ssl: Ssl<'_>) -> c_int;
    safe fn SSL_set_quiet_shutdown(ssl: Ssl<'_>, mode: c_int);
    safe fn SSL_get_shutdown(ssl: Ssl<'_>) -> c_int;
    safe fn SSL_set_shutdown(ssl: Ssl<'_>, mode: c_int);
    safe fn SSL_get_verify_result(ssl: Ssl<'_>) -> c_long;
    safe fn SSL_version(ssl: Ssl<'_>) -> c_int;
    safe fn SSL_pending(ssl: Ssl<'_>) -> c_int;
    safe fn SSL_set_accept_state(ssl: Ssl<'_>);
    safe fn SSL_set_connect_state(ssl: Ssl<'_>);
    /// `get0`: the SSL's own chain, null without one.
    safe fn SSL_get_peer_cert_chain<'a>(ssl: Ssl<'a>) -> Option<Chain<'a>>;
    safe fn OPENSSL_sk_num(chain: Chain<'_>) -> c_int;
    /// The chain's certificate at `index`, null outside it.
    safe fn OPENSSL_sk_value<'a>(chain: Chain<'a>, index: c_int) -> Option<CertRef<'a>>;
    safe fn SSL_get_rbio<'a>(ssl: Ssl<'a>) -> Option<Bio<'a>>;
    safe fn SSL_get_wbio<'a>(ssl: Ssl<'a>) -> Option<Bio<'a>>;
    safe fn SSL_get_ex_data(ssl: Ssl<'_>, index: c_int) -> *mut c_void;
    /// OpenSSL keeps `data` as opaque ex data: the indices this side makes register
    /// no callbacks that would read it. Only [`Ssl::set_ex_data`] calls it, which
    /// refuses the connection's index.
    safe fn SSL_set_ex_data(ssl: Ssl<'_>, index: c_int, data: *mut c_void) -> c_int;
    /// The host name the client sent, null without one: the SSL's own copy.
    safe fn SSL_get_servername<'a>(ssl: Ssl<'a>, kind: c_int) -> Option<Text<'a>>;
    /// Null before the handshake, else an entry of OpenSSL's static cipher table.
    safe fn SSL_get_current_cipher(ssl: Ssl<'_>) -> Option<CipherRef>;
    /// Null names the `(NONE)` cipher, whose bits are 0.
    safe fn SSL_CIPHER_get_bits(cipher: Option<CipherRef>, algorithm: Option<&mut c_int>) -> c_int;
    safe fn SSL_CIPHER_get_name(cipher: Option<CipherRef>) -> Option<Text<'static>>;
    safe fn SSL_CIPHER_get_version(cipher: Option<CipherRef>) -> Option<Text<'static>>;
    /// `SslCtrl`'s commands: `Mode` reads no pointer, and `HostName` copies `text`
    /// (null clears the name).
    safe fn SSL_ctrl(
        ssl: Ssl<'_>,
        command: SslCtrl,
        value: c_long,
        text: Option<Text<'_>>,
    ) -> c_long;
    /// The SSL takes its own reference to `context` (null: its session's context)
    /// and releases the one it held.
    safe fn SSL_set_SSL_CTX(ssl: Ssl<'_>, context: Option<&SslCtx>) -> *mut SSL_CTX;
    /// One listen step, which writes the peer's address when given somewhere to.
    safe fn DTLSv1_listen(ssl: Ssl<'_>, peer: Option<&mut BioAddr>) -> c_int;
    /// Gives the fresh SSL socket BIOs over `fd`, freeing the ones it had, which only
    /// a borrow of the [`OwnedSsl`] the exclusive `Fresh` was taken from could reach.
    safe fn SSL_set_fd(ssl: Fresh<'_>, fd: c_int) -> c_int;
    /// Gives the fresh SSL `read` and `write` (the same BIO for both directions takes
    /// one reference), taking the references [`Given`] carries, and frees the BIOs it
    /// had, as `SSL_set_fd` does.
    #[link_name = "SSL_set_bio"]
    safe fn set_bios(ssl: Fresh<'_>, read: Given, write: Given);
    /// A new SSL of `ctx`, which it references; null on failure, or without a context.
    pub(crate) safe fn SSL_new(ctx: Option<&SslCtx>) -> Option<OwnedSsl>;
    /// `SSL_get_peer_certificate`, a macro over `SSL_get1_peer_certificate` since
    /// OpenSSL 3 (build.rs): a reference the caller then owns.
    #[cfg_attr(ossl300, link_name = "SSL_get1_peer_certificate")]
    #[cfg_attr(not(ossl300), link_name = "SSL_get_peer_certificate")]
    safe fn peer_certificate(ssl: Ssl<'_>) -> Option<Cert>;
    /// `BioIntCtrl`'s one command reads only its integers, and a BIO that doesn't
    /// buffer ignores it.
    safe fn BIO_int_ctrl(
        bio: Bio<'_>,
        command: BioIntCtrl,
        large: c_long,
        integer: c_int,
    ) -> c_long;
}

/// The SSL `ex_data` index of its socket record, `swr_v1_tls_connection_index`: only
/// [`ConnectionIndex::get`] makes one. Ex data at it is null or the record that owns
/// the SSL: only [`OwnedSsl::set_connection`] stores there, before the record takes
/// the SSL ([`Ssl::set_ex_data`] refuses the index), and a record frees its SSL
/// before itself.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct ConnectionIndex(c_int);

impl ConnectionIndex {
    fn get() -> Self {
        Self(crate::tls::swr_v1_tls_connection_index())
    }
}

// libssl's ex data calls at the connection's index, typed as what it holds.
#[expect(
    clashing_extern_declarations,
    improper_ctypes,
    reason = "the ex data calls, typed for the connection's index; libssl keeps the socket as an opaque `void *`"
)]
unsafe extern "C" {
    #[link_name = "SSL_get_ex_data"]
    safe fn connection<'a>(ssl: Ssl<'a>, index: ConnectionIndex) -> Option<Socket<'a>>;
    #[link_name = "SSL_set_ex_data"]
    safe fn set_connection(ssl: Fresh<'_>, index: ConnectionIndex, socket: Socket<'_>) -> c_int;
}

/// A `BIO_METHOD` table. It is zero-sized, so a reference to it claims none of the
/// bytes OpenSSL keeps behind it, which only its setters write, before any BIO is made
/// of it.
// `repr(C)`: OpenSSL's opaque type, named behind references in `extern` blocks.
#[repr(C)]
pub(crate) struct BioMethod {
    _opaque: [u8; 0],
}

/// A `BIO_ADDR`, which this side never makes.
// `repr(C)`: OpenSSL's opaque type, named behind references in `extern` blocks.
#[repr(C)]
struct BioAddr {
    _opaque: [u8; 0],
}

/// The peer's certificate chain an SSL holds, borrowed as the SSL is: only
/// `SSL_get_peer_cert_chain` makes one, so its entries are certificates.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Chain<'a>(NonNull<OPENSSL_STACK>, PhantomData<&'a SSL>);

/// The `BIO_int_ctrl` command behind `BIO_set_write_buffer_size`.
#[derive(Clone, Copy)]
#[repr(i32)]
enum BioIntCtrl {
    SetBufferSize = 117,
}

/// The `SSL_ctrl` commands behind `SSL_set_mode` and `SSL_set_tlsext_host_name`.
#[derive(Clone, Copy)]
#[repr(i32)]
enum SslCtrl {
    Mode = SSL_CTRL_MODE,
    HostName = SSL_CTRL_SET_TLSEXT_HOSTNAME,
}

/// An OpenSSL I/O or handshake call that made no progress: what it returned (0 or
/// negative) and `SSL_get_error`'s code for that, read straight after the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SslFailure {
    pub returned: c_int,
    pub code: c_int,
}

pub(super) fn shutdown_result(
    returned: c_int,
    repaired: bool,
    error: impl FnOnce() -> c_int,
) -> Result<(), SslFailure> {
    let check = if repaired {
        returned < 0
    } else {
        returned != 1 && peek_error() != 0
    };
    if check {
        Err(SslFailure {
            returned,
            code: error(),
        })
    } else {
        Ok(())
    }
}

/// Clears this thread's OpenSSL error queue. Usually it is already empty, and the peek
/// is a quarter of the clear, which frees and zeroes all sixteen slots (#4283).
pub fn clear_errors() {
    if ERR_peek_error() != 0 {
        ERR_clear_error();
    }
}

/// This thread's oldest queued OpenSSL error, left queued.
#[must_use]
pub fn peek_error() -> c_ulong {
    ERR_peek_error()
}

/// This thread's oldest queued OpenSSL error, popped.
#[must_use]
pub fn pop_error() -> c_ulong {
    ERR_get_error()
}

/// OpenSSL's reason string for `error`, None for an unknown code.
#[must_use]
pub fn reason_string(error: c_ulong) -> Option<&'static CStr> {
    ERR_reason_error_string(error).map(Text::get)
}

/// `ERR_error_string_n`'s length: that of its 512-byte buffer, as a `size_t`.
#[repr(usize)]
enum ErrorLength {
    Buffer = 512,
}

/// `X509_check_host`'s zero length: OpenSSL measures the name itself.
#[repr(usize)]
enum Measured {
    UpToNul = 0,
}

/// `ERR_error_string_n` of `error` into a 512-byte buffer, as upstream's logs print it.
#[must_use]
pub fn error_string(error: c_ulong) -> String {
    let mut buffer = [0_u8; 512];
    ERR_error_string_n(error, &mut buffer, ErrorLength::Buffer);
    CStr::from_bytes_until_nul(&buffer)
        .map_or_else(|_| String::new(), |text| text.to_string_lossy().into())
}

/// A live SSL on this thread for `'a`. A socket record's is live while the record
/// holds it: socket operations borrow a retained [`HeldSsl`], while a new
/// session's setup borrows its [`OwnedSsl`].
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ssl<'a>(NonNull<SSL>, PhantomData<&'a SSL>);

/// An operation's retained SSL reference, still bounded by the socket record's
/// lifetime. Its handles borrow this lease, so none can survive its release.
///
/// ```compile_fail
/// use crate::reactor::Socket;
/// use swoole_rs_corebase::tls::Ssl;
/// fn escape(socket: Socket<'_>) -> Option<Ssl<'_>> {
///     let held = Ssl::of(socket)?;
///     Some(held.get())
/// }
/// ```
#[derive(Debug)]
pub struct HeldSsl<'a>(NonNull<SSL>, PhantomData<Socket<'a>>);

impl HeldSsl<'_> {
    #[must_use]
    pub fn get(&self) -> Ssl<'_> {
        Ssl(self.0, PhantomData)
    }
}

impl Drop for HeldSsl<'_> {
    fn drop(&mut self) {
        /// The one reference acquired by `SSL_up_ref`, released once by this drop.
        #[repr(transparent)]
        struct Released(NonNull<SSL>);
        unsafe extern "C" {
            safe fn SSL_free(ssl: Released);
        }
        SSL_free(Released(self.0));
    }
}

/// An entry of OpenSSL's static cipher table.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct CipherRef(NonNull<SSL_CIPHER>);

/// A connection's cipher: OpenSSL names it `(NONE)` with 0 bits before the handshake
/// picks one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cipher {
    pub bits: c_int,
    pub name: &'static CStr,
    pub version: &'static CStr,
}

impl<'a> Ssl<'a> {
    /// Retain the record's SSL for this operation. Handles borrowed from the
    /// returned lease remain live even if another operation closes the SSL slot.
    #[must_use]
    pub fn of(socket: Socket<'a>) -> Option<HeldSsl<'a>> {
        Self::of_reading(socket, |_| ()).0
    }

    /// [`Self::of`], and `read` over the record's fields in the same read of it.
    /// # Panics
    /// When OpenSSL cannot retain the live session reference.
    pub fn of_reading<T>(
        socket: Socket<'a>,
        read: impl FnOnce(&SocketFields) -> T,
    ) -> (Option<HeldSsl<'a>>, T) {
        socket.read(|record| (Self::retained(socket, record), read(record)))
    }

    /// [`Self::of`] inside a read of `socket`, which lent `record`: the lease is bounded
    /// by the socket that owns the SSL.
    /// # Panics
    /// When `record` is not `socket`'s fields, or OpenSSL cannot retain the live session
    /// reference.
    pub(crate) fn retained(socket: Socket<'a>, record: &SocketFields) -> Option<HeldSsl<'a>> {
        assert!(socket.lends(record), "an SSL retained from another record");
        NonNull::new(record.ssl().cast()).map(|ptr| {
            // The record owns this reference throughout its read. Retain
            // another before any callback or coroutine can close the slot.
            assert_eq!(SSL_up_ref(Self(ptr, PhantomData)), 1, "SSL_up_ref failed");
            HeldSsl(ptr, PhantomData)
        })
    }

    #[must_use]
    pub fn as_ptr(self) -> *mut SSL {
        self.0.as_ptr()
    }

    /// `SSL_get_error` for its last I/O result.
    #[must_use]
    pub fn error(self, result: c_int) -> c_int {
        SSL_get_error(self, result)
    }

    /// The bytes an `SSL_read`, `SSL_write` or `SSL_peek` that returned `n` moved.
    fn count(self, n: c_int) -> Result<usize, SslFailure> {
        match usize::try_from(n) {
            Ok(count) if count > 0 => Ok(count),
            _ => Err(SslFailure {
                returned: n,
                code: self.error(n),
            }),
        }
    }

    /// Whether a handshake step that returned `n` finished.
    fn step(self, n: c_int) -> Result<(), SslFailure> {
        if n == 1 {
            return Ok(());
        }
        Err(SslFailure {
            returned: n,
            code: self.error(n),
        })
    }

    /// `SSL_read` into `buffer`, which may be uninitialized: OpenSSL only writes it.
    ///
    /// # Errors
    /// Nothing was read.
    pub fn read(self, buffer: &mut [impl ReadSlot]) -> Result<usize, SslFailure> {
        lend_spare(buffer, |spare| self.read_into(spare)).0
    }

    /// `SSL_read` into `spare`'s room.
    ///
    /// # Errors
    /// Nothing was read.
    #[inline]
    pub fn read_into(self, spare: &mut Spare<'_>) -> Result<usize, SslFailure> {
        self.count(spare.lend(|at, length| SSL_read(self, at, length)))
    }

    /// `SSL_write` of `data`.
    ///
    /// # Errors
    /// Nothing was written.
    pub fn write(self, data: &[u8]) -> Result<usize, SslFailure> {
        self.count(lend_in(data, |at, length| {
            SSL_write(self, at.bytes(), length)
        }))
    }

    /// `SSL_peek` into `spare`'s room.
    ///
    /// # Errors
    /// Nothing was peeked.
    pub fn peek(self, spare: &mut Spare<'_>) -> Result<usize, SslFailure> {
        self.count(spare.lend(|at, length| SSL_peek(self, at, length)))
    }

    /// One `SSL_accept` step.
    ///
    /// # Errors
    /// The handshake isn't done.
    pub fn accept(self) -> Result<(), SslFailure> {
        self.step(SSL_accept(self))
    }

    /// One `SSL_connect` step.
    ///
    /// # Errors
    /// The handshake isn't done.
    pub fn connect(self) -> Result<(), SslFailure> {
        self.step(SSL_connect(self))
    }

    /// One `SSL_do_handshake` step.
    ///
    /// # Errors
    /// The handshake isn't done.
    pub fn do_handshake(self) -> Result<(), SslFailure> {
        self.step(SSL_do_handshake(self))
    }

    /// Run the shutdown step with the socket's half-close policy.
    /// # Errors
    /// Returns the OpenSSL error class when the step requires error handling.
    pub(crate) fn shutdown(self, repaired: bool) -> Result<(), SslFailure> {
        let returned = SSL_shutdown(self);
        shutdown_result(returned, repaired, || self.error(returned))
    }

    /// `SSL_in_init`: whether the handshake is still running.
    #[must_use]
    pub fn in_init(self) -> bool {
        SSL_in_init(self) != 0
    }

    /// `SSL_set_quiet_shutdown(ssl, 1)`.
    pub fn set_quiet_shutdown(self) {
        SSL_set_quiet_shutdown(self, 1);
    }

    /// `SSL_get_shutdown`.
    #[must_use]
    pub fn shutdown_mode(self) -> c_int {
        SSL_get_shutdown(self)
    }

    /// `SSL_set_shutdown`.
    pub fn set_shutdown_mode(self, mode: c_int) {
        SSL_set_shutdown(self, mode);
    }

    /// `SSL_get_verify_result`.
    #[must_use]
    pub fn verify_result(self) -> c_long {
        SSL_get_verify_result(self)
    }

    /// The peer's certificate (`get1`), a reference this side then owns.
    #[must_use]
    pub fn peer_certificate(self) -> Option<Cert> {
        peer_certificate(self)
    }

    /// `SSL_get_peer_cert_chain` (`get0`): the SSL's own chain, null without one.
    #[must_use]
    pub fn peer_cert_chain(self) -> *mut stack_st_X509 {
        SSL_get_peer_cert_chain(self).map_or(std::ptr::null_mut(), |chain| chain.0.as_ptr().cast())
    }

    /// `f` over each certificate of the peer's chain, in order.
    pub fn each_peer_cert(self, mut f: impl FnMut(CertRef<'_>)) {
        let Some(chain) = SSL_get_peer_cert_chain(self) else {
            return;
        };
        for index in 0..OPENSSL_sk_num(chain) {
            if let Some(cert) = OPENSSL_sk_value(chain, index) {
                f(cert);
            }
        }
    }

    /// `SSL_version`: `TLS1_3_VERSION` and the like.
    #[must_use]
    pub fn version(self) -> c_int {
        SSL_version(self)
    }

    /// `SSL_pending`: the decrypted bytes buffered to read.
    #[must_use]
    pub fn pending(self) -> c_int {
        SSL_pending(self)
    }

    /// The current cipher's bits, name and version.
    #[must_use]
    pub fn cipher(self) -> Cipher {
        let cipher = SSL_get_current_cipher(self);
        let (name, version) = (SSL_CIPHER_get_name(cipher), SSL_CIPHER_get_version(cipher));
        Cipher {
            bits: SSL_CIPHER_get_bits(cipher, None),
            // The table's name and version; OpenSSL itself answers `(NONE)` without a cipher.
            name: name.map_or(c"(NONE)", Text::get),
            version: version.map_or(c"(NONE)", Text::get),
        }
    }

    /// `SSL_set_mode`: adds `mode`'s bits.
    pub fn set_mode(self, mode: c_long) {
        SSL_ctrl(self, SslCtrl::Mode, mode, None);
    }

    /// `SSL_set_accept_state`.
    pub fn set_accept_state(self) {
        SSL_set_accept_state(self);
    }

    /// `SSL_set_connect_state`.
    pub fn set_connect_state(self) {
        SSL_set_connect_state(self);
    }

    /// `SSL_get_rbio` (`get0`), None without one: live until the SSL is freed (its
    /// BIOs are set before a handle to it exists).
    #[must_use]
    pub fn read_bio(self) -> Option<Bio<'a>> {
        SSL_get_rbio(self)
    }

    /// `SSL_get_wbio` (`get0`), None without one, live as `read_bio`'s is.
    #[must_use]
    pub fn write_bio(self) -> Option<Bio<'a>> {
        SSL_get_wbio(self)
    }

    /// `SSL_get_ex_data` at `index`.
    #[must_use]
    pub fn ex_data(self, index: c_int) -> *mut c_void {
        SSL_get_ex_data(self, index)
    }

    /// The socket record that owns the SSL, None while none has taken it.
    #[must_use]
    pub fn socket(self) -> Option<Socket<'a>> {
        connection(self, ConnectionIndex::get())
    }

    /// `SSL_set_ex_data` at `index`: false when OpenSSL fails. Also false, setting
    /// nothing, at the connection's index: the info callback reads that entry as the
    /// SSL's socket record, so only [`crate::tls::socket_ssl_new`] sets it.
    #[must_use]
    pub fn set_ex_data(self, index: c_int, data: *mut c_void) -> bool {
        index != crate::tls::swr_v1_tls_connection_index()
            && SSL_set_ex_data(self, index, data) != 0
    }

    /// The SNI host name the client sent, None without one.
    #[must_use]
    pub fn servername(self) -> Option<Vec<u8>> {
        let name = SSL_get_servername(self, TLSEXT_NAMETYPE_host_name);
        name.map(|name| name.get().to_bytes().to_vec())
    }

    /// `SSL_set_SSL_CTX`: the SSL switches to `context`, which it then references;
    /// without one, to its session's context.
    pub fn set_context(self, context: Option<&SslCtx>) {
        SSL_set_SSL_CTX(self, context);
    }

    /// `SSL_set_tlsext_host_name`: false when OpenSSL refuses the name.
    #[must_use]
    pub fn set_host_name(self, name: &CStr) -> bool {
        let kind = c_long::from(TLSEXT_NAMETYPE_host_name);
        SSL_ctrl(self, SslCtrl::HostName, kind, Some(name.into())) != 0
    }

    /// One `DTLSv1_listen` step, without the peer's address: whether a client
    /// answered the cookie exchange.
    ///
    /// # Errors
    /// The listen failed fatally.
    pub fn dtls_listen(self) -> Result<bool, SslFailure> {
        let n = DTLSv1_listen(self, None);
        if n < 0 {
            return Err(SslFailure {
                returned: n,
                code: self.error(n),
            });
        }
        Ok(n > 0)
    }
}

/// OpenSSL's source/sink BIO type bit, shared by memory and datagram BIOs.
pub(super) const BIO_TYPE_SOURCE_SINK: c_int = 0x0400;

/// A live BIO for `'a`, borrowed from the SSL or [`OwnedBio`] that owns it: an SSL's
/// lives until the SSL is freed or the BIO replaced, which only unsafe calls do.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bio<'a>(NonNull<BIO>, PhantomData<&'a BIO>);

impl Bio<'_> {
    #[must_use]
    pub fn as_ptr(self) -> *mut BIO {
        self.0.as_ptr()
    }

    /// `BIO_pending`: the bytes queued to read.
    #[must_use]
    pub fn pending(self) -> usize {
        BIO_ctrl_pending(self)
    }

    /// Whether it is a memory BIO (`BIO_TYPE_MEM`), as [`OwnedBio::memory`] makes.
    #[must_use]
    pub fn is_memory(self) -> bool {
        const BIO_TYPE_MEM: c_int = BIO_TYPE_SOURCE_SINK | 1;
        BIO_method_type(self) == BIO_TYPE_MEM
    }

    /// `BIO_read` into `spare`'s room: the bytes read, or None for none.
    pub fn read(self, spare: &mut Spare<'_>) -> Option<usize> {
        let n = spare.lend(|at, length| BIO_read(self, at, length));
        usize::try_from(n).ok().filter(|&n| n > 0)
    }

    /// `BIO_write` of `data`: the bytes written, or None for none.
    #[must_use]
    pub fn write(self, data: &[u8]) -> Option<usize> {
        let n = lend_in(data, |at, length| BIO_write(self, at.bytes(), length));
        usize::try_from(n).ok().filter(|&n| n > 0)
    }

    /// `BIO_set_write_buffer_size` (`BIO_C_SET_BUFF_SIZE`, 117) of a buffering BIO.
    pub fn set_write_buffer_size(self, size: c_long) {
        BIO_int_ctrl(self, BioIntCtrl::SetBufferSize, size, 1);
    }
}

/// A BIO this side owns, freed when dropped unless an SSL takes it.
#[repr(transparent)]
#[derive(Debug)]
pub struct OwnedBio(NonNull<BIO>);

impl OwnedBio {
    /// A new memory BIO, None when OpenSSL fails.
    #[must_use]
    pub fn memory() -> Option<Self> {
        BIO_new(BIO_s_mem())
    }

    /// `BIO_new_file(path, "r")`: a BIO reading the file, None when it fails to open.
    #[must_use]
    pub(crate) fn open(path: &CStr) -> Option<Self> {
        BIO_new_file(path.into(), c"r".into())
    }

    /// The BIO, borrowed while this side owns it.
    pub(crate) fn bio(&self) -> Bio<'_> {
        Bio(self.0, PhantomData)
    }

    /// The BIO, whose reference the caller hands on.
    fn into_raw(self) -> NonNull<BIO> {
        std::mem::ManuallyDrop::new(self).0
    }
}

impl Drop for OwnedBio {
    fn drop(&mut self) {
        /// The reference a dropped `OwnedBio` held: only this drop makes one, once per
        /// handle, so `BIO_free_all` releases each reference once.
        #[repr(transparent)]
        struct Released(NonNull<BIO>);
        unsafe extern "C" {
            safe fn BIO_free_all(bio: Released);
        }
        BIO_free_all(Released(self.0));
    }
}

/// A live certificate, borrowed for `'a`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct CertRef<'a>(NonNull<X509>, PhantomData<&'a X509>);

impl CertRef<'_> {
    #[must_use]
    pub fn as_ptr(self) -> *mut X509 {
        self.0.as_ptr()
    }

    /// `Socket::ssl_check_host`'s test: whether the certificate matches the host
    /// `name` under `X509_check_host`.
    #[must_use]
    pub fn matches_host(self, name: &CStr) -> bool {
        X509_check_host(self, name.into(), Measured::UpToNul, 0, None) == 1
    }

    /// `_ssl_read_x509_file`: the certificate as PEM into at most `buffer`'s length.
    ///
    /// # Errors
    /// The [`PemError`] step that failed.
    pub fn pem(self, buffer: &mut [impl ReadSlot]) -> Result<usize, PemError> {
        lend_spare(buffer, |spare| self.pem_into(spare)).0
    }

    /// [`CertRef::pem`] into `spare`'s room.
    ///
    /// # Errors
    /// The [`PemError`] step that failed.
    pub fn pem_into(self, spare: &mut Spare<'_>) -> Result<usize, PemError> {
        let bio = OwnedBio::memory().ok_or(PemError::Bio)?;
        if PEM_write_bio_X509(bio.bio(), self) == 0 {
            return Err(PemError::Write);
        }
        // A memory BIO reads at most what it holds.
        if bio.bio().pending().min(spare.room()) == 0 {
            return Ok(0);
        }
        bio.bio().read(spare).ok_or(PemError::Read)
    }
}

/// The step of [`CertRef::pem`] that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PemError {
    /// `BIO_new`.
    Bio,
    /// `PEM_write_bio_X509`.
    Write,
    /// `BIO_read`.
    Read,
}

/// A live certificate reference this side owns, released when dropped.
#[repr(transparent)]
#[derive(Debug)]
pub struct Cert(pub(crate) NonNull<X509>);

impl Cert {
    /// Compare the certificate digest with a hexadecimal fingerprint.
    #[must_use]
    pub fn matches_fingerprint(&self, algorithm: &CStr, expected: &[u8]) -> bool {
        let Some(method) = EVP_get_digestbyname(algorithm.into()) else {
            return false;
        };
        let mut digest = [0; openssl_sys::EVP_MAX_MD_SIZE as usize];
        let mut length = 0;
        if X509_digest(self.get(), method, &mut digest, &mut length) != 1
            || expected.len() != length as usize * 2
            || length == 0
        {
            return false;
        }
        digest
            .iter()
            .zip(expected.as_chunks::<2>().0)
            .all(|(&byte, &[high, low])| {
                let high = char::from(high).to_digit(16);
                let low = char::from(low).to_digit(16);
                high == Some(u32::from(byte >> 4)) && low == Some(u32::from(byte & 15))
            })
    }

    #[must_use]
    pub fn as_ptr(&self) -> *mut X509 {
        self.0.as_ptr()
    }

    /// The certificate, borrowed while this reference is held.
    #[must_use]
    pub fn get(&self) -> CertRef<'_> {
        CertRef(self.0, PhantomData)
    }
}

impl Drop for Cert {
    fn drop(&mut self) {
        /// The reference a dropped `Cert` held: only this drop makes one, once per
        /// handle, so `X509_free` releases each reference once.
        #[repr(transparent)]
        struct Released(NonNull<X509>);
        unsafe extern "C" {
            safe fn X509_free(cert: Released);
        }
        X509_free(Released(self.0));
    }
}

/// A new SSL whose one reference this side owns, freed when dropped unless a socket
/// record takes it ([`OwnedSsl::into_socket_ssl`]).
#[repr(transparent)]
#[derive(Debug)]
pub(crate) struct OwnedSsl(SocketSsl);

/// The one reference to a BIO an [`OwnedBio`] gave up, passed to `SSL_set_bio`, which
/// then takes it (once when passed as both BIOs): only [`OwnedSsl::set_bio`] makes
/// one.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Given(NonNull<BIO>);

/// An [`OwnedSsl`], lent exclusively for a call that may replace its BIOs.
#[repr(transparent)]
struct Fresh<'a>(NonNull<SSL>, PhantomData<&'a mut OwnedSsl>);

impl OwnedSsl {
    fn ptr(&self) -> NonNull<SSL> {
        self.0.as_non_null()
    }

    /// The SSL, borrowed while this side owns it.
    pub(crate) fn get(&self) -> Ssl<'_> {
        Ssl(self.ptr(), PhantomData)
    }

    /// `SSL_set_fd`: false when OpenSSL fails. The BIOs it frees are reachable only
    /// through borrows of `self`, which `&mut` excludes.
    pub(crate) fn set_fd(&mut self, fd: c_int) -> bool {
        SSL_set_fd(Fresh(self.ptr(), PhantomData), fd) != 0
    }

    /// `SSL_set_bio(ssl, read, write)`: the SSL takes `read`, and `write` or else
    /// `read` again for writes, and frees them with itself. The BIOs it frees are
    /// reachable only through borrows of `self`.
    pub(crate) fn set_bio(&mut self, read: OwnedBio, write: Option<OwnedBio>) {
        let read = Given(read.into_raw());
        let write = write.map_or(read, |write| Given(write.into_raw()));
        set_bios(Fresh(self.ptr(), PhantomData), read, write);
    }

    /// Records `socket`, which takes the SSL next, as its connection's ex data, which
    /// [`Ssl::socket`] reads: false when OpenSSL fails.
    pub(crate) fn set_connection(&mut self, socket: Socket<'_>) -> bool {
        set_connection(
            Fresh(self.ptr(), PhantomData),
            ConnectionIndex::get(),
            socket,
        ) != 0
    }

    /// The owner a socket record holds the SSL by.
    pub(crate) fn into_socket_ssl(self) -> SocketSsl {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{NonNull, PhantomData, SSL, Ssl};

    impl Ssl<'_> {
        /// # Safety
        /// `ssl` is null or a live SSL that nothing frees while the handle is used.
        #[must_use]
        pub(crate) unsafe fn new(ssl: *mut SSL) -> Option<Self> {
            NonNull::new(ssl).map(|ssl| Self(ssl, PhantomData))
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "calls OpenSSL, which Miri cannot execute")]
    fn clear_errors_empties_the_queue() {
        // A length of -1 reads to the NUL.
        let text = c"not a certificate";
        unsafe {
            let bio = openssl_sys::BIO_new_mem_buf(text.as_ptr().cast(), -1);
            let cert = openssl_sys::PEM_read_bio_X509(
                bio,
                std::ptr::null_mut(),
                None,
                std::ptr::null_mut(),
            );
            assert!(cert.is_null());
            openssl_sys::BIO_free_all(bio);
        }
        assert_ne!(super::peek_error(), 0, "the failed read queued an error");
        super::clear_errors();
        assert_eq!(super::peek_error(), 0);
        super::clear_errors();
        assert_eq!(super::peek_error(), 0);
    }
}
