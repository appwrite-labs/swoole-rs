//! Packet codecs and protocol decisions over borrowed bytes: MQTT framing, SOCKS5,
//! RESP and PHP RESP replies. Their C ABI and upstream-symbol entries are
//! crates/protocols-ffi; the `MessageBus` is crates/bus.
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
