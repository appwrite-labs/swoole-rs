//! `swoole::SSLContext`'s OpenSSL side: library initialization and the `ex_data`
//! indexes, the error text, creating and configuring the `SSL_CTX`, and its
//! callbacks (password, ALPN, verify, DTLS cookies). The configuration lives in the
//! owner (`owned.rs`), the info callback in `info.rs`; warnings go through the
//! configuration's `warn` callback. Requires OpenSSL 3.
use openssl_sys::{
    BIO, BIO_ctrl, DTLS_method, ERR_clear_error, ERR_get_error, ERR_reason_error_string, EVP_MD,
    EVP_PKEY, EVP_PKEY_free, EVP_sha1, OPENSSL_NPN_NEGOTIATED, OPENSSL_init_ssl, SSL,
    SSL_CTRL_SET_GROUPS_LIST, SSL_CTX, SSL_CTX_check_private_key, SSL_CTX_clear_options,
    SSL_CTX_ctrl, SSL_CTX_free, SSL_CTX_load_verify_locations, SSL_CTX_new,
    SSL_CTX_set_alpn_protos, SSL_CTX_set_alpn_select_cb__fixed_rust, SSL_CTX_set_cipher_list,
    SSL_CTX_set_client_CA_list, SSL_CTX_set_cookie_generate_cb, SSL_CTX_set_cookie_verify_cb,
    SSL_CTX_set_default_verify_paths, SSL_CTX_set_mode, SSL_CTX_set_options,
    SSL_CTX_set_read_ahead, SSL_CTX_set_session_cache_mode, SSL_CTX_set_session_id_context,
    SSL_CTX_set_verify, SSL_CTX_set_verify_depth, SSL_CTX_use_PrivateKey_file,
    SSL_CTX_use_certificate_chain_file, SSL_CTX_use_certificate_file, SSL_FILETYPE_PEM,
    SSL_MODE_NO_AUTO_CHAIN, SSL_MODE_RELEASE_BUFFERS, SSL_OP_CIPHER_SERVER_PREFERENCE,
    SSL_OP_DONT_INSERT_EMPTY_FRAGMENTS, SSL_OP_NO_COMPRESSION, SSL_OP_NO_SSLv2, SSL_OP_NO_SSLv3,
    SSL_OP_NO_TLSv1, SSL_OP_NO_TLSv1_1, SSL_OP_NO_TLSv1_2, SSL_OP_NO_TLSv1_3,
    SSL_SESS_CACHE_SERVER, SSL_TLSEXT_ERR_NOACK, SSL_TLSEXT_ERR_OK, SSL_VERIFY_NONE,
    SSL_VERIFY_PEER, SSL_get_ex_new_index, SSL_get_wbio, SSL_load_client_CA_file,
    SSL_select_next_proto, TLS_method, X509_STORE_CTX,
};
use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_int, c_long, c_uchar, c_uint, c_void};
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use swoole_rs_corebase::log;

// SAFETY: libssl/libcrypto's own declarations; openssl-sys binds none of them, or
// (`SSL_CTX_set0_tmp_dh_pkey`) only over OpenSSL 3 headers, which the fuzz image,
// building crates/clients' parsers against OpenSSL 1.1, lacks.
unsafe extern "C" {
    fn SSL_CTX_set0_tmp_dh_pkey(ctx: *mut SSL_CTX, key: *mut EVP_PKEY) -> c_int;
    fn BIO_free(bio: *mut BIO) -> c_int;
    fn BIO_new_file(filename: *const c_char, mode: *const c_char) -> *mut BIO;
    fn PEM_read_bio_Parameters(bio: *mut BIO, out: *mut *mut EVP_PKEY) -> *mut EVP_PKEY;
    fn ERR_print_errors_cb(
        callback: extern "C" fn(*const c_char, usize, *mut c_void) -> c_int,
        user: *mut c_void,
    );
    fn HMAC(
        md: *const EVP_MD,
        key: *const c_void,
        key_len: c_int,
        data: *const c_uchar,
        data_len: usize,
        out: *mut c_uchar,
        out_len: *mut c_uint,
    ) -> *mut c_uchar;
    fn SSL_CTX_set_default_passwd_cb(
        ctx: *mut SSL_CTX,
        callback: extern "C" fn(*mut c_char, c_int, c_int, *mut c_void) -> c_int,
    );
    fn SSL_CTX_set_default_passwd_cb_userdata(ctx: *mut SSL_CTX, data: *mut c_void);
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
/// `BIO_CTRL_DGRAM_GET_PEER`.
const BIO_CTRL_DGRAM_GET_PEER: c_int = 46;

/// A native `std::string`: its `c_str()` and `length()`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TlsStr {
    pub data: *const c_char,
    pub length: usize,
}

impl From<swoole_rs_corebase::CoreStringView> for TlsStr {
    fn from(value: swoole_rs_corebase::CoreStringView) -> Self {
        Self {
            data: value.data,
            length: value.length,
        }
    }
}

impl TlsStr {
    const fn is_empty(self) -> bool {
        self.length == 0
    }

    /// What `%s` prints: the bytes before the first NUL.
    fn text(self) -> Vec<u8> {
        // SAFETY: `data` is a NUL-terminated `c_str()` that outlives the call that lent it.
        unsafe { CStr::from_ptr(self.data) }.to_bytes().to_vec()
    }

    /// The `c_str()`, or null when empty (what upstream passes for an unset path).
    fn or_null(self) -> *const c_char {
        if self.is_empty() {
            ptr::null()
        } else {
            self.data
        }
    }
}

/// Logs `"<function>(): <message>"` at warning level.
pub type Warn = extern "C" fn(function: *const c_char, message: *const c_char);

/// Reads an owner's current passphrase.
pub type PassphraseOf = extern "C" fn(owner: *const c_void) -> TlsStr;

/// A view of `swoole::SSLContext`'s configuration, built by native code for one call.
#[repr(C)]
#[derive(Debug)]
pub struct TlsConfig {
    pub ciphers: TlsStr,
    pub ecdh_curve: TlsStr,
    pub dhparam: TlsStr,
    pub cert_file: TlsStr,
    pub key_file: TlsStr,
    pub passphrase: TlsStr,
    pub client_cert_file: TlsStr,
    pub cafile: TlsStr,
    pub capath: TlsStr,
    /// In: the configured mask. Out (create): zero becomes every TLS version.
    pub protocols: u32,
    pub verify_depth: u8,
    pub http: bool,
    pub http_v2: bool,
    pub prefer_server_ciphers: bool,
    pub disable_compress: bool,
    pub verify_peer: bool,
    pub warn: Warn,
    /// The native `SSLContext`, which the password callback reads through `passphrase_of`.
    pub owner: *const c_void,
    pub passphrase_of: PassphraseOf,
}

/// One upstream method: its log name and the configuration it reads.
#[derive(Clone, Copy)]
pub(crate) struct PasswordBinding {
    pub userdata: *mut c_void,
    pub callback: extern "C" fn(*mut c_char, c_int, c_int, *mut c_void) -> c_int,
}

struct Method<'a> {
    config: &'a TlsConfig,
    ctx: *mut SSL_CTX,
    function: &'static CStr,
    password: Option<PasswordBinding>,
}

impl Method<'_> {
    fn warn(&self, message: &[u8]) {
        let mut text: Vec<u8> = message.iter().copied().filter(|&b| b != 0).collect();
        text.push(0);
        (self.config.warn)(self.function.as_ptr(), text.as_ptr().cast());
    }

    /// Upstream's `ssl_error`: pops one error and appends its reason and code.
    fn ssl_error(&self, message: &[u8]) {
        // SAFETY: pops this thread's OpenSSL error queue.
        let code = unsafe { ERR_get_error() };
        // SAFETY: returns a static string or null.
        let reason = unsafe { ERR_reason_error_string(code) };
        let reason: &[u8] = if reason.is_null() {
            b"(null)"
        } else {
            // SAFETY: OpenSSL's static, NUL-terminated reason string.
            unsafe { CStr::from_ptr(reason) }.to_bytes()
        };
        #[expect(
            clippy::cast_possible_wrap,
            reason = "upstream stores ERR_get_error's unsigned long in a long and prints %ld"
        )]
        let code = code as c_long;
        let mut text = message.to_vec();
        text.extend_from_slice(b", Error: ");
        text.extend_from_slice(reason);
        text.extend_from_slice(format!("[{code}]").as_bytes());
        self.warn(&text);
    }

    fn set_capath(&self) -> bool {
        let config = self.config;
        if !config.cafile.is_empty() || !config.capath.is_empty() {
            // SAFETY: a live context and NUL-terminated paths or null.
            let loaded = unsafe {
                SSL_CTX_load_verify_locations(
                    self.ctx,
                    config.cafile.or_null(),
                    config.capath.or_null(),
                )
            };
            if loaded == 0 {
                return false;
            }
        } else {
            // SAFETY: a live context.
            if unsafe { SSL_CTX_set_default_verify_paths(self.ctx) } == 0 {
                self.ssl_error(b"SSL_CTX_set_default_verify_paths() failed");
                return false;
            }
        }
        if config.verify_depth > 0 {
            // SAFETY: a live context.
            unsafe { SSL_CTX_set_verify_depth(self.ctx, c_int::from(config.verify_depth)) };
        }
        true
    }

    fn set_ciphers(&self) -> bool {
        let config = self.config;
        if !config.ciphers.is_empty() {
            // SAFETY: a live context and a NUL-terminated cipher list.
            if unsafe { SSL_CTX_set_cipher_list(self.ctx, config.ciphers.data) } == 0 {
                self.ssl_error(&quoted(
                    b"SSL_CTX_set_cipher_list(\"",
                    config.ciphers,
                    b"\") failed",
                ));
                return false;
            }
            if config.prefer_server_ciphers
                // SAFETY: a live context.
                && unsafe { SSL_CTX_set_options(self.ctx, SSL_OP_CIPHER_SERVER_PREFERENCE) } == 0
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
        // SAFETY: a live context; the callback is a plain function.
        unsafe { SSL_CTX_set_verify(self.ctx, SSL_VERIFY_PEER, Some(verify_callback)) };
        // SAFETY: a live context.
        unsafe { SSL_CTX_set_verify_depth(self.ctx, c_int::from(self.config.verify_depth)) };
        // SAFETY: a live context and a NUL-terminated path.
        if unsafe { SSL_CTX_load_verify_locations(self.ctx, file.data, ptr::null()) } == 0 {
            self.ssl_error(&quoted(
                b"SSL_CTX_load_verify_locations(\"",
                file,
                b"\") failed",
            ));
            return false;
        }
        // SAFETY: clears this thread's error queue.
        unsafe { ERR_clear_error() };
        // SAFETY: a NUL-terminated path.
        let names = unsafe { SSL_load_client_CA_file(file.data) };
        if names.is_null() {
            self.ssl_error(&quoted(b"SSL_load_client_CA_file(\"", file, b"\") failed"));
            return false;
        }
        // SAFETY: clears this thread's error queue.
        unsafe { ERR_clear_error() };
        // SAFETY: a live context takes ownership of the list.
        unsafe { SSL_CTX_set_client_CA_list(self.ctx, names) };
        true
    }

    fn set_ecdh_curve(&self) -> bool {
        let curve = self.config.ecdh_curve;
        // SSL_OP_SINGLE_ECDH_USE is 0 and ECDH is automatic in OpenSSL 3.
        if curve.text() == b"auto" {
            return true;
        }
        // SAFETY: `SSL_CTX_set1_curves_list` on a live context and a NUL-terminated list.
        let set = unsafe {
            SSL_CTX_ctrl(
                self.ctx,
                SSL_CTRL_SET_GROUPS_LIST,
                0,
                curve.data.cast_mut().cast(),
            )
        };
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
        // SAFETY: a NUL-terminated path and mode.
        let bio = unsafe { BIO_new_file(file.data, c"r".as_ptr()) };
        if bio.is_null() {
            self.ssl_error(&quoted(b"BIO_new_file(", file, b") failed"));
            return false;
        }
        // SAFETY: a live file BIO.
        let key = unsafe { PEM_read_bio_Parameters(bio, ptr::null_mut()) };
        if key.is_null() {
            self.ssl_error(&quoted(b"PEM_read_bio_Parameters('", file, b"') failed"));
            // SAFETY: the BIO opened above.
            unsafe { BIO_free(bio) };
            return false;
        }
        // SAFETY: a live context takes the key on success.
        if unsafe { SSL_CTX_set0_tmp_dh_pkey(self.ctx, key) } != 1 {
            self.ssl_error(&quoted(b"SSL_CTX_set0_tmp_dh_pkey('", file, b"') failed"));
            // SAFETY: the key the context refused.
            unsafe { EVP_PKEY_free(key) };
            // SAFETY: the BIO opened above.
            unsafe { BIO_free(bio) };
            return false;
        }
        // SAFETY: the BIO opened above.
        unsafe { BIO_free(bio) };
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
        let protocols = config.protocols;
        // The other legacy SSL_OP_*_BUG workarounds are 0 in OpenSSL 3.
        // SAFETY: option, mode and callback setters on a live context, here and below.
        unsafe { SSL_CTX_set_options(ctx, SSL_OP_DONT_INSERT_EMPTY_FRAGMENTS) };
        // SAFETY: as above.
        unsafe { SSL_CTX_clear_options(ctx, SSL_OP_NO_SSLv2 | SSL_OP_NO_SSLv3 | SSL_OP_NO_TLSv1) };
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
        // SAFETY: as above.
        unsafe {
            SSL_CTX_clear_options(
                ctx,
                SSL_OP_NO_TLSv1_1 | SSL_OP_NO_TLSv1_2 | SSL_OP_NO_TLSv1_3,
            )
        };
        // SAFETY: as above.
        unsafe { SSL_CTX_set_options(ctx, disable) };
        if config.disable_compress {
            // SAFETY: as above.
            unsafe { SSL_CTX_set_options(ctx, SSL_OP_NO_COMPRESSION) };
        }
        // SAFETY: as above.
        unsafe { SSL_CTX_set_mode(ctx, SSL_MODE_RELEASE_BUFFERS | SSL_MODE_NO_AUTO_CHAIN) };
        // SAFETY: as above.
        unsafe { SSL_CTX_set_read_ahead(ctx, 1) };

        if !config.passphrase.is_empty() {
            let binding = self.password.unwrap_or_else(|| {
                PASSPHRASE_OF.get_or_init(|| config.passphrase_of);
                PasswordBinding {
                    userdata: config.owner.cast_mut(),
                    callback: password_callback,
                }
            });
            // SAFETY: a live context; the binding's owner outlives the borrowed context.
            unsafe { SSL_CTX_set_default_passwd_cb_userdata(ctx, binding.userdata) };
            // SAFETY: a live context; a plain callback with the binding's userdata.
            unsafe { SSL_CTX_set_default_passwd_cb(ctx, binding.callback) };
        }
        if !config.cert_file.is_empty() {
            let file = config.cert_file;
            // SAFETY: a live context and a NUL-terminated path.
            if unsafe { SSL_CTX_use_certificate_file(ctx, file.data, SSL_FILETYPE_PEM) } <= 0 {
                self.ssl_error(&quoted(b"SSL_CTX_use_certificate_file(", file, b") failed"));
                return false;
            }
            // SAFETY: as above.
            if unsafe { SSL_CTX_use_certificate_chain_file(ctx, file.data) } <= 0 {
                self.ssl_error(&quoted(
                    b"SSL_CTX_use_certificate_chain_file(",
                    file,
                    b") failed",
                ));
                return false;
            }
        }
        if !config.key_file.is_empty() {
            let file = config.key_file;
            // SAFETY: a live context and a NUL-terminated path.
            if unsafe { SSL_CTX_use_PrivateKey_file(ctx, file.data, SSL_FILETYPE_PEM) } <= 0 {
                self.ssl_error(&quoted(b"SSL_CTX_use_PrivateKey_file(", file, b") failed"));
                return false;
            }
            // SAFETY: a live context.
            if unsafe { SSL_CTX_check_private_key(ctx) } == 0 {
                self.ssl_error(b"SSL_CTX_check_private_key() failed");
                return false;
            }
        }
        if protocols & DTLS != 0 {
            // SAFETY: a live context; plain functions.
            unsafe { SSL_CTX_set_cookie_generate_cb(ctx, Some(generate_cookie)) };
            // SAFETY: as above.
            unsafe { SSL_CTX_set_cookie_verify_cb(ctx, Some(verify_cookie)) };
        }
        let capath = self.with(c"SSLContext::set_capath").set_capath();
        if config.verify_peer {
            if !capath {
                return false;
            }
        } else {
            // SAFETY: a live context.
            unsafe { SSL_CTX_set_verify(ctx, SSL_VERIFY_NONE, None) };
        }
        if config.http || config.http_v2 {
            let protos: &'static Protos = if config.http_v2 { &H2_HTTP1 } else { &HTTP1 };
            #[expect(
                clippy::cast_possible_truncation,
                reason = "both ALPN lists are a few bytes"
            )]
            let length = protos.0.len() as c_uint;
            // SAFETY: a live context and a static list.
            if unsafe { SSL_CTX_set_alpn_protos(ctx, protos.0.as_ptr(), length) } < 0 {
                self.ssl_error(&[b"SSL_CTX_set_alpn_protos(", protos.0, b") failed"].concat());
                return false;
            }
            // SAFETY: a live context; the argument is a static.
            unsafe {
                SSL_CTX_set_alpn_select_cb__fixed_rust(
                    ctx,
                    Some(select_alpn),
                    ptr::from_ref(protos).cast_mut().cast(),
                );
            }
            // SAFETY: a live context and a static id.
            unsafe { SSL_CTX_set_session_id_context(ctx, b"HTTP".as_ptr(), 4) };
            // SAFETY: a live context.
            unsafe { SSL_CTX_set_session_cache_mode(ctx, SSL_SESS_CACHE_SERVER) };
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

fn quoted(prefix: &[u8], value: TlsStr, suffix: &[u8]) -> Vec<u8> {
    [prefix, &value.text(), suffix].concat()
}

/// An ALPN wire-format list.
struct Protos(&'static [u8]);
static H2_HTTP1: Protos = Protos(b"\x02h2\x08http/1.1");
static HTTP1: Protos = Protos(b"\x08http/1.1");

/// The server's ALPN choice: the first of its protocols the client offers.
unsafe extern "C" fn select_alpn(
    _ssl: *mut SSL,
    out: *mut *const c_uchar,
    out_length: *mut c_uchar,
    offered: *const c_uchar,
    offered_length: c_uint,
    arg: *mut c_void,
) -> c_int {
    // SAFETY: `arg` is one of the static lists installed by `configure`.
    let protos = unsafe { &*arg.cast::<Protos>() };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "both ALPN lists are a few bytes"
    )]
    let length = protos.0.len() as c_uint;
    // SAFETY: OpenSSL's output slots and offered list; ours is static.
    let negotiated = unsafe {
        SSL_select_next_proto(
            out.cast(),
            out_length,
            protos.0.as_ptr(),
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

/// Peer certificates are checked later by the callers; the chain walk always continues.
extern "C" fn verify_callback(_ok: c_int, _store: *mut X509_STORE_CTX) -> c_int {
    1
}

/// Native code's passphrase reader, the same function for every context.
static PASSPHRASE_OF: OnceLock<PassphraseOf> = OnceLock::new();

/// Copies the owner's current passphrase when it fits with a spare byte, else reports none.
extern "C" fn password_callback(
    buffer: *mut c_char,
    capacity: c_int,
    _verify: c_int,
    owner: *mut c_void,
) -> c_int {
    let Some(passphrase_of) = PASSPHRASE_OF.get() else {
        return 0;
    };
    let passphrase = passphrase_of(owner.cast_const());
    // SAFETY: OpenSSL lends the buffer; the reader lends the live passphrase bytes.
    unsafe { copy_password(buffer, capacity, passphrase) }
}

/// # Safety
/// Passphrase bytes remain live; buffer holds capacity writable bytes when positive.
pub(crate) unsafe fn copy_password(
    buffer: *mut c_char,
    capacity: c_int,
    passphrase: TlsStr,
) -> c_int {
    let Ok(length) = c_int::try_from(passphrase.length) else {
        return 0;
    };
    if passphrase.is_empty() || i64::from(length) >= i64::from(capacity) - 1 {
        return 0;
    }
    // SAFETY: the owner's `length` bytes into OpenSSL's buffer of `capacity` > `length + 1` bytes.
    unsafe { ptr::copy_nonoverlapping(passphrase.data, buffer, passphrase.length) };
    // SAFETY: as above; the terminator is inside the buffer.
    let terminator = unsafe { buffer.add(passphrase.length) };
    // SAFETY: as above.
    unsafe { terminator.write(0) };
    length
}

/// DTLS cookie: HMAC-SHA1 of the peer's port and address, keyed by the SSL's address.
extern "C" fn generate_cookie(
    ssl: *mut SSL,
    cookie: *mut c_uchar,
    cookie_length: *mut c_uint,
) -> c_int {
    let secret = [(ssl as c_long).to_ne_bytes(); 32 / size_of::<c_long>()].concat();
    // SAFETY: zero is a valid sockaddr_storage.
    let mut peer: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    // SAFETY: OpenSSL's live SSL, in its cookie callback.
    let wbio = unsafe { SSL_get_wbio(ssl) };
    // SAFETY: `BIO_dgram_get_peer`, which copies at most a sockaddr_storage.
    unsafe { BIO_ctrl(wbio, BIO_CTRL_DGRAM_GET_PEER, 0, (&raw mut peer).cast()) };
    let mut data = Vec::with_capacity(18);
    match c_int::from(peer.ss_family) {
        libc::AF_INET => {
            // SAFETY: an AF_INET peer is a sockaddr_in.
            let v4 = unsafe { &*(&raw const peer).cast::<libc::sockaddr_in>() };
            data.extend_from_slice(&v4.sin_port.to_ne_bytes());
            data.extend_from_slice(&v4.sin_addr.s_addr.to_ne_bytes());
        }
        libc::AF_INET6 => {
            // SAFETY: an AF_INET6 peer is a sockaddr_in6.
            let v6 = unsafe { &*(&raw const peer).cast::<libc::sockaddr_in6>() };
            data.extend_from_slice(&v6.sin6_port.to_ne_bytes());
            data.extend_from_slice(&v6.sin6_addr.s6_addr);
        }
        // Upstream's OPENSSL_assert(0).
        _ => std::process::abort(),
    }
    let mut result = [0u8; 64];
    let mut length: c_uint = 0;
    // SAFETY: a static digest.
    let sha1 = unsafe { EVP_sha1() };
    // SAFETY: `HMAC` over local buffers; `result` holds EVP_MAX_MD_SIZE bytes.
    let digest = unsafe {
        HMAC(
            sha1,
            secret.as_ptr().cast(),
            32,
            data.as_ptr(),
            data.len(),
            result.as_mut_ptr(),
            &raw mut length,
        )
    };
    if digest.is_null() {
        return 0;
    }
    // SAFETY: OpenSSL's cookie buffer holds DTLS1_COOKIE_LENGTH (255) bytes.
    unsafe { ptr::copy_nonoverlapping(result.as_ptr(), cookie, length as usize) };
    // SAFETY: OpenSSL's length slot.
    unsafe { cookie_length.write(length) };
    1
}

extern "C" fn verify_cookie(ssl: *mut SSL, cookie: *const c_uchar, cookie_length: c_uint) -> c_int {
    let mut expected = [0u8; 64];
    let mut length: c_uint = 0;
    generate_cookie(ssl, expected.as_mut_ptr(), &raw mut length);
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

fn new_ssl_index() -> c_int {
    // SAFETY: allocates an SSL ex_data index without callbacks.
    unsafe { SSL_get_ex_new_index(0, ptr::null_mut(), None, None, None) }
}

/// `void swoole_ssl_init()`: once until `swoole_ssl_destroy`. A failure logs at
/// error level and exits the process, as upstream's `swoole_error` does.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z15swoole_ssl_initv")
)]
pub extern "C" fn swoole_ssl_init() {
    match swr_v1_tls_init() {
        1 => log::error(
            c"swoole_ssl_init",
            format_args!("OPENSSL_init_ssl() failed"),
        ),
        2 => log::error(
            c"swoole_ssl_init",
            format_args!("SSL_get_ex_new_index() failed"),
        ),
        _ => {}
    }
}

/// 0 done (or already), 1 `OPENSSL_init_ssl` failed, 2 an index failed.
pub fn swr_v1_tls_init() -> c_int {
    if INITIALIZED.load(Ordering::Acquire) {
        return 0;
    }
    // SAFETY: library initialization with default settings.
    if unsafe { OPENSSL_init_ssl(INIT_OPTIONS, ptr::null()) } == 0 {
        return 1;
    }
    let connection = new_ssl_index();
    if connection < 0 {
        return 2;
    }
    CONNECTION_INDEX.store(connection, Ordering::Release);
    let port = new_ssl_index();
    if port < 0 {
        return 2;
    }
    PORT_INDEX.store(port, Ordering::Release);
    INITIALIZED.store(true, Ordering::Release);
    0
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

extern "C" fn keep_last_line(line: *const c_char, length: usize, _user: *mut c_void) -> c_int {
    // SAFETY: OpenSSL lends `length` bytes of the line.
    let line = unsafe { std::slice::from_raw_parts(line.cast::<u8>(), length) };
    ERROR_TEXT.with_borrow_mut(|text| {
        text.clear();
        text.extend_from_slice(line);
        text.push(0);
    });
    0
}

/// `swoole_ssl_get_error`: drains this thread's error queue and returns its last
/// line ("" when empty), valid until the thread's next call.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z20swoole_ssl_get_errorv")
)]
pub extern "C" fn swr_v1_tls_error() -> *const c_char {
    ERROR_TEXT.with_borrow_mut(|text| {
        text.clear();
        text.push(0);
    });
    // SAFETY: the callback only touches this thread's buffer.
    unsafe { ERR_print_errors_cb(keep_last_line, ptr::null_mut()) };
    ERROR_TEXT.with_borrow(|text| text.as_ptr().cast())
}

/// `SSLContext::create`: a configured context, or null after logging why. Writes
/// the normalized protocol mask back to `config.protocols`.
///
/// # Safety
/// `config` is a live view whose strings are NUL-terminated and outlive the call.
pub unsafe fn swr_v1_tls_context_create(config: *mut TlsConfig) -> *mut SSL_CTX {
    // SAFETY: forwards the caller's live configuration view.
    unsafe { create_context(config, None) }
}

/// # Safety
/// Config and any password binding remain live throughout the borrowed context.
pub(crate) unsafe fn create_context(
    config: *mut TlsConfig,
    password: Option<PasswordBinding>,
) -> *mut SSL_CTX {
    // SAFETY: the caller's live view.
    let config = unsafe { &mut *config };
    let method = if config.protocols & DTLS != 0 {
        // SAFETY: a static method table.
        unsafe { DTLS_method() }
    } else {
        // SAFETY: a static method table.
        unsafe { TLS_method() }
    };
    if config.protocols == 0 {
        config.protocols = ALL;
    }
    // SAFETY: creates a context from a static method.
    let ctx = unsafe { SSL_CTX_new(method) };
    let create = Method {
        config,
        ctx,
        function: c"SSLContext::create",
        password,
    };
    if ctx.is_null() {
        create.ssl_error(b"SSL_CTX_new() failed");
        return ptr::null_mut();
    }
    if create.configure() {
        return ctx;
    }
    // SAFETY: the context created above.
    unsafe { SSL_CTX_free(ctx) };
    ptr::null_mut()
}

macro_rules! method {
    ($name:ident, $function:literal, $body:ident) => {
        /// The `SSLContext` method of the same name, on an existing context.
        ///
        /// # Safety
        /// `ctx` is a live context and `config` a live view whose strings are
        /// NUL-terminated and outlive the call.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(ctx: *mut SSL_CTX, config: *const TlsConfig) -> bool {
            Method {
                // SAFETY: the caller's live view.
                config: unsafe { &*config },
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
    swr_v1_tls_set_ciphers,
    c"SSLContext::set_ciphers",
    set_ciphers
);
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

#[cfg(test)]
mod tests {
    use super::*;
    use openssl_sys::{SSL_CTX_get_options, SSL_CTX_get_verify_mode};

    unsafe extern "C" {
        fn SSL_CTX_get_verify_depth(ctx: *const SSL_CTX) -> c_int;
    }
    use std::ffi::CString;
    use std::sync::Mutex;

    static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

    extern "C" fn record(function: *const c_char, message: *const c_char) {
        // SAFETY: the NUL-terminated strings `Method::warn` passes.
        let (function, message) = unsafe { (CStr::from_ptr(function), CStr::from_ptr(message)) };
        LOG.lock().unwrap().push(format!(
            "{}(): {}",
            function.to_string_lossy(),
            message.to_string_lossy()
        ));
    }

    extern "C" fn no_passphrase(_owner: *const c_void) -> TlsStr {
        TlsStr {
            data: c"".as_ptr(),
            length: 0,
        }
    }

    struct Owned(Vec<CString>);

    impl Owned {
        fn str(&mut self, value: &str) -> TlsStr {
            let value = CString::new(value).unwrap();
            let view = TlsStr {
                data: value.as_ptr(),
                length: value.as_bytes().len(),
            };
            self.0.push(value);
            view
        }

        fn config(&mut self) -> TlsConfig {
            let empty = self.str("");
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
                owner: ptr::null(),
                passphrase_of: no_passphrase,
            }
        }
    }

    fn take_log() -> Vec<String> {
        std::mem::take(&mut *LOG.lock().unwrap())
    }

    #[test]
    fn protocols_and_options() {
        let _guard = SERIAL.lock().unwrap();
        let mut owned = Owned(Vec::new());
        let mut config = owned.config();
        unsafe {
            let ctx = swr_v1_tls_context_create(&raw mut config);
            assert!(!ctx.is_null());
            assert_eq!(config.protocols, ALL, "zero means every version");
            let options = SSL_CTX_get_options(ctx);
            assert_eq!(options & (SSL_OP_NO_TLSv1_2 | SSL_OP_NO_TLSv1_3), 0);
            assert_eq!(SSL_CTX_get_verify_mode(ctx), SSL_VERIFY_NONE);
            SSL_CTX_free(ctx);

            config.protocols = TLSV1_3 | DTLS;
            config.disable_compress = true;
            let ctx = swr_v1_tls_context_create(&raw mut config);
            let options = SSL_CTX_get_options(ctx);
            assert_ne!(options & SSL_OP_NO_TLSv1_1, 0);
            assert_eq!(options & SSL_OP_NO_TLSv1_2, 0, "DTLS keeps 1.2");
            assert_eq!(options & SSL_OP_NO_TLSv1_3, 0);
            assert_ne!(options & SSL_OP_NO_COMPRESSION, 0);
            SSL_CTX_free(ctx);
        }
        assert_eq!(take_log(), Vec::<String>::new());
    }

    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn failures_log_like_upstream() {
        let _guard = SERIAL.lock().unwrap();
        take_log();
        let mut owned = Owned(Vec::new());
        let mut config = owned.config();
        config.cert_file = owned.str("/nonexistent/m21.pem");
        unsafe { ERR_clear_error() };
        let ctx = unsafe { swr_v1_tls_context_create(&raw mut config) };
        assert!(ctx.is_null());
        let log = take_log();
        assert_eq!(log.len(), 1);
        assert!(
            log[0].starts_with(
                "SSLContext::create(): SSL_CTX_use_certificate_file(/nonexistent/m21.pem) failed, Error: "
            ),
            "{log:?}"
        );

        let mut config = owned.config();
        config.ciphers = owned.str("M21-nonexistent-cipher");
        let ctx = unsafe { swr_v1_tls_context_create(&raw mut config) };
        assert!(ctx.is_null());
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
        let mut owned = Owned(Vec::new());
        let mut config = owned.config();
        unsafe {
            let ctx = SSL_CTX_new(TLS_method());
            config.ecdh_curve = owned.str("auto");
            assert!(swr_v1_tls_set_ecdh_curve(ctx, &raw const config));
            config.ecdh_curve = owned.str("prime256v1");
            assert!(swr_v1_tls_set_ecdh_curve(ctx, &raw const config));
            config.ecdh_curve = owned.str("M21-nonexistent-curve");
            assert!(!swr_v1_tls_set_ecdh_curve(ctx, &raw const config));
            assert_eq!(
                take_log(),
                [
                    "SSLContext::set_ecdh_curve(): SSL_CTX_set1_curves_list(\"M21-nonexistent-curve\") failed"
                ]
            );

            config.dhparam = owned.str("/nonexistent/m21-dh.pem");
            assert!(!swr_v1_tls_set_dhparam(ctx, &raw const config));
            let log = take_log();
            assert!(log[0].starts_with(
                "SSLContext::set_dhparam(): BIO_new_file(/nonexistent/m21-dh.pem) failed"
            ));

            config.client_cert_file = owned.str("/nonexistent/m21-ca.pem");
            config.verify_depth = 3;
            assert!(!swr_v1_tls_set_client_certificate(ctx, &raw const config));
            assert_eq!(SSL_CTX_get_verify_mode(ctx), SSL_VERIFY_PEER);
            assert_eq!(SSL_CTX_get_verify_depth(ctx), 3);
            take_log();

            config.cafile = owned.str("/nonexistent/m21-ca.pem");
            assert!(!swr_v1_tls_set_capath(ctx, &raw const config));
            assert_eq!(
                take_log(),
                Vec::<String>::new(),
                "a failed CA load is silent"
            );
            SSL_CTX_free(ctx);
        }
        assert_eq!(verify_callback(0, ptr::null_mut()), 1);
    }

    #[test]
    fn password_copies_when_it_fits() {
        extern "C" fn read(owner: *const c_void) -> TlsStr {
            // SAFETY: the test's CString.
            let owner = unsafe { &*owner.cast::<CString>() };
            TlsStr {
                data: owner.as_ptr(),
                length: owner.as_bytes().len(),
            }
        }
        PASSPHRASE_OF.get_or_init(|| read);
        let mut passphrase = CString::new("swoole").unwrap();
        let owner: *mut c_void = (&raw mut passphrase).cast();
        let mut buffer = [0x55u8; 8];
        assert_eq!(
            password_callback(buffer.as_mut_ptr().cast(), 8, 0, owner),
            6
        );
        assert_eq!(&buffer[..7], b"swoole\0");
        for capacity in [c_int::MIN, -1, 0, 7] {
            assert_eq!(
                password_callback(buffer.as_mut_ptr().cast(), capacity, 0, owner),
                0
            );
        }
        *unsafe { &mut *owner.cast::<CString>() } = CString::new("xy").unwrap();
        assert_eq!(
            password_callback(buffer.as_mut_ptr().cast(), 8, 0, owner),
            2,
            "reads the live value"
        );
    }

    #[test]
    fn alpn_prefers_h2() {
        let offered = b"\x08http/1.1\x02h2";
        let mut out = ptr::null();
        let mut length = 0;
        let arg = ptr::from_ref(&H2_HTTP1).cast_mut().cast();
        let chosen = unsafe {
            select_alpn(
                ptr::null_mut(),
                &raw mut out,
                &raw mut length,
                offered.as_ptr(),
                12,
                arg,
            )
        };
        assert_eq!(chosen, SSL_TLSEXT_ERR_OK);
        assert_eq!(
            unsafe { std::slice::from_raw_parts(out, length.into()) },
            b"h2"
        );
        let arg = ptr::from_ref(&HTTP1).cast_mut().cast();
        let chosen = unsafe {
            select_alpn(
                ptr::null_mut(),
                &raw mut out,
                &raw mut length,
                b"\x02h2".as_ptr(),
                3,
                arg,
            )
        };
        assert_eq!(chosen, SSL_TLSEXT_ERR_NOACK);
    }

    #[test]
    fn init_and_error_text() {
        assert_eq!(swr_v1_tls_init(), 0);
        assert!(swr_v1_tls_connection_index() >= 0);
        assert_ne!(swr_v1_tls_connection_index(), swr_v1_tls_port_index());
        unsafe { ERR_clear_error() };
        let text = unsafe { CStr::from_ptr(swr_v1_tls_error()) };
        assert_eq!(text.to_bytes(), b"");
    }
}
