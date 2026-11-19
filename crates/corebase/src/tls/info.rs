//! OpenSSL info callback over the authoritative reactor Socket record. Upstream's
//! renegotiation `swoole_debug` exists only in `SW_DEBUG` builds, which the hybrid isn't.
use crate::tls::{Ssl, SslCtx};
use std::ffi::c_int;
// OpenSSL's declaration; openssl-sys (0.9.118, the latest) does not bind this entry point. It
// stores the callback in the live context, and calls it with the live, non-null SSL
// whose state changed, which is what a callback of this type takes.
unsafe extern "C" {
    pub(crate) safe fn SSL_CTX_set_info_callback(
        ctx: &SslCtx,
        callback: Option<extern "C" fn(Ssl<'_>, c_int, c_int)>,
    );
}
/// The info callback of every context `ssl_context_create` makes: `ssl` is
/// OpenSSL's live SSL for the call.
pub(crate) extern "C" fn info(ssl: Ssl<'_>, where_: c_int, _result: c_int) {
    let renegotiating = where_ & 0x10 != 0;
    let handshake_done = where_ & 0x2001 == 0x2001;
    if !renegotiating && !handshake_done {
        return;
    }
    let Some(socket) = ssl.socket() else {
        return;
    };
    if renegotiating {
        socket.edit(|record| {
            if record.tls.state == crate::tls::constants::SslState::SW_SSL_STATE_READY {
                record.ssl_renegotiation = true;
            }
        });
    }
    if handshake_done && !socket.read(|record| record.ssl_handshake_buffer_set) {
        let write = ssl.write_bio();
        if ssl.read_bio() != write {
            if let Some(write) = write {
                write.set_write_buffer_size(16384);
            }
            socket.edit(|record| record.ssl_handshake_buffer_set = true);
        }
    }
}
