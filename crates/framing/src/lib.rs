//! The length/EOF receive loops, packet-length helpers and the MIME table;
//! native code keeps the socket IO, callbacks and buffer allocation.
#![expect(
    clippy::missing_safety_doc,
    reason = "every export is covered by the module-local C contract"
)]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
use std::slice;
pub mod mime;

pub mod recv;
pub use recv::{Protocol, swr_v1_framing_recv};

unsafe fn bytes<'a>(p: *const u8, n: usize) -> Option<&'a [u8]> {
    if n == 0 {
        Some(&[])
    } else if p.is_null() || n > isize::MAX as usize {
        None
    } else {
        // SAFETY: `p` is non-null and the caller lends `n <= isize::MAX` initialized bytes.
        Some(unsafe { slice::from_raw_parts(p, n) })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_framing_width(format: u8) -> usize {
    match format {
        b'c' | b'C' => 1,
        b's' | b'S' | b'n' | b'v' => 2,
        b'i' | b'I' | b'l' | b'L' | b'N' | b'V' => 4,
        b'q' | b'Q' | b'J' | b'P' => 8,
        _ => 0,
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_framing_unpack(
    format: u8,
    data: *const u8,
    len: usize,
    out: *mut i64,
) -> i32 {
    let width = swr_v1_framing_width(format);
    if width == 0 {
        return -1;
    }
    // SAFETY: the caller lends `len` initialized bytes at `data`.
    let v = unsafe { bytes(data, len) };
    // SAFETY: the caller passes null or a writable i64.
    let out = unsafe { out.as_mut() };
    let (Some(v), Some(out)) = (v, out) else {
        return -2;
    };
    let Some(field) = v.get(..width) else {
        return -2;
    };
    let mut raw = [0u8; 8];
    for (slot, &byte) in raw.iter_mut().zip(field) {
        *slot = byte;
    }
    *out = match format {
        b'c' => i64::from(raw[0].cast_signed()),
        b'C' => i64::from(raw[0]),
        b's' => i64::from(i16::from_ne_bytes([raw[0], raw[1]])),
        b'S' => i64::from(u16::from_ne_bytes([raw[0], raw[1]])),
        b'n' => i64::from(u16::from_be_bytes([raw[0], raw[1]])),
        b'v' => i64::from(u16::from_le_bytes([raw[0], raw[1]])),
        b'i' | b'l' => i64::from(i32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]])),
        b'I' | b'L' => i64::from(u32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]])),
        b'N' => i64::from(u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])),
        b'V' => i64::from(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])),
        b'q' | b'Q' => i64::from_ne_bytes(raw),
        b'J' => u64::from_be_bytes(raw).cast_signed(),
        b'P' => u64::from_le_bytes(raw).cast_signed(),
        _ => return -1,
    };
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_framing_hton64(value: u64) -> u64 {
    value.to_be()
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_framing_default(
    format: u8,
    data: *const u8,
    len: usize,
    offset: u32,
    size: u32,
    body_offset: u32,
    out: *mut i64,
) -> i32 {
    if size == 0 {
        return -1;
    }
    // SAFETY: the caller passes null or a writable i64.
    let Some(out) = (unsafe { out.as_mut() }) else {
        return -1;
    };
    let need = offset as usize + size as usize;
    if len < need {
        *out = i64::try_from(need).unwrap_or(i64::MAX);
        return 0;
    }
    // SAFETY: the caller lends `len` initialized bytes at `data`.
    let Some(v) = (unsafe { bytes(data, len) }) else {
        return -1;
    };
    let width = swr_v1_framing_width(format);
    if width == 0 {
        return -2;
    }
    let Some(field) = v
        .get(offset as usize..)
        .filter(|field| width <= field.len())
    else {
        return -1;
    };
    let mut value = 0;
    // SAFETY: `width <= field.len()`, so `width` bytes from `field` lie inside `v`.
    let result = unsafe { swr_v1_framing_unpack(format, field.as_ptr(), width, &raw mut value) };
    if result != 0 || value < 0 {
        return -1;
    }
    let Some(total) = value.checked_add(i64::from(body_offset)) else {
        return -1;
    };
    *out = total;
    1
}
