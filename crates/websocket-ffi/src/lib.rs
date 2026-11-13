//! The WebSocket codec's C ABI (`native/websocket/swoole_rs_websocket_codec.h`)
//! and `src/protocol/websocket.cc`'s entry points
//! ([`frame_api`]) over crates/websocket's safe codec, which forbids `unsafe`.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

pub mod frame_api;

use std::mem::MaybeUninit;

use swoole_rs_websocket::{
    Context, Decision, DecodedFrame, control_invalid, decide_frame, decode_in_place,
    encode_close_into, encode_parts, encoded_size, mask, message_policy, pack_policy,
};

const HEADER_ONLY: u8 = 64;

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

/// [`decode_in_place`] over a C buffer.
///
/// # Safety
/// Input covers `length` exclusive writable bytes, null only at length zero.
/// Output is aligned, exclusive, writable and disjoint from the input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_ws_decode(
    data: *mut u8,
    length: usize,
    output: Option<&mut DecodedFrame>,
) -> i64 {
    let Some(output) = output else {
        return -1;
    };
    if length > isize::MAX as usize || (data.is_null() && length != 0) {
        return -1;
    }
    let data = if length == 0 {
        &mut []
    } else {
        // SAFETY: as the caller promises, checked above.
        unsafe { core::slice::from_raw_parts_mut(data, length) }
    };
    let (total, frame) = decode_in_place(data);
    *output = frame;
    total
}

/// Close, ping and pong frames must be final and carry at most 125 bytes.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ws_control_invalid(opcode: u8, flags: u8, length: usize) -> bool {
    control_invalid(opcode, flags, length)
}

/// Actual encoded size, including the pinned header-only behavior.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ws_encoded_size(length: usize, flags: u8) -> usize {
    encoded_size(length, flags)
}

/// The `length` bytes at `data` a C caller lends (null only at length zero),
/// and its `capacity`-byte output, or `None` when either is invalid.
///
/// # Safety
/// As `swr_v1_ws_encode`, with `data` read only when `read`.
unsafe fn borrowed<'a>(
    data: *const u8,
    length: usize,
    read: bool,
    output: *mut u8,
    capacity: usize,
) -> Option<(&'a [u8], &'a mut [MaybeUninit<u8>])> {
    if output.is_null() || capacity > isize::MAX as usize {
        return None;
    }
    let data = if !read || length == 0 {
        &[]
    } else if data.is_null() || length > isize::MAX as usize {
        return None;
    } else {
        // SAFETY: the caller lends `length` readable bytes.
        unsafe { std::slice::from_raw_parts(data, length) }
    };
    // SAFETY: the caller lends `capacity` exclusive writable bytes, disjoint from `data`.
    let output = unsafe { std::slice::from_raw_parts_mut(output.cast(), capacity) };
    Some((data, output))
}

/// [`encode_into`] over native-owned storage; errors leave it intact.
///
/// # Safety
/// Input covers `length` readable bytes (null permitted for empty/header-only
/// input), output covers `capacity` exclusive writable bytes. Ranges are disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_ws_encode(
    data: *const u8,
    length: usize,
    opcode: u8,
    flags: u8,
    output: *mut u8,
    capacity: usize,
) -> usize {
    // SAFETY: as the caller promises.
    match unsafe { borrowed(data, length, flags & HEADER_ONLY == 0, output, capacity) } {
        Some((data, output)) => encode_parts(length, data, opcode, flags, output),
        None => 0,
    }
}

/// [`encode_close_into`] over native-owned storage.
///
/// # Safety
/// Same disjoint borrowed ranges as `swr_v1_ws_encode`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_ws_encode_close(
    code: i32,
    reason: *const u8,
    length: usize,
    flags: u8,
    output: *mut u8,
    capacity: usize,
) -> usize {
    // SAFETY: as the caller promises.
    match unsafe { borrowed(reason, length, true, output, capacity) } {
        Some((reason, output)) => encode_close_into(code, reason, flags, output),
        None => 0,
    }
}

/// [`decide_frame`] over C pointers.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ws_decide(
    frame: Option<&DecodedFrame>,
    wire_length: usize,
    context: Option<&Context>,
    output: Option<&mut Decision>,
) -> i32 {
    let (Some(frame), Some(context)) = (frame, context) else {
        return -1;
    };
    match (decide_frame(frame, wire_length, context), output) {
        (Some(decision), Some(output)) => {
            *output = decision;
            0
        }
        _ => -1,
    }
}

/// [`pack_policy`] for C.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ws_pack_policy(opcode: u8, flags: u8, length: usize) -> u16 {
    pack_policy(opcode, flags, length)
}

/// [`message_policy`] for C.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ws_message_policy(opcode: u8, settings: u8) -> u8 {
    message_policy(opcode, settings)
}
