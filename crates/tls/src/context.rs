//! `swoole::SSLContext`'s OpenSSL side: library initialization and the `ex_data`
//! indexes, the error text, creating and configuring the `SSL_CTX`, and its
//! callbacks (password, ALPN, verify, DTLS cookies). The configuration lives in the
//! owner (`owned.rs`), the info callback in `info.rs`; warnings go through the
//! configuration's `warn` callback. Requires OpenSSL 3.
use crate::handle::{Bio, CText, Ssl};
use openssl_sys::{
    EVP_PKEY, OPENSSL_NPN_NEGOTIATED, SSL, SSL_CTRL_MODE, SSL_CTRL_SET_GROUPS_LIST,
    SSL_CTRL_SET_READ_AHEAD, SSL_CTRL_SET_SESS_CACHE_MODE, SSL_CTRL_SET_TLSEXT_SERVERNAME_CB,
    SSL_CTX, SSL_FILETYPE_PEM, SSL_MODE_NO_AUTO_CHAIN, SSL_MODE_RELEASE_BUFFERS,
    SSL_OP_CIPHER_SERVER_PREFERENCE, SSL_OP_DONT_INSERT_EMPTY_FRAGMENTS, SSL_OP_NO_COMPRESSION,
    SSL_OP_NO_SSLv2, SSL_OP_NO_SSLv3, SSL_OP_NO_TLSv1, SSL_OP_NO_TLSv1_1, SSL_OP_NO_TLSv1_2,
    SSL_OP_NO_TLSv1_3, SSL_SESS_CACHE_SERVER, SSL_TLSEXT_ERR_NOACK, SSL_TLSEXT_ERR_OK,
    SSL_VERIFY_NONE, SSL_VERIFY_PEER, SSL_select_next_proto, X509_STORE_CTX,
};
use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_int, c_long, c_uchar, c_uint, c_void};
use std::marker::{PhantomData, PhantomPinned};
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::StringSlot;
use swoole_rs_corebase::log;

// libssl's own declarations, which openssl-sys doesn't bind.
unsafe extern "C" {
    fn SSL_CTX_set_default_passwd_cb(ctx: *mut SSL_CTX, callback: PasswordCallback);
    fn SSL_CTX_set_default_passwd_cb_userdata(ctx: *mut SSL_CTX, data: *mut c_void);
}

// libssl/libcrypto's declarations, typed so that a live context is a
// reference and a NUL-terminated string a `CText`: each reads only its arguments
// (copying what it keeps of a string), `SSL_CTX_new` returns its new context's one
// reference, or null, and `SSL_load_client_CA_file` a new list, which
// `SSL_CTX_set_client_CA_list` takes over.
unsafe extern "C" {
    safe fn SSL_CTX_load_verify_locations(
        ctx: &SslCtx,
        file: Option<CText<'_>>,
        path: Option<CText<'_>>,
    ) -> c_int;
    safe fn SSL_CTX_set_cipher_list(ctx: &SslCtx, ciphers: CText<'_>) -> c_int;
    safe fn SSL_CTX_use_certificate_file(ctx: &SslCtx, file: CText<'_>, kind: c_int) -> c_int;
    safe fn SSL_CTX_use_certificate_chain_file(ctx: &SslCtx, file: CText<'_>) -> c_int;
    safe fn SSL_CTX_use_PrivateKey_file(ctx: &SslCtx, file: CText<'_>, kind: c_int) -> c_int;
    safe fn SSL_load_client_CA_file(file: CText<'_>) -> Option<ClientCaNames>;
    safe fn SSL_CTX_set_client_CA_list(ctx: &SslCtx, names: ClientCaNames);
    // Reads the BIO for new parameters, its one reference the caller's, or null; it
    // reuses no key, since this side has no slot to pass.
    safe fn PEM_read_bio_Parameters(bio: Bio<'_>, reuse: Option<&mut KeySlot>) -> Option<DhParams>;
    // Drops the caller's reference, which the value was.
    safe fn EVP_PKEY_free(key: DhParams);
    // Copies `length` bytes of the id, or fails for more than `SSL_MAX_SID_CTX_LENGTH`,
    // the array's 32.
    safe fn SSL_CTX_set_session_id_context(ctx: &SslCtx, id: &[u8; 32], length: c_uint) -> c_int;
    // Pops each of this thread's queued errors and calls `callback` with its line, which
    // it formats into a buffer and lends NUL-terminated, the line's `strlen` and `user`.
    safe fn ERR_print_errors_cb(callback: ErrorLine, user: Option<&'static c_void>);

    safe fn TLS_method() -> &'static SslMethod;
    safe fn DTLS_method() -> &'static SslMethod;
    safe fn SSL_CTX_new(method: &SslMethod) -> Option<SslContext>;
    safe fn SSL_CTX_up_ref(ctx: &SslCtx) -> c_int;
    safe fn SSL_CTX_set_options(ctx: &SslCtx, options: Options) -> Options;
    safe fn SSL_CTX_clear_options(ctx: &SslCtx, options: Options) -> Options;
    safe fn SSL_CTX_set_verify(ctx: &SslCtx, mode: c_int, callback: Option<VerifyCallback>);
    safe fn SSL_CTX_set_verify_depth(ctx: &SslCtx, depth: c_int);
    safe fn SSL_CTX_set_default_verify_paths(ctx: &SslCtx) -> c_int;
    safe fn SSL_CTX_check_private_key(ctx: &SslCtx) -> c_int;
    /// libcrypto's static SHA-1 method, null when it has none.
    safe fn EVP_sha1() -> Option<&'static EvpMd>;
    /// HMAC of `md` over the key's and data's lengths' bytes into `out`, whose 255
    /// bytes exceed `EVP_MAX_MD_SIZE`, and the digest's length into `out_length`: `out`,
    /// or null on failure. Each length is at most its array's.
    safe fn HMAC(
        md: &EvpMd,
        key: &[u8; 32],
        key_length: KeyLength,
        data: &[u8; 18],
        data_length: DataLength,
        out: &mut [u8; 255],
        out_length: &mut c_uint,
    ) -> *mut c_uchar;
    /// `Ctrl`'s commands: `GroupsList` parses `text`, and the others read only `value`.
    safe fn SSL_CTX_ctrl(ctx: &SslCtx, command: Ctrl, value: c_long, text: CText<'_>) -> c_long;
    /// `ServernameCallback`, the one command, takes a callback of its type, which
    /// OpenSSL calls with the live, non-null SSL whose client hello it is reading.
    safe fn SSL_CTX_callback_ctrl(
        ctx: &SslCtx,
        command: CallbackCtrl,
        callback: Option<ServernameCallback>,
    ) -> c_long;
    /// Library initialization, with default settings when given none.
    safe fn OPENSSL_init_ssl(options: u64, settings: Option<&InitSettings>) -> c_int;
    /// A new `ex_data` index of `class` (`SSL_get_ex_new_index` is this macro over the SSL
    /// class), or -1. OpenSSL keeps `argl` and `argp` only to hand them to the callbacks,
    /// each a safe fn, which any arguments may call.
    safe fn CRYPTO_get_ex_new_index(
        class: c_int,
        argl: c_long,
        argp: Option<&'static c_void>,
        new: Option<ExCallback>,
        dup: Option<ExDup>,
        free: Option<ExCallback>,
    ) -> c_int;
}

/// `ERR_print_errors_cb`'s callback: an error's line, its length and the user data.
type ErrorLine = extern "C" fn(CText<'_>, usize, Option<&'static c_void>) -> c_int;

/// `CRYPTO_EX_new` and `CRYPTO_EX_free`: `(parent, ptr, ad, idx, argl, argp)`.
type ExCallback = extern "C" fn(*mut c_void, *mut c_void, *mut c_void, c_int, c_long, *mut c_void);
/// `CRYPTO_EX_dup`: `(to, from, from_d, idx, argl, argp)`.
type ExDup =
    extern "C" fn(*mut c_void, *const c_void, *mut c_void, c_int, c_long, *mut c_void) -> c_int;

/// An `OPENSSL_INIT_SETTINGS`, which this side never makes.
#[repr(C)]
struct InitSettings {
    _opaque: [u8; 0],
}

/// An `SSL_METHOD`, one of libssl's static tables.
#[repr(C)]
struct SslMethod {
    _opaque: [u8; 0],
}

/// An `EVP_MD`, one of libcrypto's static method tables.
#[repr(C)]
struct EvpMd {
    _opaque: [u8; 0],
}

/// The DTLS cookie's HMAC key length: the whole 32-byte key.
#[derive(Clone, Copy)]
#[repr(i32)]
enum KeyLength {
    Secret = 32,
}

/// The DTLS cookie's HMAC data length: a port and an IPv4 or IPv6 address, at most
/// the 18-byte array's.
#[derive(Clone, Copy)]
#[repr(usize)]
enum DataLength {
    V4 = 6,
    V6 = 18,
}

/// A `STACK_OF(X509_NAME)` this side owns, as `SSL_load_client_CA_file` returns it,
/// until a context takes it.
#[repr(transparent)]
struct ClientCaNames(ptr::NonNull<c_void>);

/// DH parameters this side owns, as `PEM_read_bio_Parameters` returns them, until a
/// context takes them or `EVP_PKEY_free` frees them.
#[repr(transparent)]
struct DhParams(ptr::NonNull<EVP_PKEY>);

/// `SSL_CTX_set0_tmp_dh_pkey`: the context takes `key`'s reference, or refuses it
/// (false) and `key` is freed.
fn set_dh_params(ctx: &SslCtx, key: DhParams) -> bool {
    /// The reference the context takes on success: only this function makes one, from
    /// the parameters it was given, once.
    #[repr(transparent)]
    struct Given(ptr::NonNull<EVP_PKEY>);
    // libssl's declaration, which openssl-sys binds only over OpenSSL 3 headers (the
    // fuzz image builds crates/clients' parsers against OpenSSL 1.1): it keeps the
    // parameters on success (1) and leaves them the caller's otherwise.
    unsafe extern "C" {
        safe fn SSL_CTX_set0_tmp_dh_pkey(ctx: &SslCtx, key: Given) -> c_int;
    }
    if SSL_CTX_set0_tmp_dh_pkey(ctx, Given(key.0)) == 1 {
        return true;
    }
    EVP_PKEY_free(key);
    false
}

/// An `EVP_PKEY *` for `PEM_read_bio_Parameters` to reuse, which this side never makes.
#[repr(C)]
struct KeySlot {
    _opaque: [u8; 0],
}

/// The session id context of an HTTP server's sessions: its first four bytes.
static SESSION_ID_CONTEXT: [u8; 32] =
    *b"HTTP\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";

/// A live `SSL_CTX`, borrowed from an [`SslContext`]. It is zero-sized, so a
/// reference to it claims none of the bytes OpenSSL changes behind it.
#[repr(C)]
pub struct SslCtx {
    _opaque: [u8; 0],
    _not_send: PhantomData<(*mut u8, PhantomPinned)>,
}

impl SslCtx {
    #[must_use]
    pub fn as_ptr(&self) -> *mut SSL_CTX {
        ptr::from_ref(self).cast_mut().cast()
    }

    /// `SSL_CTX_set_tlsext_servername_callback`: `callback` chooses among contexts
    /// by the name a client sends.
    pub fn set_servername_callback(&self, callback: ServernameCallback) {
        SSL_CTX_callback_ctrl(self, CallbackCtrl::ServernameCallback, Some(callback));
    }

    /// `SSL_CTX_ctrl` for a command that reads only its integer.
    fn ctrl(&self, command: Ctrl, value: c_long) -> c_long {
        SSL_CTX_ctrl(self, command, value, c"".into())
    }
}

/// The `SSL_CTX_ctrl` commands behind `SSL_CTX_set_mode`, `_set_read_ahead`,
/// `_set_session_cache_mode` and `SSL_CTX_set1_curves_list`. Only the last reads its
/// pointer, as a NUL-terminated list.
#[derive(Clone, Copy)]
#[repr(i32)]
enum Ctrl {
    Mode = SSL_CTRL_MODE,
    ReadAhead = SSL_CTRL_SET_READ_AHEAD,
    SessionCacheMode = SSL_CTRL_SET_SESS_CACHE_MODE,
    GroupsList = SSL_CTRL_SET_GROUPS_LIST,
}

/// OpenSSL's servername callback: the handshake's live SSL, its alert slot and the
/// context's servername argument (null unless set).
pub type ServernameCallback = extern "C" fn(Ssl<'_>, *mut c_int, *mut c_void) -> c_int;

/// The `SSL_CTX_callback_ctrl` command behind `SSL_CTX_set_tlsext_servername_callback`.
#[derive(Clone, Copy)]
#[repr(i32)]
enum CallbackCtrl {
    ServernameCallback = SSL_CTRL_SET_TLSEXT_SERVERNAME_CB,
}

/// One reference to an `SSL_CTX`, released on drop. A clone takes another.
#[repr(transparent)]
pub struct SslContext(&'static SslCtx);

impl SslContext {
    /// Another reference, None when OpenSSL fails to take one: `clone` without the
    /// panic.
    #[must_use]
    pub fn try_clone(&self) -> Option<Self> {
        (SSL_CTX_up_ref(self) == 1).then(|| Self(self.0))
    }

    /// A fresh `TLS_method` context, for tests.
    #[cfg(test)]
    pub(crate) fn tls() -> Self {
        SSL_CTX_new(TLS_method()).unwrap()
    }
}

impl Deref for SslContext {
    type Target = SslCtx;

    /// The context, live while this reference is.
    fn deref(&self) -> &SslCtx {
        self.0
    }
}

impl Clone for SslContext {
    fn clone(&self) -> Self {
        // OpenSSL 3 fails only when its lock does; 1.1 never does.
        assert_eq!(SSL_CTX_up_ref(self), 1, "SSL_CTX_up_ref failed");
        Self(self.0)
    }
}

impl Drop for SslContext {
    fn drop(&mut self) {
        /// The reference a dropped `SslContext` held: only this drop makes one, once per
        /// handle, after every borrow of the context through it has ended, so
        /// `SSL_CTX_free` releases each reference once.
        #[repr(transparent)]
        struct Released<'a>(&'a SslCtx);
        unsafe extern "C" {
            safe fn SSL_CTX_free(ctx: Released<'_>);
        }
        SSL_CTX_free(Released(self.0));
    }
}

/// `swSSLVersion`'s protocol bits.
const SSLV2: u32 = crate::constants::SslVersion::SW_SSL_SSLv2 as u32;
const SSLV3: u32 = crate::constants::SslVersion::SW_SSL_SSLv3 as u32;
const TLSV1: u32 = crate::constants::SslVersion::SW_SSL_TLSv1 as u32;
const TLSV1_1: u32 = crate::constants::SslVersion::SW_SSL_TLSv1_1 as u32;
const TLSV1_2: u32 = crate::constants::SslVersion::SW_SSL_TLSv1_2 as u32;
const TLSV1_3: u32 = crate::constants::SslVersion::SW_SSL_TLSv1_3 as u32;
const DTLS: u32 = crate::constants::SslVersion::SW_SSL_DTLS as u32;
const ALL: u32 = crate::constants::SW_SSL_ALL.unsigned_abs();

/// `OPENSSL_INIT_LOAD_CONFIG | OPENSSL_INIT_LOAD_SSL_STRINGS | OPENSSL_INIT_LOAD_CRYPTO_STRINGS`.
const INIT_OPTIONS: u64 = 0x40 | 0x0020_0000 | 0x2;
/// The `BIO_ctrl` command behind `BIO_dgram_get_peer`.
#[derive(Clone, Copy)]
#[repr(i32)]
enum BioPeerCtrl {
    GetPeer = 46,
}

/// Where `BIO_dgram_get_peer` copies a peer's address: room for any sockaddr.
type PeerBytes = [u8; size_of::<libc::sockaddr_storage>()];
// The session BIO's own answer (dtls.rs's `bio_ctrl`) is the native address union.
const _: () = assert!(
    size_of::<swoole_rs_reactor::AddressStorage>() <= size_of::<PeerBytes>(),
    "the session BIO's peer address fits where BIO_dgram_get_peer copies it"
);

// libcrypto's `BIO_ctrl`, declared for its one command here. A handle's BIO is one
// this crate gave an SSL or made itself: a datagram BIO copies its peer's sockaddr,
// the session BIO its address union, neither longer than `peer`, and the socket,
// memory and file BIOs write nothing. OpenSSL answers a null BIO itself.
unsafe extern "C" {
    #[link_name = "BIO_ctrl"]
    safe fn bio_peer(
        bio: Option<Bio<'_>>,
        command: BioPeerCtrl,
        size: c_long,
        peer: &mut PeerBytes,
    ) -> c_long;
}

/// A native `std::string`, borrowed: its `c_str()` (none when unset) and `length()`.
#[derive(Clone, Copy, Debug)]
pub struct TlsStr<'a> {
    text: Option<&'a CStr>,
    length: usize,
}

impl<'a> From<&'a StringSlot> for TlsStr<'a> {
    fn from(slot: &'a StringSlot) -> Self {
        Self {
            text: Some(slot.c_str()),
            length: slot.bytes().len(),
        }
    }
}

impl<'a> TlsStr<'a> {
    /// An unset string.
    const NONE: Self = Self {
        text: None,
        length: 0,
    };

    /// `empty()`.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.length == 0
    }

    /// The `c_str()`, None when unset.
    const fn c_str(&self) -> Option<&'a CStr> {
        self.text
    }

    /// What `%s` prints: the bytes before the first NUL.
    fn text(self) -> Vec<u8> {
        self.c_str()
            .map_or_else(Vec::new, |text| text.to_bytes().to_vec())
    }

    /// The `c_str()`, or None when empty (upstream passes null for an unset path).
    fn path(&self) -> Option<CText<'a>> {
        if self.is_empty() {
            None
        } else {
            self.c_str().map(CText::from)
        }
    }
}

/// Logs `"<function>(): <message>"` at warning level.
pub type Warn = fn(function: &CStr, message: &str);

/// A view of `swoole::SSLContext`'s configuration, built for one call by `owned.rs`'s
/// `configuration`.
#[repr(C)]
#[derive(Debug)]
pub struct TlsConfig<'a> {
    pub ciphers: TlsStr<'a>,
    pub ecdh_curve: TlsStr<'a>,
    pub dhparam: TlsStr<'a>,
    pub cert_file: TlsStr<'a>,
    pub key_file: TlsStr<'a>,
    pub passphrase: TlsStr<'a>,
    pub client_cert_file: TlsStr<'a>,
    pub cafile: TlsStr<'a>,
    pub capath: TlsStr<'a>,
    /// In: the configured mask. Out (create): zero becomes every TLS version.
    pub protocols: u32,
    pub verify_depth: u8,
    pub http: bool,
    pub http_v2: bool,
    pub prefer_server_ciphers: bool,
    pub disable_compress: bool,
    pub verify_peer: bool,
    pub warn: Warn,
}

/// OpenSSL's `pem_password_cb`: the buffer and its capacity, whether to verify, and
/// the userdata it was installed with.
pub(crate) type PasswordCallback =
    unsafe extern "C" fn(*mut c_char, c_int, c_int, &StringSlot) -> c_int;

/// One upstream method: its log name and the configuration it reads.
#[derive(Clone, Copy)]
pub(crate) struct PasswordBinding {
    /// What OpenSSL hands the callback: a slot that outlives the context.
    pub userdata: *const StringSlot,
    pub callback: PasswordCallback,
}

struct Method<'a> {
    config: &'a TlsConfig<'a>,
    ctx: &'a SslCtx,
    function: &'static CStr,
    password: Option<PasswordBinding>,
}

/// `SSL_OP_*`'s type: `uint64_t` since OpenSSL 3.
#[cfg(ossl300)]
type Options = u64;
#[cfg(not(ossl300))]
type Options = std::ffi::c_ulong;

fn warn(config: &TlsConfig<'_>, function: &CStr, message: &[u8]) {
    let text: Vec<u8> = message.iter().copied().filter(|&b| b != 0).collect();
    (config.warn)(function, &String::from_utf8_lossy(&text));
}

/// Upstream's `ssl_error`: pops one error and appends its reason and code.
fn ssl_error(config: &TlsConfig<'_>, function: &CStr, message: &[u8]) {
    let code = crate::pop_error();
    let reason = crate::reason_string(code).map_or(b"(null)".as_slice(), CStr::to_bytes);
    let code = code.c_cast::<c_long>();
    let mut text = message.to_vec();
    text.extend_from_slice(b", Error: ");
    text.extend_from_slice(reason);
    text.extend_from_slice(format!("[{code}]").as_bytes());
    warn(config, function, &text);
}

impl Method<'_> {
    fn warn(&self, message: &[u8]) {
        warn(self.config, self.function, message);
    }

    fn ssl_error(&self, message: &[u8]) {
        ssl_error(self.config, self.function, message);
    }

    /// `SSL_CTX_load_verify_locations`, an empty file or path given as null.
    fn load_verify_locations(&self, file: TlsStr<'_>, path: TlsStr<'_>) -> c_int {
        SSL_CTX_load_verify_locations(self.ctx, file.path(), path.path())
    }

    fn set_capath(&self) -> bool {
        let config = self.config;
        if !config.cafile.is_empty() || !config.capath.is_empty() {
            if self.load_verify_locations(config.cafile, config.capath) == 0 {
                return false;
            }
        } else if SSL_CTX_set_default_verify_paths(self.ctx) == 0 {
            self.ssl_error(b"SSL_CTX_set_default_verify_paths() failed");
            return false;
        }
        if config.verify_depth > 0 {
            SSL_CTX_set_verify_depth(self.ctx, c_int::from(config.verify_depth));
        }
        true
    }

    fn set_ciphers(&self) -> bool {
        let config = self.config;
        if let Some(ciphers) = config.ciphers.path() {
            if SSL_CTX_set_cipher_list(self.ctx, ciphers) == 0 {
                self.ssl_error(&quoted(
                    b"SSL_CTX_set_cipher_list(\"",
                    config.ciphers,
                    b"\") failed",
                ));
                return false;
            }
            if config.prefer_server_ciphers
                && SSL_CTX_set_options(self.ctx, SSL_OP_CIPHER_SERVER_PREFERENCE) == 0
            {
                self.ssl_error(b"SSL_CTX_set_options(SSL_OP_CIPHER_SERVER_PREFERENCE) failed");
                return false;
            }
        }
        if !config.dhparam.is_empty() && !self.with(c"SSLContext::set_dhparam").set_dhparam() {
            return false;
        }
        !(!config.ecdh_curve.is_empty()
            && !self.with(c"SSLContext::set_ecdh_curve").set_ecdh_curve())
    }

    fn set_client_certificate(&self) -> bool {
        let file = self.config.client_cert_file;
        SSL_CTX_set_verify(self.ctx, SSL_VERIFY_PEER, Some(verify_callback));
        SSL_CTX_set_verify_depth(self.ctx, c_int::from(self.config.verify_depth));
        // `configure` calls this for a file that is set, and with no directory.
        if self.load_verify_locations(file, TlsStr::NONE) == 0 {
            self.ssl_error(&quoted(
                b"SSL_CTX_load_verify_locations(\"",
                file,
                b"\") failed",
            ));
            return false;
        }
        crate::clear_errors();
        let names = file
            .c_str()
            .and_then(|file| SSL_load_client_CA_file(file.into()));
        let Some(names) = names else {
            self.ssl_error(&quoted(b"SSL_load_client_CA_file(\"", file, b"\") failed"));
            return false;
        };
        crate::clear_errors();
        SSL_CTX_set_client_CA_list(self.ctx, names);
        true
    }

    fn set_ecdh_curve(&self) -> bool {
        let curve = self.config.ecdh_curve;
        // SSL_OP_SINGLE_ECDH_USE is 0 and ECDH is automatic in OpenSSL 3.
        if curve.text() == b"auto" {
            return true;
        }
        // `SSL_CTX_set1_curves_list`.
        let set = curve.c_str().map_or(0, |list| {
            SSL_CTX_ctrl(self.ctx, Ctrl::GroupsList, 0, list.into())
        });
        if set == 0 {
            self.warn(&quoted(
                b"SSL_CTX_set1_curves_list(\"",
                curve,
                b"\") failed",
            ));
            return false;
        }
        true
    }

    fn set_dhparam(&self) -> bool {
        let file = self.config.dhparam;
        // Freed on each return below, after its error is reported.
        let Some(bio) = file.c_str().and_then(crate::OwnedBio::open) else {
            self.ssl_error(&quoted(b"BIO_new_file(", file, b") failed"));
            return false;
        };
        let Some(key) = PEM_read_bio_Parameters(bio.bio(), None) else {
            self.ssl_error(&quoted(b"PEM_read_bio_Parameters('", file, b"') failed"));
            return false;
        };
        if !set_dh_params(self.ctx, key) {
            self.ssl_error(&quoted(b"SSL_CTX_set0_tmp_dh_pkey('", file, b"') failed"));
            return false;
        }
        true
    }

    const fn with(&self, function: &'static CStr) -> Self {
        Method {
            config: self.config,
            ctx: self.ctx,
            function,
            password: self.password,
        }
    }

    /// `SSLContext::create` after `SSL_CTX_new`; false when the context must be freed.
    fn configure(&self) -> bool {
        let config = self.config;
        let ctx = self.ctx;
        let raw = ctx.as_ptr();
        let protocols = config.protocols;
        // The other legacy SSL_OP_*_BUG workarounds are 0 in OpenSSL 3.
        SSL_CTX_set_options(ctx, SSL_OP_DONT_INSERT_EMPTY_FRAGMENTS);
        SSL_CTX_clear_options(ctx, SSL_OP_NO_SSLv2 | SSL_OP_NO_SSLv3 | SSL_OP_NO_TLSv1);
        let mut disable = 0;
        for (bit, option) in [
            (SSLV2, SSL_OP_NO_SSLv2),
            (SSLV3, SSL_OP_NO_SSLv3),
            (TLSV1, SSL_OP_NO_TLSv1),
            (TLSV1_1, SSL_OP_NO_TLSv1_1),
            (TLSV1_2, SSL_OP_NO_TLSv1_2),
            (TLSV1_3, SSL_OP_NO_TLSv1_3),
        ] {
            // DTLS keeps TLS 1.2 (DTLS 1.2 shares its option bit).
            if protocols & bit == 0 && !(bit == TLSV1_2 && protocols & DTLS != 0) {
                disable |= option;
            }
        }
        SSL_CTX_clear_options(
            ctx,
            SSL_OP_NO_TLSv1_1 | SSL_OP_NO_TLSv1_2 | SSL_OP_NO_TLSv1_3,
        );
        SSL_CTX_set_options(ctx, disable);
        if config.disable_compress {
            SSL_CTX_set_options(ctx, SSL_OP_NO_COMPRESSION);
        }
        ctx.ctrl(
            Ctrl::Mode,
            SSL_MODE_RELEASE_BUFFERS | SSL_MODE_NO_AUTO_CHAIN,
        );
        ctx.ctrl(Ctrl::ReadAhead, 1);

        // The owner's binding reads its live passphrase slot (crates/tls's owned.rs).
        if !config.passphrase.is_empty()
            && let Some(binding) = self.password
        {
            let userdata = binding.userdata.cast_mut().cast();
            // SAFETY: a live context; the binding's owner outlives the borrowed context.
            unsafe { SSL_CTX_set_default_passwd_cb_userdata(raw, userdata) };
            // SAFETY: a live context; OpenSSL calls the callback with the buffer it
            // lends and the userdata set above, the binding's slot.
            unsafe { SSL_CTX_set_default_passwd_cb(raw, binding.callback) };
        }
        if let Some(path) = config.cert_file.path() {
            let file = config.cert_file;
            if SSL_CTX_use_certificate_file(ctx, path, SSL_FILETYPE_PEM) <= 0 {
                self.ssl_error(&quoted(b"SSL_CTX_use_certificate_file(", file, b") failed"));
                return false;
            }
            if SSL_CTX_use_certificate_chain_file(ctx, path) <= 0 {
                self.ssl_error(&quoted(
                    b"SSL_CTX_use_certificate_chain_file(",
                    file,
                    b") failed",
                ));
                return false;
            }
        }
        if let Some(path) = config.key_file.path() {
            let file = config.key_file;
            if SSL_CTX_use_PrivateKey_file(ctx, path, SSL_FILETYPE_PEM) <= 0 {
                self.ssl_error(&quoted(b"SSL_CTX_use_PrivateKey_file(", file, b") failed"));
                return false;
            }
            if SSL_CTX_check_private_key(ctx) == 0 {
                self.ssl_error(b"SSL_CTX_check_private_key() failed");
                return false;
            }
        }
        if protocols & DTLS != 0 {
            set_cookie_generate_cb(ctx, Some(generate_cookie));
            // SAFETY: OpenSSL calls the verifier with the cookie's bytes and their count.
            unsafe { set_cookie_verify_cb(ctx, Some(verify_cookie)) };
        }
        let capath = self.with(c"SSLContext::set_capath").set_capath();
        if config.verify_peer {
            if !capath {
                return false;
            }
        } else {
            SSL_CTX_set_verify(ctx, SSL_VERIFY_NONE, None);
        }
        if config.http || config.http_v2 {
            let protos: &'static Protos = if config.http_v2 { &H2_HTTP1 } else { &HTTP1 };
            if SSL_CTX_set_alpn_protos(ctx, &protos.wire, protos.length) < 0 {
                self.ssl_error(
                    &[b"SSL_CTX_set_alpn_protos(", protos.bytes(), b") failed"].concat(),
                );
                return false;
            }
            // SAFETY: a live context; OpenSSL calls the selector with the client's offered
            // list and its length, and the static list given here.
            unsafe { set_alpn_select_cb(ctx, Some(select_alpn), protos) };
            SSL_CTX_set_session_id_context(ctx, &SESSION_ID_CONTEXT, 4);
            ctx.ctrl(Ctrl::SessionCacheMode, SSL_SESS_CACHE_SERVER);
        }
        if !config.client_cert_file.is_empty()
            && !self
                .with(c"SSLContext::set_client_certificate")
                .set_client_certificate()
        {
            return false;
        }
        if !self.with(c"SSLContext::set_ciphers").set_ciphers() {
            self.warn(b"set_cipher() error");
            return false;
        }
        true
    }
}

fn quoted(prefix: &[u8], value: TlsStr<'_>, suffix: &[u8]) -> Vec<u8> {
    [prefix, &value.text(), suffix].concat()
}

/// An ALPN wire-format list: the first `length` bytes of `wire`. OpenSSL only hands
/// one back to the selector, as its argument.
#[repr(C)]
struct Protos {
    wire: [u8; 12],
    length: AlpnLength,
}
static H2_HTTP1: Protos = Protos {
    wire: *b"\x02h2\x08http/1.1",
    length: AlpnLength::H2Http1,
};
static HTTP1: Protos = Protos {
    wire: *b"\x08http/1.1\0\0\0",
    length: AlpnLength::Http1,
};

impl Protos {
    /// The list's bytes.
    fn bytes(&self) -> &[u8] {
        self.wire.get(..self.length as usize).unwrap_or(&self.wire)
    }
}

/// The length of an ALPN list in a 12-byte [`Protos::wire`]: each value is at most the
/// array's, so OpenSSL reads only its bytes.
#[derive(Clone, Copy)]
#[repr(u32)]
enum AlpnLength {
    Http1 = 9,
    H2Http1 = 12,
}

// libssl's declaration, typed so that the list is the array its length bounds; it
// copies the list into the live context: 0 on success.
unsafe extern "C" {
    safe fn SSL_CTX_set_alpn_protos(ctx: &SslCtx, protos: &[u8; 12], length: AlpnLength) -> c_int;
}

/// OpenSSL's ALPN selector, with its argument typed as the static list `configure`
/// installs it with.
type SelectAlpn = unsafe extern "C" fn(
    *mut SSL,
    *mut *const c_uchar,
    *mut c_uchar,
    *const c_uchar,
    c_uint,
    &'static Protos,
) -> c_int;

// libssl's declaration, with the argument typed as what `configure` passes. A
// selector reads the raw list it is lent, so storing one is not `safe`.
unsafe extern "C" {
    #[link_name = "SSL_CTX_set_alpn_select_cb"]
    fn set_alpn_select_cb(ctx: &SslCtx, callback: Option<SelectAlpn>, arg: &'static Protos);
}

/// The server's ALPN choice: the first of its protocols the client offers.
///
/// # Safety
/// `out` and `out_length` are writable, and `offered` holds `offered_length` bytes.
unsafe extern "C" fn select_alpn(
    _ssl: *mut SSL,
    out: *mut *const c_uchar,
    out_length: *mut c_uchar,
    offered: *const c_uchar,
    offered_length: c_uint,
    protos: &'static Protos,
) -> c_int {
    let length = protos.length as c_uint;
    // SAFETY: OpenSSL's output slots and offered list; ours is static.
    let negotiated = unsafe {
        SSL_select_next_proto(
            out.cast(),
            out_length,
            protos.wire.as_ptr(),
            length,
            offered,
            offered_length,
        )
    };
    if negotiated == OPENSSL_NPN_NEGOTIATED {
        SSL_TLSEXT_ERR_OK
    } else {
        SSL_TLSEXT_ERR_NOACK
    }
}

/// `SSL_CTX_set_verify`'s callback.
type VerifyCallback = extern "C" fn(c_int, *mut X509_STORE_CTX) -> c_int;

/// Peer certificates are checked later by the callers; the chain walk always continues.
extern "C" fn verify_callback(_ok: c_int, _store: *mut X509_STORE_CTX) -> c_int {
    1
}

/// Copies the passphrase and the NUL after it (`terminated` is both) into `buffer`
/// when it fits with a spare byte, else reports none.
pub(crate) fn copy_password(buffer: &mut [MaybeUninit<u8>], terminated: &[u8]) -> c_int {
    let Ok(length) = c_int::try_from(terminated.len().saturating_sub(1)) else {
        return 0;
    };
    if length == 0 || terminated.len() >= buffer.len() {
        return 0;
    }
    for (slot, &byte) in buffer.iter_mut().zip(terminated) {
        slot.write(byte);
    }
    length
}

/// The buffer OpenSSL lends a cookie generator: `DTLS1_COOKIE_LENGTH` bytes.
type CookieBuffer = [MaybeUninit<u8>; 255];
type GenerateCookie = extern "C" fn(Ssl<'_>, &mut CookieBuffer, &mut MaybeUninit<c_uint>) -> c_int;
type VerifyCookie = unsafe extern "C" fn(Ssl<'_>, *const c_uchar, c_uint) -> c_int;

// libssl's declarations, with the callbacks retyped as what OpenSSL calls
// them with: the live, non-null SSL that is listening or shaking hands and, for the
// generator, its `DTLS1_COOKIE_LENGTH`-byte cookie buffer and length slot (neither
// initialized), lent for the call. Storing a generator, which is a safe function of exactly those, is
// `safe`; a verifier reads the raw cookie it is lent, so storing one is not.
unsafe extern "C" {
    #[link_name = "SSL_CTX_set_cookie_generate_cb"]
    safe fn set_cookie_generate_cb(ctx: &SslCtx, callback: Option<GenerateCookie>);
    #[link_name = "SSL_CTX_set_cookie_verify_cb"]
    fn set_cookie_verify_cb(ctx: &SslCtx, callback: Option<VerifyCookie>);
}

/// Copies `N` bytes of `bytes` at `at`.
fn field<const N: usize>(bytes: &[u8], at: usize) -> [u8; N] {
    bytes
        .get(at..at + N)
        .and_then(|field| field.try_into().ok())
        .unwrap_or([0; N])
}

/// DTLS cookie: HMAC-SHA1 of the peer's port and address, keyed by the SSL's address,
/// into `cookie` with its length in `cookie_length`.
fn cookie_of(ssl: Ssl<'_>, cookie: &mut [u8; 255], cookie_length: &mut c_uint) -> c_int {
    let mut secret = [0; 32];
    let address_bytes = (ssl.as_ptr() as c_long).to_ne_bytes();
    for chunk in secret.as_chunks_mut::<{ size_of::<c_long>() }>().0 {
        chunk.copy_from_slice(&address_bytes);
    }
    let mut peer: PeerBytes = [0; size_of::<PeerBytes>()];
    // `BIO_dgram_get_peer` on the SSL's write BIO.
    bio_peer(ssl.write_bio(), BioPeerCtrl::GetPeer, 0, &mut peer);
    let family = libc::sa_family_t::from_ne_bytes(field(
        &peer,
        std::mem::offset_of!(libc::sockaddr_storage, ss_family),
    ));
    // The port, then the address.
    let mut data = [0; 18];
    let (port, address, length) = match c_int::from(family) {
        libc::AF_INET => (
            field::<2>(&peer, std::mem::offset_of!(libc::sockaddr_in, sin_port)),
            &field::<4>(&peer, std::mem::offset_of!(libc::sockaddr_in, sin_addr))[..],
            DataLength::V4,
        ),
        libc::AF_INET6 => (
            field::<2>(&peer, std::mem::offset_of!(libc::sockaddr_in6, sin6_port)),
            &field::<16>(&peer, std::mem::offset_of!(libc::sockaddr_in6, sin6_addr))[..],
            DataLength::V6,
        ),
        // Upstream's OPENSSL_assert(0).
        _ => swoole_rs_corebase::fatal("a peer address that is neither IPv4 nor IPv6"),
    };
    for (slot, &byte) in data.iter_mut().zip(port.iter().chain(address)) {
        *slot = byte;
    }
    let Some(sha1) = EVP_sha1() else {
        return 0;
    };
    let digest = HMAC(
        sha1,
        &secret,
        KeyLength::Secret,
        &data,
        length,
        cookie,
        cookie_length,
    );
    c_int::from(!digest.is_null())
}

/// OpenSSL's cookie generator: [`cookie_of`] into the buffer it lends, the length
/// set when there is a cookie.
extern "C" fn generate_cookie(
    ssl: Ssl<'_>,
    cookie: &mut CookieBuffer,
    cookie_length: &mut MaybeUninit<c_uint>,
) -> c_int {
    let mut made_cookie = [0; 255];
    let mut length = 0;
    let made = cookie_of(ssl, &mut made_cookie, &mut length);
    if made != 0 {
        let digest = made_cookie.get(..length as usize).unwrap_or_default();
        for (slot, &byte) in cookie.iter_mut().zip(digest) {
            slot.write(byte);
        }
        cookie_length.write(length);
    }
    made
}

/// # Safety
/// OpenSSL's cookie callback: `ssl` is live and `cookie` holds `cookie_length` bytes.
unsafe extern "C" fn verify_cookie(
    ssl: Ssl<'_>,
    cookie: *const c_uchar,
    cookie_length: c_uint,
) -> c_int {
    let mut expected = [0u8; 255];
    let mut length: c_uint = 0;
    cookie_of(ssl, &mut expected, &mut length);
    if cookie_length != length {
        return 0;
    }
    // SAFETY: OpenSSL lends `cookie_length` bytes.
    let cookie = unsafe { std::slice::from_raw_parts(cookie, cookie_length as usize) };
    c_int::from(expected.get(..length as usize) == Some(cookie))
}

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static CONNECTION_INDEX: AtomicI32 = AtomicI32::new(0);
static PORT_INDEX: AtomicI32 = AtomicI32::new(0);

/// `SSL_get_ex_new_index(0, NULL, NULL, NULL, NULL)`.
fn new_ssl_index() -> c_int {
    CRYPTO_get_ex_new_index(openssl_sys::CRYPTO_EX_INDEX_SSL, 0, None, None, None, None)
}

/// `void swoole_ssl_init()`: once until `swoole_ssl_destroy`. A failure logs at
/// error level and exits the process, as upstream's `swoole_error` does.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z15swoole_ssl_initv")
)]
pub extern "C" fn swoole_ssl_init() {
    let failed = match swr_v1_tls_init() {
        Ok(()) => return,
        Err(TlsInitError::Init) => "OPENSSL_init_ssl",
        Err(TlsInitError::Index) => "SSL_get_ex_new_index",
    };
    log::error(c"swoole_ssl_init", format_args!("{failed}() failed"));
}

/// The step of TLS initialization that failed.
#[derive(Debug, PartialEq, Eq)]
pub enum TlsInitError {
    /// `OPENSSL_init_ssl`.
    Init,
    /// An `SSL_get_ex_new_index`.
    Index,
}

/// Initializes OpenSSL and the `ex_data` indexes; done, or already done.
/// # Errors
/// The step that failed.
pub fn swr_v1_tls_init() -> Result<(), TlsInitError> {
    if INITIALIZED.load(Ordering::Acquire) {
        return Ok(());
    }
    if OPENSSL_init_ssl(INIT_OPTIONS, None) == 0 {
        return Err(TlsInitError::Init);
    }
    let connection = new_ssl_index();
    if connection < 0 {
        return Err(TlsInitError::Index);
    }
    CONNECTION_INDEX.store(connection, Ordering::Release);
    let port = new_ssl_index();
    if port < 0 {
        return Err(TlsInitError::Index);
    }
    PORT_INDEX.store(port, Ordering::Release);
    INITIALIZED.store(true, Ordering::Release);
    Ok(())
}

/// `void swoole_ssl_destroy()`: the next init allocates fresh indexes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z18swoole_ssl_destroyv")
)]
pub extern "C" fn swr_v1_tls_destroy() {
    INITIALIZED.store(false, Ordering::Release);
}

/// The SSL `ex_data` index holding the connection's `Socket`.
pub extern "C" fn swr_v1_tls_connection_index() -> c_int {
    CONNECTION_INDEX.load(Ordering::Acquire)
}

/// The SSL `ex_data` index holding the accepting `ListenPort`.
pub extern "C" fn swr_v1_tls_port_index() -> c_int {
    PORT_INDEX.load(Ordering::Acquire)
}

thread_local! {
    static ERROR_TEXT: RefCell<Vec<u8>> = RefCell::new(vec![0]);
}

/// `ERR_print_errors_cb`'s callback: keeps `line` as the thread's error line. It returns
/// 0, as upstream's `ssl_error_cb` does, so OpenSSL stops after this one error.
extern "C" fn keep_last_line(
    line: CText<'_>,
    _length: usize,
    _user: Option<&'static c_void>,
) -> c_int {
    let line = line.get().to_bytes();
    ERROR_TEXT.with_borrow_mut(|text| {
        text.clear();
        text.extend_from_slice(line);
        text.push(0);
    });
    0
}

/// `swoole_ssl_get_error`: pops this thread's oldest queued error and returns its line
/// ("" when none is queued), valid until the thread's next call.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z20swoole_ssl_get_errorv")
)]
#[must_use]
pub extern "C" fn swr_v1_tls_error() -> *const c_char {
    drain_errors();
    ERROR_TEXT.with_borrow(|text| text.as_ptr().cast())
}

/// `swoole_ssl_get_error()`'s line, copied.
#[must_use]
pub fn error_line() -> String {
    drain_errors();
    ERROR_TEXT.with_borrow(|text| {
        String::from_utf8_lossy(text.split_last().map_or(&[], |(_, line)| line)).into_owned()
    })
}

/// Pops this thread's oldest queued error, keeping its line (none when the queue is empty).
fn drain_errors() {
    ERROR_TEXT.with_borrow_mut(|text| {
        text.clear();
        text.push(0);
    });
    ERR_print_errors_cb(keep_last_line, None);
}

/// # Safety
/// Any password binding remains live throughout the borrowed context.
pub(crate) unsafe fn create_context(
    config: &mut TlsConfig<'_>,
    password: Option<PasswordBinding>,
) -> Option<SslContext> {
    let method = if config.protocols & DTLS != 0 {
        DTLS_method()
    } else {
        TLS_method()
    };
    if config.protocols == 0 {
        config.protocols = ALL;
    }
    let function = c"SSLContext::create";
    let Some(ctx) = SSL_CTX_new(method) else {
        ssl_error(config, function, b"SSL_CTX_new() failed");
        return None;
    };
    let create = Method {
        config,
        ctx: &ctx,
        function,
        password,
    };
    create.configure().then_some(ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openssl_sys::ERR_clear_error;

    /// `SSLContext::create`: a configured context, or null after logging why. Writes
    /// the normalized protocol mask back to `config.protocols`.
    ///
    /// # Safety
    /// `config`'s strings are NUL-terminated and outlive the call.
    unsafe fn swr_v1_tls_context_create(config: &mut TlsConfig<'_>) -> Option<SslContext> {
        // SAFETY: forwards the caller's live configuration view.
        unsafe { create_context(config, None) }
    }

    macro_rules! method {
        ($name:ident, $function:literal, $body:ident) => {
            /// The `SSLContext` method of the same name, on an existing context.
            ///
            /// # Safety
            /// `ctx` is a live context and `config`'s strings are NUL-terminated and
            /// outlive the call.
            unsafe fn $name(ctx: &SslCtx, config: &TlsConfig<'_>) -> bool {
                Method {
                    config,
                    ctx,
                    function: $function,
                    password: None,
                }
                .$body()
            }
        };
    }

    method!(swr_v1_tls_set_capath, c"SSLContext::set_capath", set_capath);
    method!(
        swr_v1_tls_set_client_certificate,
        c"SSLContext::set_client_certificate",
        set_client_certificate
    );
    method!(
        swr_v1_tls_set_ecdh_curve,
        c"SSLContext::set_ecdh_curve",
        set_ecdh_curve
    );
    method!(
        swr_v1_tls_set_dhparam,
        c"SSLContext::set_dhparam",
        set_dhparam
    );

    use openssl_sys::{SSL_CTX_get_options, SSL_CTX_get_verify_mode};

    unsafe extern "C" {
        fn SSL_CTX_get_verify_depth(ctx: *const SSL_CTX) -> c_int;
    }
    use std::ffi::CString;
    use std::sync::Mutex;

    static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

    fn record(function: &CStr, message: &str) {
        LOG.lock()
            .unwrap()
            .push(format!("{}(): {message}", function.to_string_lossy()));
    }

    /// Configuration strings, leaked so that each view is `'static`.
    struct Owned;

    impl Owned {
        fn str(value: &str) -> TlsStr<'static> {
            let value: &'static CStr = Box::leak(CString::new(value).unwrap().into_boxed_c_str());
            TlsStr {
                text: Some(value),
                length: value.to_bytes().len(),
            }
        }

        fn config() -> TlsConfig<'static> {
            let empty = Self::str("");
            TlsConfig {
                ciphers: empty,
                ecdh_curve: empty,
                dhparam: empty,
                cert_file: empty,
                key_file: empty,
                passphrase: empty,
                client_cert_file: empty,
                cafile: empty,
                capath: empty,
                protocols: 0,
                verify_depth: 0,
                http: false,
                http_v2: false,
                prefer_server_ciphers: false,
                disable_compress: false,
                verify_peer: false,
                warn: record,
            }
        }
    }

    fn take_log() -> Vec<String> {
        std::mem::take(&mut *LOG.lock().unwrap())
    }

    #[test]
    fn protocols_and_options() {
        let _guard = SERIAL.lock().unwrap();
        let mut config = Owned::config();
        unsafe {
            let ctx = swr_v1_tls_context_create(&mut config).unwrap();
            assert_eq!(config.protocols, ALL, "zero means every version");
            let options = SSL_CTX_get_options(ctx.as_ptr());
            assert_eq!(options & (SSL_OP_NO_TLSv1_2 | SSL_OP_NO_TLSv1_3), 0);
            assert_eq!(SSL_CTX_get_verify_mode(ctx.as_ptr()), SSL_VERIFY_NONE);
            drop(ctx);

            config.protocols = TLSV1_3 | DTLS;
            config.disable_compress = true;
            let ctx = swr_v1_tls_context_create(&mut config).unwrap();
            let options = SSL_CTX_get_options(ctx.as_ptr());
            assert_ne!(options & SSL_OP_NO_TLSv1_1, 0);
            assert_eq!(options & SSL_OP_NO_TLSv1_2, 0, "DTLS keeps 1.2");
            assert_eq!(options & SSL_OP_NO_TLSv1_3, 0);
            assert_ne!(options & SSL_OP_NO_COMPRESSION, 0);
        }
        assert_eq!(take_log(), Vec::<String>::new());
    }

    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn failures_log_like_upstream() {
        let _guard = SERIAL.lock().unwrap();
        take_log();
        let mut config = Owned::config();
        config.cert_file = Owned::str("/nonexistent/m21.pem");
        unsafe { ERR_clear_error() };
        let ctx = unsafe { swr_v1_tls_context_create(&mut config) };
        assert!(ctx.is_none());
        let log = take_log();
        assert_eq!(log.len(), 1);
        assert!(
            log[0].starts_with(
                "SSLContext::create(): SSL_CTX_use_certificate_file(/nonexistent/m21.pem) failed, Error: "
            ),
            "{log:?}"
        );

        let mut config = Owned::config();
        config.ciphers = Owned::str("M21-nonexistent-cipher");
        let ctx = unsafe { swr_v1_tls_context_create(&mut config) };
        assert!(ctx.is_none());
        let log = take_log();
        assert_eq!(log.len(), 2, "{log:?}");
        assert!(log[0].starts_with(
            "SSLContext::set_ciphers(): SSL_CTX_set_cipher_list(\"M21-nonexistent-cipher\") failed, Error: "
        ));
        assert_eq!(log[1], "SSLContext::create(): set_cipher() error");
    }

    #[test]
    fn direct_methods() {
        let _guard = SERIAL.lock().unwrap();
        take_log();
        let mut config = Owned::config();
        unsafe {
            let context = SSL_CTX_new(TLS_method()).unwrap();
            let ctx = &*context;
            config.ecdh_curve = Owned::str("auto");
            assert!(swr_v1_tls_set_ecdh_curve(ctx, &config));
            config.ecdh_curve = Owned::str("prime256v1");
            assert!(swr_v1_tls_set_ecdh_curve(ctx, &config));
            config.ecdh_curve = Owned::str("M21-nonexistent-curve");
            assert!(!swr_v1_tls_set_ecdh_curve(ctx, &config));
            assert_eq!(
                take_log(),
                [
                    "SSLContext::set_ecdh_curve(): SSL_CTX_set1_curves_list(\"M21-nonexistent-curve\") failed"
                ]
            );

            config.dhparam = Owned::str("/nonexistent/m21-dh.pem");
            assert!(!swr_v1_tls_set_dhparam(ctx, &config));
            let log = take_log();
            assert!(log[0].starts_with(
                "SSLContext::set_dhparam(): BIO_new_file(/nonexistent/m21-dh.pem) failed"
            ));

            config.client_cert_file = Owned::str("/nonexistent/m21-ca.pem");
            config.verify_depth = 3;
            assert!(!swr_v1_tls_set_client_certificate(ctx, &config));
            assert_eq!(SSL_CTX_get_verify_mode(ctx.as_ptr()), SSL_VERIFY_PEER);
            assert_eq!(SSL_CTX_get_verify_depth(ctx.as_ptr()), 3);
            take_log();

            config.cafile = Owned::str("/nonexistent/m21-ca.pem");
            assert!(!swr_v1_tls_set_capath(ctx, &config));
            assert_eq!(
                take_log(),
                Vec::<String>::new(),
                "a failed CA load is silent"
            );
        }
        assert_eq!(verify_callback(0, ptr::null_mut()), 1);
    }

    #[test]
    fn password_copies_when_it_fits() {
        let passphrase = CString::new("swoole").unwrap();
        let mut buffer = [0x55u8; 8];
        let copy = |buffer: &mut [u8; 8], capacity: c_int, text: &CString| {
            let capacity = usize::try_from(capacity).unwrap_or(0);
            let lent =
                unsafe { std::slice::from_raw_parts_mut(buffer.as_mut_ptr().cast(), capacity) };
            copy_password(lent, text.as_bytes_with_nul())
        };
        assert_eq!(copy(&mut buffer, 8, &passphrase), 6);
        assert_eq!(&buffer[..7], b"swoole\0");
        for capacity in [c_int::MIN, -1, 0, 7] {
            assert_eq!(copy(&mut buffer, capacity, &passphrase), 0);
        }
        assert_eq!(copy(&mut buffer, 8, &CString::new("xy").unwrap()), 2);
    }

    #[test]
    fn alpn_prefers_h2() {
        let offered = b"\x08http/1.1\x02h2";
        let mut out = ptr::null();
        let mut length = 0;
        let chosen = unsafe {
            select_alpn(
                ptr::null_mut(),
                &raw mut out,
                &raw mut length,
                offered.as_ptr(),
                12,
                &H2_HTTP1,
            )
        };
        assert_eq!(chosen, SSL_TLSEXT_ERR_OK);
        assert_eq!(
            unsafe { std::slice::from_raw_parts(out, length.into()) },
            b"h2"
        );
        let chosen = unsafe {
            select_alpn(
                ptr::null_mut(),
                &raw mut out,
                &raw mut length,
                b"\x02h2".as_ptr(),
                3,
                &HTTP1,
            )
        };
        assert_eq!(chosen, SSL_TLSEXT_ERR_NOACK);
    }

    #[test]
    fn init_and_error_text() {
        assert_eq!(swr_v1_tls_init(), Ok(()));
        assert!(swr_v1_tls_connection_index() >= 0);
        assert_ne!(swr_v1_tls_connection_index(), swr_v1_tls_port_index());
        unsafe { ERR_clear_error() };
        assert_eq!(error_line(), "");
        // A failed open queues OpenSSL's errors (fopen's, then the BIO's), which the
        // callback receives as lines: it returns 0, so, as upstream's, it reads one per call.
        assert!(crate::OwnedBio::open(c"/nonexistent/swoole-rs-tls.pem").is_none());
        for _ in 0..2 {
            let line = error_line();
            assert!(line.contains(":error:"), "{line}");
            assert!(line.to_lowercase().contains("no such file"), "{line}");
        }
        assert_eq!(error_line(), "");
    }
}
