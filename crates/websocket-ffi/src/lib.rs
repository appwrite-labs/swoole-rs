//! The WebSocket mask export of ABI v1 and `src/protocol/websocket.cc`'s entry points
//! ([`frame_api`]) over crates/websocket's safe codec, which forbids `unsafe`.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

pub mod frame_api;

use swoole_rs_websocket::mask;

/// XOR a payload with a four-byte key; zero length permits null pointers.
///
/// # Safety
/// For nonzero length, `data` must cover `length` exclusive writable bytes and
/// `key` four readable bytes. The key may overlap data: it is copied before any
/// mutable borrow. All other aliases must remain unused for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_ws_mask(data: *mut u8, length: usize, key: *const u8) -> i32 {
    if length == 0 {
        return 0;
    }
    if data.is_null() || key.is_null() || length > isize::MAX as usize {
        return -1;
    }
    // SAFETY: Copy all four key bytes before the mutable payload borrow so
    // overlap is valid. read_unaligned preserves byte-aligned key support.
    let key = unsafe { key.cast::<[u8; 4]>().read_unaligned() };
    // SAFETY: non-null, `length <= isize::MAX`; the caller lends `length` writable bytes and
    // the key was copied out first, so an overlapping key is fine.
    mask(unsafe { std::slice::from_raw_parts_mut(data, length) }, key);
    0
}
