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
    // Upstream's renegotiation mark (`SSL_CB_HANDSHAKE_START` once ready) is read
    // nowhere, so only a done handshake matters.
    if where_ & 0x2001 != 0x2001 {
        return;
    }
    let Some(socket) = ssl.socket() else {
        return;
    };
    let write = ssl.write_bio();
    if ssl.read_bio() != write
        && socket.edit(crate::reactor::SocketFields::claim_handshake_buffer)
        && let Some(write) = write
    {
        write.set_write_buffer_size(16384);
    }
}
