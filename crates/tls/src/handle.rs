//! Handles over OpenSSL's objects, each wrapping its calls once: [`Ssl`] (a live
//! connection the socket record owns), [`Bio`] (a live BIO an SSL owns) and
//! [`OwnedBio`] (one this side owns until an SSL takes it), [`CertRef`] and [`Cert`]
//! (a borrowed and an owned certificate), and this thread's error queue. Callers go
//! through their methods. `Ssl<'a>` and `Bio<'a>` borrow their owner for `'a`, so a
//! handle can't outlive the record or SSL it came from, and the calls that need only a
//! live handle are declared `safe`. What frees an SSL or replaces its BIOs within
//! `'a` is unsafe: the crate's own constructors from raw pointers, `Ssl::set_fd`,
//! `Ssl::set_bio` and `upstream::socket_ssl_close`.
use crate::context::SslCtx;
use openssl_sys::{
    BIO, BIO_read, BIO_write, OPENSSL_STACK, SSL, SSL_CIPHER, SSL_CTRL_MODE,
    SSL_CTRL_SET_TLSEXT_HOSTNAME, SSL_CTX, SSL_peek, SSL_read, SSL_set_bio, SSL_write,
    TLSEXT_NAMETYPE_host_name, X509, X509_check_host, stack_st_X509,
};
use std::ffi::{CStr, c_char, c_int, c_long, c_ulong, c_void};
use std::marker::PhantomData;
use std::ptr::NonNull;
use swoole_rs_corebase::sys::ReadSlot;
use swoole_rs_reactor::{Socket, SocketFields};

// libssl's and libcrypto's own declarations, with each pointer typed as the
// handle it is: `Ssl`, `Bio` and `CertRef` are live for their lifetime, `CipherRef` is
// an entry of OpenSSL's static cipher table, `Cert` and `OwnedBio` take the reference
// OpenSSL returns, a `CText<'static>` is one of OpenSSL's static strings, and an
// `SslCtx` reference is a live context.
// The `safe` ones take only such handles and plain values, read or change only the
// object behind the handle or this thread's error queue, and return raw pointers the
// caller dereferences in its own unsafe block.
unsafe extern "C" {
    safe fn ERR_clear_error();
    safe fn ERR_peek_error() -> c_ulong;
    safe fn ERR_get_error() -> c_ulong;
    safe fn ERR_reason_error_string(error: c_ulong) -> Option<CText<'static>>;
    /// Writes at most `length` bytes, NUL-terminated, into `buffer`: the length is the
    /// buffer's own, the one value [`ErrorLength`] has.
    safe fn ERR_error_string_n(error: c_ulong, buffer: &mut [u8; 512], length: ErrorLength);
    safe fn BIO_s_mem() -> &'static BioMethod;
    /// A new BIO of `method`, its one reference the caller's; null on failure. The
    /// table lives for the process: OpenSSL's own, or `dtls.rs`'s, never freed once made,
    /// whose methods take a BIO without data as none of its session's.
    pub(crate) safe fn BIO_new(method: &'static BioMethod) -> Option<OwnedBio>;
    /// `fopen`s `filename` with `mode`; null on failure.
    safe fn BIO_new_file(filename: CText<'_>, mode: CText<'_>) -> Option<OwnedBio>;
    /// `BIO_pending` as a function: the bytes queued to read.
    safe fn BIO_ctrl_pending(bio: Bio<'_>) -> usize;
    safe fn BIO_method_type(bio: Bio<'_>) -> c_int;
    safe fn PEM_write_bio_X509(bio: Bio<'_>, cert: CertRef<'_>) -> c_int;
    safe fn SSL_get_error(ssl: Ssl<'_>, result: c_int) -> c_int;
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
    /// no callbacks that would read it.
    pub(crate) safe fn SSL_set_ex_data(ssl: Ssl<'_>, index: c_int, data: *mut c_void) -> c_int;
    /// The host name the client sent, null without one: the SSL's own copy.
    safe fn SSL_get_servername<'a>(ssl: Ssl<'a>, kind: c_int) -> Option<CText<'a>>;
    /// Null before the handshake, else an entry of OpenSSL's static cipher table.
    safe fn SSL_get_current_cipher(ssl: Ssl<'_>) -> Option<CipherRef>;
    /// Null names the `(NONE)` cipher, whose bits are 0.
    safe fn SSL_CIPHER_get_bits(cipher: Option<CipherRef>, algorithm: Option<&mut c_int>) -> c_int;
    safe fn SSL_CIPHER_get_name(cipher: Option<CipherRef>) -> Option<CText<'static>>;
    safe fn SSL_CIPHER_get_version(cipher: Option<CipherRef>) -> Option<CText<'static>>;
    /// `SslCtrl`'s commands: `Mode` reads no pointer, and `HostName` copies `text`
    /// (null clears the name).
    safe fn SSL_ctrl(
        ssl: Ssl<'_>,
        command: SslCtrl,
        value: c_long,
        text: Option<CText<'_>>,
    ) -> c_long;
    /// The SSL takes its own reference to `context` (null: its session's context)
    /// and releases the one it held.
    safe fn SSL_set_SSL_CTX(ssl: Ssl<'_>, context: Option<&SslCtx>) -> *mut SSL_CTX;
    /// One listen step, which writes the peer's address when given somewhere to.
    safe fn DTLSv1_listen(ssl: Ssl<'_>, peer: Option<&mut BioAddr>) -> c_int;
    /// Gives the fresh SSL socket BIOs over `fd`, freeing the ones it had, which only
    /// a borrow of the [`OwnedSsl`] the exclusive `Fresh` was taken from could reach.
    safe fn SSL_set_fd(ssl: Fresh<'_>, fd: c_int) -> c_int;
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

/// A `BIO_METHOD` table. It is zero-sized, so a reference to it claims none of the
/// bytes OpenSSL keeps behind it, which only its setters write, before any BIO is made
/// of it.
#[repr(C)]
pub(crate) struct BioMethod {
    _opaque: [u8; 0],
}

/// A `BIO_ADDR`, which this side never makes.
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

/// A NUL-terminated string that stays live and unchanged for `'a`, as thin as the
/// `const char *` it is: what a declaration takes or returns in place of the raw pointer.
/// Safe code makes one only from a `&CStr`.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct CText<'a>(NonNull<c_char>, PhantomData<&'a CStr>);

impl<'a> From<&'a CStr> for CText<'a> {
    fn from(text: &'a CStr) -> Self {
        Self(NonNull::from(text).cast(), PhantomData)
    }
}

impl<'a> CText<'a> {
    pub(crate) fn get(self) -> &'a CStr {
        // SAFETY: the type's invariant: a NUL-terminated string live for `'a`.
        unsafe { CStr::from_ptr(self.0.as_ptr()) }
    }
}

/// The int OpenSSL's I/O calls take for a buffer's length: at most `INT_MAX`, where
/// upstream's `(int)` cast would wrap a buffer over 2 GiB.
fn int(length: usize) -> c_int {
    c_int::try_from(length).unwrap_or(c_int::MAX)
}

/// Clears this thread's OpenSSL error queue.
pub fn clear_errors() {
    ERR_clear_error();
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
    ERR_reason_error_string(error).map(CText::get)
}

/// `ERR_error_string_n`'s length: that of its 512-byte buffer, as a `size_t`.
#[repr(usize)]
enum ErrorLength {
    Buffer = 512,
}

/// `ERR_error_string_n` of `error` into a 512-byte buffer, as upstream's logs print it.
#[must_use]
pub fn error_string(error: c_ulong) -> String {
    let mut buffer = [0_u8; 512];
    ERR_error_string_n(error, &mut buffer, ErrorLength::Buffer);
    CStr::from_bytes_until_nul(&buffer)
        .map_or_else(|_| String::new(), |text| text.to_string_lossy().into())
}

/// A live SSL on this thread for `'a`. A socket record's is live while it is set:
/// `Socket::ssl_create` sets it, and the unsafe `socket_ssl_close` frees it and clears
/// it, its caller promising that no handle to the SSL or its BIOs is used afterwards.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ssl<'a>(NonNull<SSL>, PhantomData<&'a SSL>);

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
    /// # Safety
    /// `ssl` is null or a live SSL that nothing frees for `'a`.
    #[must_use]
    pub(crate) unsafe fn new(ssl: *mut SSL) -> Option<Self> {
        NonNull::new(ssl).map(|ssl| Self(ssl, PhantomData))
    }

    /// The SSL of `socket`'s record, or None without one, for as long as the record.
    #[must_use]
    pub fn of(socket: Socket<'a>) -> Option<Self> {
        Self::of_reading(socket, |_| ()).0
    }

    /// [`Self::of`], and `read` over the record's fields in the same read of it.
    pub fn of_reading<T>(
        socket: Socket<'a>,
        read: impl FnOnce(&SocketFields) -> T,
    ) -> (Option<Self>, T) {
        let (ssl, value) = socket.read(|record| (record.ssl(), read(record)));
        // SAFETY: the record's `ssl` is null or a live SSL it owns: only the unsafe
        // `set_ssl` (whose caller promises that) and native code (`Socket::from_ptr`'s
        // contract) set it, and only the unsafe `socket_ssl_close` frees it, its caller
        // promising that no handle to it is used afterwards.
        (unsafe { Self::new(ssl.cast()) }, value)
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

    /// `SSL_read` into `buffer`, which may be uninitialized: OpenSSL only writes it.
    #[must_use]
    pub fn read(self, buffer: &mut [impl ReadSlot]) -> c_int {
        let length = int(buffer.len());
        // SAFETY: a live SSL, into the buffer's writable bytes.
        unsafe { SSL_read(self.as_ptr(), buffer.as_mut_ptr().cast(), length) }
    }

    /// `SSL_write` of `data`.
    #[must_use]
    pub fn write(self, data: &[u8]) -> c_int {
        // SAFETY: a live SSL, from the data's readable bytes.
        unsafe { SSL_write(self.as_ptr(), data.as_ptr().cast(), int(data.len())) }
    }

    /// `SSL_peek` into `buffer`.
    #[must_use]
    pub fn peek(self, buffer: &mut [impl ReadSlot]) -> c_int {
        let length = int(buffer.len());
        // SAFETY: a live SSL, into the buffer's writable bytes.
        unsafe { SSL_peek(self.as_ptr(), buffer.as_mut_ptr().cast(), length) }
    }

    /// One `SSL_accept` step.
    #[must_use]
    pub fn accept(self) -> c_int {
        SSL_accept(self)
    }

    /// One `SSL_connect` step.
    #[must_use]
    pub fn connect(self) -> c_int {
        SSL_connect(self)
    }

    /// One `SSL_do_handshake` step.
    #[must_use]
    pub fn do_handshake(self) -> c_int {
        SSL_do_handshake(self)
    }

    /// `SSL_shutdown`.
    #[must_use]
    pub fn shutdown(self) -> c_int {
        SSL_shutdown(self)
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
            name: name.map_or(c"(NONE)", CText::get),
            version: version.map_or(c"(NONE)", CText::get),
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

    /// `SSL_set_bio`: the SSL takes both BIOs (one reference when they are the same
    /// BIO) and frees them with itself; the handles borrow them meanwhile.
    ///
    /// # Safety
    /// No handle to the SSL's current BIOs is used afterwards: it frees them.
    pub unsafe fn set_bio(self, read: OwnedBio, write: Option<OwnedBio>) -> (Bio<'a>, Bio<'a>) {
        let read = read.into_raw();
        let write = write.map_or(read, OwnedBio::into_raw);
        // SAFETY: a live SSL takes the references this side owned, and frees the BIOs
        // it replaces, which the caller promises have no handle in use.
        unsafe { SSL_set_bio(self.as_ptr(), read.as_ptr(), write.as_ptr()) };
        (Bio(read, PhantomData), Bio(write, PhantomData))
    }

    /// `SSL_get_rbio` (`get0`), None without one: live until the SSL is freed or the
    /// unsafe `set_bio`/`set_fd` replace it.
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

    /// `SSL_set_ex_data` at `index`: false when OpenSSL fails. Also false, setting
    /// nothing, at the connection's index: the info callback reads that entry as the
    /// SSL's socket record, so only [`crate::socket_ssl_new`] sets it.
    #[must_use]
    pub fn set_ex_data(self, index: c_int, data: *mut c_void) -> bool {
        index != crate::swr_v1_tls_connection_index() && SSL_set_ex_data(self, index, data) != 0
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

    /// One `DTLSv1_listen` step, without the peer's address.
    #[must_use]
    pub fn dtls_listen(self) -> c_int {
        DTLSv1_listen(self, None)
    }
}

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
        /// OpenSSL's `BIO_TYPE_MEM`: `BIO_TYPE_SOURCE_SINK` (0x0400) with index 1.
        const BIO_TYPE_MEM: c_int = 0x0401;
        BIO_method_type(self) == BIO_TYPE_MEM
    }

    /// `BIO_read` into `buffer`.
    #[must_use]
    pub fn read(self, buffer: &mut [impl ReadSlot]) -> c_int {
        let length = int(buffer.len());
        // SAFETY: a live BIO, into the buffer's writable bytes.
        unsafe { BIO_read(self.as_ptr(), buffer.as_mut_ptr().cast(), length) }
    }

    /// `BIO_write` of `data`.
    #[must_use]
    pub fn write(self, data: &[u8]) -> c_int {
        // SAFETY: a live BIO, from the data's readable bytes.
        unsafe { BIO_write(self.as_ptr(), data.as_ptr().cast(), int(data.len())) }
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
    /// # Safety
    /// `cert` is null or a live certificate, not freed while the handle is used.
    #[cfg(test)]
    #[must_use]
    pub(crate) unsafe fn new(cert: *mut X509) -> Option<Self> {
        NonNull::new(cert).map(|cert| Self(cert, PhantomData))
    }

    #[must_use]
    pub fn as_ptr(self) -> *mut X509 {
        self.0.as_ptr()
    }

    /// `Socket::ssl_check_host`'s test: whether the certificate matches the host
    /// `name` under `X509_check_host`.
    #[must_use]
    pub fn matches_host(self, name: &[u8]) -> bool {
        let (at, length, none) = (name.as_ptr().cast(), name.len(), std::ptr::null_mut());
        // SAFETY: the live certificate and the name's bytes.
        let matched = unsafe { X509_check_host(self.as_ptr(), at, length, 0, none) };
        matched == 1
    }

    /// `_ssl_read_x509_file`: the certificate as PEM into at most `buffer`'s length.
    ///
    /// # Errors
    /// The [`PemError`] step that failed.
    pub fn pem(self, buffer: &mut [impl ReadSlot]) -> Result<usize, PemError> {
        let bio = OwnedBio::memory().ok_or(PemError::Bio)?;
        if PEM_write_bio_X509(bio.bio(), self) == 0 {
            return Err(PemError::Write);
        }
        let count = bio.bio().pending().min(buffer.len());
        let read = bio.bio().read(buffer.get_mut(..count).unwrap_or_default());
        usize::try_from(read).map_err(|_| PemError::Read)
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
/// record takes it ([`OwnedSsl::into_raw`]).
#[repr(transparent)]
#[derive(Debug)]
pub(crate) struct OwnedSsl(NonNull<SSL>);

/// An [`OwnedSsl`], lent exclusively for a call that may replace its BIOs.
#[repr(transparent)]
struct Fresh<'a>(NonNull<SSL>, PhantomData<&'a mut OwnedSsl>);

impl OwnedSsl {
    /// The SSL, borrowed while this side owns it.
    pub(crate) fn get(&self) -> Ssl<'_> {
        Ssl(self.0, PhantomData)
    }

    /// `SSL_set_fd`: false when OpenSSL fails. The BIOs it frees are reachable only
    /// through borrows of `self`, which `&mut` excludes.
    pub(crate) fn set_fd(&mut self, fd: c_int) -> bool {
        SSL_set_fd(Fresh(self.0, PhantomData), fd) != 0
    }

    /// The SSL, whose reference the caller hands on.
    pub(crate) fn into_raw(self) -> *mut SSL {
        std::mem::ManuallyDrop::new(self).0.as_ptr()
    }
}

impl Drop for OwnedSsl {
    fn drop(&mut self) {
        /// The reference a dropped `OwnedSsl` held: only this drop makes one, once per
        /// handle, so `SSL_free` releases each SSL once.
        #[repr(transparent)]
        struct Released(NonNull<SSL>);
        unsafe extern "C" {
            safe fn SSL_free(ssl: Released);
        }
        SSL_free(Released(self.0));
    }
}
