//! The C ABI over crates/protocols' codecs: the `swr_v1_*` exports
//! `native/protocols/swoole_rs_protocols.h` declares, `swoole::mqtt` and
//! `swoole::Socks5Proxy` under their upstream symbols, and `redis::recv_packet`'s
//! loop. Each export borrows its pointers as slices for one call and keeps none.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
mod mqtt;
mod owned_proxy;
mod redis;
mod resp_recv;
mod socks;
pub use mqtt::*;
pub use owned_proxy::*;
pub use redis::*;
pub use resp_recv::*;
pub use socks::*;

/// # Safety
/// A non-null pointer covers `length` readable bytes borrowed for this call.
unsafe fn input<'a>(data: *const u8, length: usize) -> Option<&'a [u8]> {
    if length > isize::MAX as usize || (data.is_null() && length != 0) {
        None
    } else if length == 0 {
        Some(&[])
    } else {
        // SAFETY: non-null, `length <= isize::MAX`; the caller lends `length` readable bytes.
        Some(unsafe { std::slice::from_raw_parts(data, length) })
    }
}

/// The `capacity` writable elements at `data`, or none when it is null or too large,
/// so a codec finds them too short.
///
/// # Safety
/// A non-null `data` covers `capacity` exclusive writable elements, disjoint from inputs.
unsafe fn output<'a, T>(data: *mut T, capacity: usize) -> &'a mut [T] {
    if data.is_null() || capacity > isize::MAX as usize / size_of::<T>().max(1) {
        &mut []
    } else {
        // SAFETY: non-null and within isize::MAX bytes; the caller lends `capacity`
        // exclusive writable elements.
        unsafe { std::slice::from_raw_parts_mut(data, capacity) }
    }
}
