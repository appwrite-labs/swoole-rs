//! Packet codecs and protocol decisions over borrowed bytes: MQTT framing, SOCKS5,
//! RESP and PHP RESP replies. crates/clients' `mqtt`, `owned_proxy` and `resp_recv`
//! run them over the client's and server's records; the `MessageBus` is crates/bus.
#![forbid(unsafe_code)]
mod mqtt;
mod redis;
mod reply;
mod socks;
pub use mqtt::*;
pub use redis::*;
pub use reply::*;
pub use socks::*;

/// Copies `parts` back to back into `out`, stopping at the first part that no longer fits.
fn concat(mut out: &mut [u8], parts: &[&[u8]]) {
    for part in parts {
        let Some((head, tail)) = std::mem::take(&mut out).split_at_mut_checked(part.len()) else {
            return;
        };
        head.copy_from_slice(part);
        out = tail;
    }
}
