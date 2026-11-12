//! Handles over OpenSSL's objects, each wrapping its calls once: [`Ssl`] (a live
//! connection the socket record owns), [`Bio`] (a live BIO an SSL owns) and
//! [`OwnedBio`] (one this side owns until an SSL takes it), [`CertRef`] and [`Cert`]
//! (a borrowed and an owned certificate), and this thread's error queue. Callers go
//! through their methods. A handle borrows its owner for `'a`, and what frees the
//! object under it is unsafe: `socket_ssl_close` (the SSL), [`Ssl::set_bio`] and
//! [`Ssl::set_fd`] (the BIOs they replace); dropping an [`OwnedBio`] or a [`Cert`]
//! ends its borrows.
use openssl_sys::{
    BIO, BIO_ctrl, BIO_free_all, BIO_new, BIO_read, BIO_s_mem, BIO_write, ERR_clear_error,
    ERR_get_error, ERR_reason_error_string, OPENSSL_STACK, OPENSSL_sk_num, OPENSSL_sk_value,
    PEM_write_bio_X509, SSL, SSL_CIPHER_get_bits, SSL_CIPHER_get_name, SSL_CIPHER_get_version,
    SSL_CTRL_MODE, SSL_CTX, SSL_accept, SSL_connect, SSL_ctrl, SSL_do_handshake,
    SSL_get_current_cipher, SSL_get_error, SSL_get_ex_data, SSL_get_rbio, SSL_get_servername,
    SSL_get_shutdown, SSL_get_verify_result, SSL_get_wbio, SSL_peek, SSL_read, SSL_set_SSL_CTX,
    SSL_set_accept_state, SSL_set_bio, SSL_set_connect_state, SSL_set_ex_data, SSL_set_shutdown,
    SSL_shutdown, SSL_version, SSL_write, TLSEXT_NAMETYPE_host_name, X509, X509_check_host,
    X509_free, stack_st_X509,
};
use std::ffi::{CStr, c_char, c_int, c_long, c_ulong, c_void};
use std::marker::PhantomData;
use std::ptr::NonNull;
use swoole_rs_corebase::sys::ReadSlot;
use swoole_rs_reactor::{Socket, SocketFields};

// SAFETY: libssl's and libcrypto's own declarations; openssl-sys binds none of them.
unsafe extern "C" {
    fn ERR_peek_error() -> c_ulong;
    fn ERR_error_string_n(error: c_ulong, buffer: *mut c_char, length: usize);
    fn SSL_in_init(ssl: *const SSL) -> c_int;
    fn SSL_set_quiet_shutdown(ssl: *mut SSL, mode: c_int);
    fn SSL_get_peer_cert_chain(ssl: *const SSL) -> *mut stack_st_X509;
    fn DTLSv1_listen(ssl: *mut SSL, peer: *mut c_void) -> c_int;
    fn SSL_set_fd(ssl: *mut SSL, fd: c_int) -> c_int;
    /// `SSL_get_peer_certificate`, a macro over `SSL_get1_peer_certificate` since
    /// OpenSSL 3 (build.rs).
    #[cfg_attr(ossl300, link_name = "SSL_get1_peer_certificate")]
    #[cfg_attr(not(ossl300), link_name = "SSL_get_peer_certificate")]
    fn peer_certificate(ssl: *const SSL) -> *mut X509;
    fn BIO_int_ctrl(bio: *mut BIO, command: c_int, large: c_long, integer: c_int) -> c_long;
}

/// `BIO_CTRL_PENDING`, behind the `BIO_pending` macro.
const BIO_CTRL_PENDING: c_int = 10;

/// The int OpenSSL's I/O calls take for a buffer's length: at most `INT_MAX`, where
/// upstream's `(int)` cast would wrap a buffer over 2 GiB.
fn int(length: usize) -> c_int {
    c_int::try_from(length).unwrap_or(c_int::MAX)
}

/// Clears this thread's OpenSSL error queue.
pub fn clear_errors() {
    // SAFETY: this thread's queue; no preconditions.
    unsafe { ERR_clear_error() };
}

/// This thread's oldest queued OpenSSL error, left queued.
#[must_use]
pub fn peek_error() -> c_ulong {
    // SAFETY: this thread's queue; no preconditions.
    unsafe { ERR_peek_error() }
}

/// This thread's oldest queued OpenSSL error, popped.
#[must_use]
pub fn pop_error() -> c_ulong {
    // SAFETY: this thread's queue; no preconditions.
    unsafe { ERR_get_error() }
}

/// OpenSSL's reason string for `error`, None for an unknown code.
#[must_use]
pub fn reason_string(error: c_ulong) -> Option<&'static CStr> {
    // SAFETY: OpenSSL's static string table, null for an unknown code.
    let reason = unsafe { ERR_reason_error_string(error) };
    // SAFETY: a static NUL-terminated string when not null.
    (!reason.is_null()).then(|| unsafe { CStr::from_ptr(reason) })
}

/// `ERR_error_string_n` of `error` into a 512-byte buffer, as upstream's logs print it.
#[must_use]
pub fn error_string(error: c_ulong) -> String {
    let mut buffer = [0_u8; 512];
    // SAFETY: at most the buffer's length, NUL-terminated.
    unsafe { ERR_error_string_n(error, buffer.as_mut_ptr().cast(), buffer.len()) };
    CStr::from_bytes_until_nul(&buffer)
        .map_or_else(|_| String::new(), |text| text.to_string_lossy().into())
}

/// A live SSL on this thread for `'a`. A socket record's is live while it is set:
/// `Socket::ssl_create` sets it, and the unsafe `socket_ssl_close` frees it and
/// clears it, its caller promising no handle of it is used afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ssl<'a>(NonNull<SSL>, PhantomData<&'a SSL>);

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
    pub unsafe fn new(ssl: *mut SSL) -> Option<Self> {
        NonNull::new(ssl).map(|ssl| Self(ssl, PhantomData))
    }

    /// The SSL of `socket`'s record, or None without one, for as long as the record.
    #[must_use]
    pub fn of(socket: Socket<'a>) -> Option<Self> {
        let ssl = socket.read(SocketFields::ssl);
        // SAFETY: the record's `ssl` is null or a live SSL it owns: only the unsafe
        // `set_ssl` (whose caller promises that) and native code (`Socket::from_ptr`'s
        // contract) set it, and only the unsafe `socket_ssl_close` frees it, whose
        // caller promises no handle of it is used afterwards.
        unsafe { Self::new(ssl.cast()) }
    }

    #[must_use]
    pub fn as_ptr(self) -> *mut SSL {
        self.0.as_ptr()
    }

    /// `SSL_get_error` for its last I/O result.
    #[must_use]
    pub fn error(self, result: c_int) -> c_int {
        // SAFETY: a live SSL, and its last I/O result.
        unsafe { SSL_get_error(self.as_ptr(), result) }
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
        // SAFETY: a live SSL.
        unsafe { SSL_accept(self.as_ptr()) }
    }

    /// One `SSL_connect` step.
    #[must_use]
    pub fn connect(self) -> c_int {
        // SAFETY: a live SSL.
        unsafe { SSL_connect(self.as_ptr()) }
    }

    /// One `SSL_do_handshake` step.
    #[must_use]
    pub fn do_handshake(self) -> c_int {
        // SAFETY: a live SSL.
        unsafe { SSL_do_handshake(self.as_ptr()) }
    }

    /// `SSL_shutdown`.
    #[must_use]
    pub fn shutdown(self) -> c_int {
        // SAFETY: a live SSL.
        unsafe { SSL_shutdown(self.as_ptr()) }
    }

    /// `SSL_in_init`: whether the handshake is still running.
    #[must_use]
    pub fn in_init(self) -> bool {
        // SAFETY: a live SSL.
        unsafe { SSL_in_init(self.as_ptr()) != 0 }
    }

    /// `SSL_set_quiet_shutdown(ssl, 1)`.
    pub fn set_quiet_shutdown(self) {
        // SAFETY: a live SSL.
        unsafe { SSL_set_quiet_shutdown(self.as_ptr(), 1) };
    }

    /// `SSL_get_shutdown`.
    #[must_use]
    pub fn shutdown_mode(self) -> c_int {
        // SAFETY: a live SSL.
        unsafe { SSL_get_shutdown(self.as_ptr()) }
    }

    /// `SSL_set_shutdown`.
    pub fn set_shutdown_mode(self, mode: c_int) {
        // SAFETY: a live SSL.
        unsafe { SSL_set_shutdown(self.as_ptr(), mode) };
    }

    /// `SSL_get_verify_result`.
    #[must_use]
    pub fn verify_result(self) -> c_long {
        // SAFETY: a live SSL.
        unsafe { SSL_get_verify_result(self.as_ptr()) }
    }

    /// The peer's certificate, a reference this side then owns.
    #[must_use]
    pub fn peer_certificate(self) -> Option<Cert> {
        // SAFETY: a live SSL; the reference it returns is ours.
        NonNull::new(unsafe { peer_certificate(self.as_ptr()) }).map(Cert)
    }

    /// `SSL_get_peer_cert_chain`: the SSL's own chain, null without one.
    #[must_use]
    pub fn peer_cert_chain(self) -> *mut stack_st_X509 {
        // SAFETY: a live SSL.
        unsafe { SSL_get_peer_cert_chain(self.as_ptr()) }
    }

    /// `f` over each certificate of the peer's chain, in order.
    pub fn each_peer_cert(self, mut f: impl FnMut(CertRef<'_>)) {
        let chain = self.peer_cert_chain().cast::<OPENSSL_STACK>();
        if chain.is_null() {
            return;
        }
        // SAFETY: the SSL's live chain.
        let count = unsafe { OPENSSL_sk_num(chain) };
        for index in 0..count {
            // SAFETY: an index within the live chain, whose entries are certificates.
            let cert = unsafe { OPENSSL_sk_value(chain, index) };
            if let Some(cert) = NonNull::new(cert.cast()) {
                f(CertRef(cert, PhantomData));
            }
        }
    }

    /// `SSL_version`: `TLS1_3_VERSION` and the like.
    #[must_use]
    pub fn version(self) -> c_int {
        // SAFETY: a live SSL.
        unsafe { SSL_version(self.as_ptr()) }
    }

    /// The current cipher's bits, name and version.
    #[must_use]
    pub fn cipher(self) -> Cipher {
        // SAFETY: a live SSL; null before the handshake, which the getters below take.
        let cipher = unsafe { SSL_get_current_cipher(self.as_ptr()) };
        // SAFETY: null or an entry of OpenSSL's static cipher table.
        let bits = unsafe { SSL_CIPHER_get_bits(cipher, std::ptr::null_mut()) };
        // SAFETY: as above.
        let name = unsafe { SSL_CIPHER_get_name(cipher) };
        // SAFETY: as above.
        let version = unsafe { SSL_CIPHER_get_version(cipher) };
        Cipher {
            bits,
            // SAFETY: the table's name or the `(NONE)` literal: static and NUL-terminated.
            name: unsafe { CStr::from_ptr(name) },
            // SAFETY: a static NUL-terminated literal.
            version: unsafe { CStr::from_ptr(version) },
        }
    }

    /// `SSL_set_mode`: adds `mode`'s bits.
    pub fn set_mode(self, mode: c_long) {
        // SAFETY: a live SSL; `SSL_set_mode` is this `SSL_ctrl`.
        unsafe { SSL_ctrl(self.as_ptr(), SSL_CTRL_MODE, mode, std::ptr::null_mut()) };
    }

    /// `SSL_set_fd`: false when OpenSSL fails.
    ///
    /// # Safety
    /// No handle of the SSL's earlier BIOs, which this frees, is used afterwards.
    #[must_use]
    pub unsafe fn set_fd(self, fd: c_int) -> bool {
        // SAFETY: a live SSL over the caller's descriptor; the caller drops the old BIOs' handles.
        unsafe { SSL_set_fd(self.as_ptr(), fd) != 0 }
    }

    /// `SSL_set_accept_state`.
    pub fn set_accept_state(self) {
        // SAFETY: a live SSL.
        unsafe { SSL_set_accept_state(self.as_ptr()) };
    }

    /// `SSL_set_connect_state`.
    pub fn set_connect_state(self) {
        // SAFETY: a live SSL.
        unsafe { SSL_set_connect_state(self.as_ptr()) };
    }

    /// `SSL_set_bio`: the SSL takes both BIOs (one reference when they are the same
    /// BIO) and frees them with itself; the handles borrow them meanwhile.
    ///
    /// # Safety
    /// No handle of the SSL's earlier BIOs, which this frees, is used afterwards.
    pub unsafe fn set_bio(self, read: OwnedBio, write: Option<OwnedBio>) -> (Bio<'a>, Bio<'a>) {
        let read = read.into_raw();
        let write = write.map_or(read, OwnedBio::into_raw);
        // SAFETY: a live SSL takes the references this side owned; the caller drops
        // the old BIOs' handles.
        unsafe { SSL_set_bio(self.as_ptr(), read.as_ptr(), write.as_ptr()) };
        (Bio(read, PhantomData), Bio(write, PhantomData))
    }

    /// `SSL_get_rbio`, None without one.
    #[must_use]
    pub fn read_bio(self) -> Option<Bio<'a>> {
        // SAFETY: a live SSL; the BIO it owns lives until the SSL is freed or the
        // unsafe `set_bio`/`set_fd` replace it.
        NonNull::new(unsafe { SSL_get_rbio(self.as_ptr()) }).map(|bio| Bio(bio, PhantomData))
    }

    /// `SSL_get_wbio`, None without one.
    #[must_use]
    pub fn write_bio(self) -> Option<Bio<'a>> {
        // SAFETY: as in `read_bio`.
        NonNull::new(unsafe { SSL_get_wbio(self.as_ptr()) }).map(|bio| Bio(bio, PhantomData))
    }

    /// `SSL_get_ex_data` at `index`.
    #[must_use]
    pub fn ex_data(self, index: c_int) -> *mut c_void {
        // SAFETY: a live SSL; the slot is opaque to OpenSSL.
        unsafe { SSL_get_ex_data(self.as_ptr(), index) }
    }

    /// `SSL_set_ex_data` at `index`: false when OpenSSL fails.
    #[must_use]
    #[expect(
        clippy::not_unsafe_ptr_arg_deref,
        reason = "OpenSSL keeps `data` as opaque ex data and never dereferences it"
    )]
    pub fn set_ex_data(self, index: c_int, data: *mut c_void) -> bool {
        // SAFETY: a live SSL keeps `data` as opaque ex data.
        unsafe { SSL_set_ex_data(self.as_ptr(), index, data) != 0 }
    }

    /// The SNI host name the client sent, None without one.
    #[must_use]
    pub fn servername(self) -> Option<Vec<u8>> {
        // SAFETY: a live SSL.
        let name = unsafe { SSL_get_servername(self.as_ptr(), TLSEXT_NAMETYPE_host_name) };
        // SAFETY: a NUL-terminated name the SSL holds, copied before it changes.
        (!name.is_null()).then(|| unsafe { CStr::from_ptr(name) }.to_bytes().to_vec())
    }

    /// `SSL_set_SSL_CTX`: the SSL switches to `context`, which it then references.
    ///
    /// # Safety
    /// `context` is a live `SSL_CTX`.
    pub unsafe fn set_context(self, context: *mut SSL_CTX) {
        // SAFETY: a live SSL and, as the caller promises, a live context.
        unsafe { SSL_set_SSL_CTX(self.as_ptr(), context) };
    }

    /// `SSL_set_tlsext_host_name`: false when OpenSSL refuses the name.
    #[must_use]
    pub fn set_host_name(self, name: &CStr) -> bool {
        // SAFETY: a live SSL copies the NUL-terminated name.
        unsafe {
            openssl_sys::SSL_set_tlsext_host_name(self.as_ptr(), name.as_ptr().cast_mut()) != 0
        }
    }

    /// One `DTLSv1_listen` step, without the peer's address.
    #[must_use]
    pub fn dtls_listen(self) -> c_int {
        // SAFETY: a live SSL; OpenSSL takes a null peer.
        unsafe { DTLSv1_listen(self.as_ptr(), std::ptr::null_mut()) }
    }
}

/// A live BIO, borrowed for `'a` from the SSL or [`OwnedBio`] that owns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bio<'a>(NonNull<BIO>, PhantomData<&'a BIO>);

impl Bio<'_> {
    /// # Safety
    /// `bio` is null or a live BIO that nothing frees for the handle's lifetime.
    #[must_use]
    pub unsafe fn new(bio: *mut BIO) -> Option<Self> {
        NonNull::new(bio).map(|bio| Self(bio, PhantomData))
    }

    #[must_use]
    pub fn as_ptr(self) -> *mut BIO {
        self.0.as_ptr()
    }

    /// `BIO_pending`: the bytes queued to read.
    #[must_use]
    pub fn pending(self) -> usize {
        // SAFETY: a live BIO; BIO_pending.
        let pending = unsafe { BIO_ctrl(self.as_ptr(), BIO_CTRL_PENDING, 0, std::ptr::null_mut()) };
        usize::try_from(pending).unwrap_or(0)
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
        // SAFETY: a live BIO; a BIO that doesn't buffer ignores the command.
        unsafe { BIO_int_ctrl(self.as_ptr(), 117, size, 1) };
    }
}

/// A BIO this side owns, freed when dropped unless an SSL takes it.
#[derive(Debug)]
pub struct OwnedBio(NonNull<BIO>);

impl OwnedBio {
    /// A new memory BIO, None when OpenSSL fails.
    #[must_use]
    pub fn memory() -> Option<Self> {
        // SAFETY: OpenSSL's static memory BIO method.
        let method = unsafe { BIO_s_mem() };
        // SAFETY: a fresh BIO of that method, null on failure.
        NonNull::new(unsafe { BIO_new(method) }).map(Self)
    }

    /// A BIO this side owns from now on.
    ///
    /// # Safety
    /// `bio` is null or a live BIO whose reference nothing else releases.
    #[must_use]
    pub unsafe fn from_raw(bio: *mut BIO) -> Option<Self> {
        NonNull::new(bio).map(Self)
    }

    /// The BIO, borrowed while this side owns it.
    #[must_use]
    pub fn bio(&self) -> Bio<'_> {
        Bio(self.0, PhantomData)
    }

    /// The BIO, whose reference the caller hands on.
    fn into_raw(self) -> NonNull<BIO> {
        std::mem::ManuallyDrop::new(self).0
    }
}

impl Drop for OwnedBio {
    fn drop(&mut self) {
        // SAFETY: releases the reference this side owns.
        unsafe { BIO_free_all(self.0.as_ptr()) };
    }
}

/// A live certificate, borrowed for `'a`.
#[derive(Clone, Copy, Debug)]
pub struct CertRef<'a>(NonNull<X509>, PhantomData<&'a X509>);

impl CertRef<'_> {
    /// # Safety
    /// `cert` is null or a live certificate, not freed while the handle is used.
    #[must_use]
    pub unsafe fn new(cert: *mut X509) -> Option<Self> {
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
    /// Returns the bytes written, or -1 when `BIO_new` failed and -2 when
    /// `PEM_write_bio_X509` failed.
    #[must_use]
    pub fn pem(self, buffer: &mut [impl ReadSlot]) -> c_int {
        let Some(bio) = OwnedBio::memory() else {
            return -1;
        };
        // SAFETY: the live certificate into the fresh BIO.
        if unsafe { PEM_write_bio_X509(bio.bio().as_ptr(), self.as_ptr()) } == 0 {
            return -2;
        }
        let count = bio.bio().pending().min(buffer.len());
        bio.bio().read(buffer.get_mut(..count).unwrap_or_default())
    }
}

/// A live certificate reference this side owns, released when dropped.
#[derive(Debug)]
pub struct Cert(pub(crate) NonNull<X509>);

impl Cert {
    /// A reference this side owns from now on.
    ///
    /// # Safety
    /// `cert` is null or a live certificate whose reference nothing else releases.
    #[must_use]
    pub unsafe fn from_raw(cert: *mut X509) -> Option<Self> {
        NonNull::new(cert).map(Self)
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
        // SAFETY: drops the reference this side owns.
        unsafe { X509_free(self.as_ptr()) };
    }
}
