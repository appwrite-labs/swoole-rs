//! OpenSSL info callback over the authoritative reactor Socket record. Upstream's
//! renegotiation `swoole_debug` exists only in `SW_DEBUG` builds, which the hybrid isn't.
use openssl_sys::{SSL, SSL_CTX, SSL_get_ex_data, SSL_get_rbio, SSL_get_wbio};
use std::ffi::{c_int, c_long};
use swoole_rs_reactor::Socket;
// SAFETY: OpenSSL's declarations; openssl-sys does not bind these entry points.
unsafe extern "C" {
    pub(crate) fn SSL_CTX_set_info_callback(
        ctx: *mut SSL_CTX,
        callback: Option<unsafe extern "C" fn(*const SSL, c_int, c_int)>,
    );
    fn BIO_int_ctrl(
        bio: *mut openssl_sys::BIO,
        command: c_int,
        large: c_long,
        integer: c_int,
    ) -> c_long;
}
/// # Safety
/// `ssl` is OpenSSL's live SSL for the call, whose connection `ex_data` (which
/// `Socket::ssl_create` registers before handshake callbacks) is null or its live
/// socket record, dispatched by its owning event loop.
pub(crate) unsafe extern "C" fn info(ssl: *const SSL, where_: c_int, _result: c_int) {
    let renegotiating = where_ & 0x10 != 0;
    let handshake_done = where_ & 0x2001 == 0x2001;
    if !renegotiating && !handshake_done {
        return;
    }
    // SAFETY: per the caller.
    let socket = unsafe { SSL_get_ex_data(ssl, crate::swr_v1_tls_connection_index()) };
    // SAFETY: per the caller.
    let Some(socket) = (unsafe { Socket::from_ptr(socket.cast()) }) else {
        return;
    };
    if renegotiating {
        socket.edit(|record| {
            if record.tls.state == crate::constants::SslState::SW_SSL_STATE_READY as u32 {
                record.ssl_renegotiation = 1;
            }
        });
    }
    if handshake_done && socket.read(|record| record.ssl_handshake_buffer_set) == 0 {
        // SAFETY: the SSL remains live throughout its callback.
        let read = unsafe { SSL_get_rbio(ssl) };
        // SAFETY: the SSL remains live throughout its callback.
        let write = unsafe { SSL_get_wbio(ssl) };
        if read != write {
            // SAFETY: the SSL owns this buffering BIO; command117 sets its write buffer.
            unsafe { BIO_int_ctrl(write, 117, 16384, 1) };
            socket.edit(|record| record.ssl_handshake_buffer_set = 1);
        }
    }
}
