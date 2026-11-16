//! Upstream's `core-tests/src/protocol/ssl.cpp`, one function per TEST with the same
//! inputs and expected values, over crates/tls: `swoole_ssl_init`, `swoole_ssl_destroy`
//! (`swr_v1_tls_destroy`) and `swoole_ssl_get_error` (`swr_v1_tls_error`) by path,
//! `SSLContext::create` as crates/tls's `ssl_context_create` by path, and [`SslContext`]
//! holding an owner from `tls_owner_new` and setting it with `string_set`.

use std::ffi::{CStr, c_char, c_int, c_ulong, c_void};

use super::network_socket::err_put_error;
use super::socket::{TlsPrepared, get_ssl_dir, string_set, tls_owner_new};
use swoole_rs_tls::{swoole_ssl_init, swr_v1_tls_destroy, swr_v1_tls_error};

// OpenSSL's codes (the SSL_F_* function codes are 0 in OpenSSL 3).
const ERR_LIB_SSL: c_int = 20;
const SSL_R_CERTIFICATE_VERIFY_FAILED: c_int = 134;
const SSL_R_PROTOCOL_IS_SHUTDOWN: c_int = 207;
// SSLContext's string slots.
const CERT_FILE: usize = 4;
const KEY_FILE: usize = 5;
const PASSPHRASE: usize = 6;
/// `offsetof(swr_v1_tls_owner_fields, context)`.
const CONTEXT: usize = 24;

#[link(name = "crypto")]
unsafe extern "C" {
    fn ERR_clear_error();
    fn ERR_peek_error() -> c_ulong;
}

/// `swoole::SSLContext`: the owner's one reference and its view.
struct SslContext(TlsPrepared);

impl SslContext {
    fn new() -> Self {
        let prepared = tls_owner_new();
        assert!(!prepared.owner.is_null());
        Self(prepared)
    }

    /// `RustStringView::operator=(const std::string &)`.
    fn set(&self, slot: usize, value: &str) {
        // SAFETY: the live owner this context made, which only this test configures.
        let status = unsafe { string_set(self.0.owner, slot, value.as_bytes()) };
        assert!(status >= 0);
    }

    fn create(&self) -> bool {
        // SAFETY: the live owner this context made, which only this test configures.
        let owner = unsafe { &*self.0.owner.cast::<swoole_rs_tls::OwnedTlsContext>() };
        let mut owner = owner.borrow_configuration();
        swoole_rs_tls::ssl_context_create(&mut owner)
    }

    fn ready(&self) -> bool {
        // SAFETY: the owner's live fields, whose `SSL_CTX *context` is at CONTEXT.
        !unsafe {
            *self
                .0
                .fields
                .cast::<u8>()
                .add(CONTEXT)
                .cast::<*mut c_void>()
        }
        .is_null()
    }
}

impl Drop for SslContext {
    fn drop(&mut self) {
        // SAFETY: the owner's one reference this context made, released once.
        drop(unsafe {
            std::rc::Rc::<swoole_rs_tls::OwnedTlsContext>::from_raw(self.0.owner.cast())
        });
    }
}

fn get_error() -> *const c_char {
    swr_v1_tls_error()
}

/// `String(error_str).contains(needle)` over a non-null string.
fn contains(error_str: *const c_char, needle: &str) -> bool {
    // SAFETY: a terminated string from `swr_v1_tls_error`.
    unsafe { CStr::from_ptr(error_str) }
        .to_string_lossy()
        .contains(needle)
}

fn destroy() {
    swoole_ssl_init();
    swr_v1_tls_destroy();
    // SAFETY: this thread's OpenSSL error queue.
    assert_eq!(unsafe { ERR_peek_error() }, 0);
}

fn get_error_() {
    swoole_ssl_init();
    {
        // SAFETY: this thread's OpenSSL error queue.
        unsafe { ERR_clear_error() };
        err_put_error(ERR_LIB_SSL, SSL_R_CERTIFICATE_VERIFY_FAILED);
        let error_str = get_error();
        assert!(!error_str.is_null());
        assert!(contains(error_str, "certificate verify failed"));
    }
    {
        // SAFETY: as above.
        unsafe { ERR_clear_error() };

        err_put_error(ERR_LIB_SSL, SSL_R_CERTIFICATE_VERIFY_FAILED);
        err_put_error(ERR_LIB_SSL, SSL_R_PROTOCOL_IS_SHUTDOWN);

        let error_str = get_error();
        assert!(!error_str.is_null());

        let error_str2 = get_error();
        assert!(!error_str2.is_null());

        assert!(contains(error_str2, "protocol is shutdown"));

        let error_st3 = get_error();
        // SAFETY: as in `contains`.
        assert_eq!(unsafe { CStr::from_ptr(error_st3) }, c"");
    }
}

fn password() {
    let ctx = SslContext::new();
    ctx.set(KEY_FILE, &(get_ssl_dir() + "/passwd_key.pem"));
    ctx.set(PASSPHRASE, "swoole");
    ctx.set(CERT_FILE, &(get_ssl_dir() + "/passwd.crt"));
    assert!(ctx.create());
}

fn failed_create() {
    let ctx = SslContext::new();
    ctx.set(CERT_FILE, &(get_ssl_dir() + "/server.key"));
    assert!(!ctx.create());
    assert!(!ctx.ready());
}

pub(super) const CASES: super::Cases = &[
    ("destroy", destroy),
    ("get_error", get_error_),
    ("password", password),
    ("failed_create", failed_create),
];
