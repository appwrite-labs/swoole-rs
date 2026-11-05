//! OpenSSL info callback over the authoritative reactor Socket record. Native
//! supplies only its nonthrowing debug-log leaf during TLS initialization.
use openssl_sys::{SSL, SSL_CTX, SSL_get_ex_data, SSL_get_rbio, SSL_get_wbio};
use std::{
    ffi::{c_int, c_long},
    sync::{Mutex, PoisonError},
};
use swoole_rs_reactor::SocketRecord;
pub type InfoLog = extern "C" fn();
static LOG: Mutex<Option<InfoLog>> = Mutex::new(None);
// SAFETY: OpenSSL's declarations; openssl-sys does not bind these entry points.
unsafe extern "C" {
    fn SSL_CTX_set_info_callback(
        ctx: *mut SSL_CTX,
        callback: Option<extern "C" fn(*const SSL, c_int, c_int)>,
    );
    fn BIO_int_ctrl(
        bio: *mut openssl_sys::BIO,
        command: c_int,
        large: c_long,
        integer: c_int,
    ) -> c_long;
}
/// Install the module's debug leaf, or detach it during TLS teardown. The leaf
/// does not throw/reenter and stays loaded until the matching teardown.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_tls_info_log(callback: Option<InfoLog>) {
    *LOG.lock().unwrap_or_else(PoisonError::into_inner) = callback;
}
/// # Safety
/// ctx is live; each SSL's connection `ex_data` is a live, synchronized `SocketRecord`.
pub(crate) unsafe fn install(ctx: *mut SSL_CTX) {
    // SAFETY: a live context; callback observes its registered canonical socket.
    unsafe { SSL_CTX_set_info_callback(ctx, Some(info)) };
}
extern "C" fn info(ssl: *const SSL, where_: c_int, _result: c_int) {
    if where_ & 0x10 != 0 {
        // SAFETY: Socket::ssl_create registers the canonical record before handshake callbacks.
        let socket = unsafe { SSL_get_ex_data(ssl, crate::swr_v1_tls_connection_index()) }
            .cast::<SocketRecord>();
        // SAFETY: OpenSSL lends this SSL's live synchronized socket; copy its scalar state.
        let state = unsafe { (*socket).tls.state };
        if state == crate::constants::SslState::SW_SSL_STATE_READY as u32 {
            // SAFETY: the socket is exclusively dispatched by its owning event loop.
            unsafe { (*socket).ssl_renegotiation = 1 };
            let log = *LOG.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(log) = log {
                log();
            }
        }
    }
    if where_ & 0x2001 == 0x2001 {
        // SAFETY: the same connection record is live for this OpenSSL callback.
        let socket = unsafe { SSL_get_ex_data(ssl, crate::swr_v1_tls_connection_index()) }
            .cast::<SocketRecord>();
        // SAFETY: the event loop serializes this record's handshake state.
        if unsafe { (*socket).ssl_handshake_buffer_set } == 0 {
            // SAFETY: the SSL remains live throughout its callback.
            let read = unsafe { SSL_get_rbio(ssl) };
            // SAFETY: the SSL remains live throughout its callback.
            let write = unsafe { SSL_get_wbio(ssl) };
            if read != write {
                // SAFETY: the SSL owns this buffering BIO; command117 sets its write buffer.
                unsafe { BIO_int_ctrl(write, 117, 16384, 1) };
                // SAFETY: the owned socket records that its write buffer was configured.
                unsafe { (*socket).ssl_handshake_buffer_set = 1 };
            }
        }
    }
}
