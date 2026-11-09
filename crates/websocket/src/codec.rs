//! Complete-frame codec and the pinned server's message/control policy.
//! Allocation, connection state, PHP and callbacks stay with the native caller.
use std::mem::MaybeUninit;

use crate::{header, mask, swr_v1_ws_frame_length};

const FIN: u8 = 1;
const RSV1: u8 = 4;
const MASK: u8 = 32;
const HEADER_ONLY: u8 = 64;
const MASK_KEY: [u8; 4] = *b"258E";
/// `swr_v1_ws_decode` result for a complete compressed (RSV1) control frame.
pub const INVALID_CONTROL: i64 = -2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DecodedFrame {
    pub payload_length: usize,
    pub header_length: usize,
    pub opcode: u8,
    pub flags: u8,
    pub length_code: u8,
    pub mask_key: [u8; 4],
}

fn flags(first: u8, second: u8) -> u8 {
    ((first >> 7) & FIN)
        | ((first >> 4) & 4)
        | ((first >> 2) & 8)
        | (first & 16)
        | ((second >> 2) & MASK)
}

/// Decode one frame, unmasking only a complete payload. Trailing bytes are left
/// untouched. Returns the expected frame length even when its body is incomplete,
/// or `INVALID_CONTROL` (payload left masked) for a complete compressed control
/// frame, with the decoded header.
pub fn decode_in_place(data: &mut [u8]) -> (i64, DecodedFrame) {
    let length = data.len();
    let mut header = 0;
    // The native Frame decoder passes header availability through uint32_t.
    // Preserve that conversion even for unusually large borrowed buffers.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the native uint32_t header availability"
    )]
    let available = length as u32 as usize;
    // SAFETY: `available <= length` readable bytes; header is a disjoint local.
    let total = unsafe { swr_v1_ws_frame_length(data.as_ptr(), available, &raw mut header) };
    let mut frame = DecodedFrame {
        header_length: header as usize,
        ..DecodedFrame::default()
    };
    if let [first, second, ..] = *data {
        frame.opcode = first & 15;
        frame.flags = flags(first, second);
        frame.length_code = second & 127;
    }
    if let Some(size) = usize::try_from(total).ok().filter(|&size| size > 0) {
        frame.payload_length = size - frame.header_length;
        if length >= size && is_control(frame.opcode) && frame.flags & RSV1 != 0 {
            return (INVALID_CONTROL, frame);
        }
        let at = frame.header_length;
        if frame.payload_length != 0
            && frame.flags & MASK != 0
            && let Some(key) = data.get(at.wrapping_sub(4)..at)
            && let Ok(key) = <[u8; 4]>::try_from(key)
            && let Some(payload) = data.get_mut(at..size)
        {
            frame.mask_key = key;
            mask(payload, key);
        }
    }
    (total, frame)
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
    output: *mut DecodedFrame,
) -> i64 {
    if output.is_null() || length > isize::MAX as usize || (data.is_null() && length != 0) {
        return -1;
    }
    let data = if length == 0 {
        &mut []
    } else {
        // SAFETY: as the caller promises, checked above.
        unsafe { core::slice::from_raw_parts_mut(data, length) }
    };
    let (total, frame) = decode_in_place(data);
    // SAFETY: The caller supplies a disjoint, aligned output.
    unsafe { output.write(frame) };
    total
}

fn is_control(opcode: u8) -> bool {
    matches!(opcode, 8..=10)
}

/// Close, ping and pong frames must be final and carry at most 125 bytes.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ws_control_invalid(opcode: u8, flags: u8, length: usize) -> bool {
    is_control(opcode) && (flags & FIN == 0 || length > 125)
}

/// Actual encoded size, including the pinned header-only behavior.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ws_encoded_size(length: usize, flags: u8) -> usize {
    let header: usize = if length < 126 {
        2
    } else if length <= 65535 {
        4
    } else {
        10
    };
    let payload = if flags & HEADER_ONLY != 0 { 0 } else { length };
    header
        .checked_add(if flags & MASK != 0 { 4 } else { 0 })
        .and_then(|size| size.checked_add(payload))
        .filter(|size| isize::try_from(*size).is_ok())
        .unwrap_or(0)
}

/// One frame of a `length`-byte payload into `out`; `data`, the payload, is
/// unused for a header-only frame. Returns the encoded size, or 0 without
/// writes when `out` is too short or the frame is invalid.
fn encode_parts(
    length: usize,
    data: &[u8],
    opcode: u8,
    flags: u8,
    out: &mut [MaybeUninit<u8>],
) -> usize {
    let size = swr_v1_ws_encoded_size(length, flags);
    if size == 0 || swr_v1_ws_control_invalid(opcode, flags, length) {
        return 0;
    }
    let (header, header_length) = header(length as u64, opcode, flags);
    let Some((head, rest)) = out
        .get_mut(..size)
        .and_then(|out| out.split_at_mut_checked(header_length))
    else {
        return 0;
    };
    head.write_copy_of_slice(header.get(..header_length).unwrap_or_default());
    let payload = match rest.split_first_chunk_mut::<4>() {
        Some((key, payload)) if flags & MASK != 0 => {
            key.write_copy_of_slice(&MASK_KEY);
            payload
        }
        _ => rest,
    };
    // Not header-only: `size` left exactly the payload's bytes.
    if flags & HEADER_ONLY == 0 && payload.len() == data.len() {
        let payload = payload.write_copy_of_slice(data);
        if flags & MASK != 0 {
            mask(payload, MASK_KEY);
        }
    }
    size
}

/// One frame of `data` into `out`, with no allocation; the encoded size, or 0
/// without writes when `out` is too short or the frame is invalid.
pub fn encode_into(data: &[u8], opcode: u8, flags: u8, out: &mut [MaybeUninit<u8>]) -> usize {
    encode_parts(data.len(), data, opcode, flags, out)
}

/// A close frame of `code` and `reason` into `out`, as [`encode_into`]. The
/// signed code truncates to 16 bits as upstream's; code plus reason is a
/// control payload, so reasons over 123 bytes are rejected.
pub fn encode_close_into(
    code: i32,
    reason: &[u8],
    flags: u8,
    out: &mut [MaybeUninit<u8>],
) -> usize {
    let mut payload = [0u8; 127];
    let Some((code_bytes, reason_bytes)) = payload
        .get_mut(..reason.len() + 2)
        .and_then(<[u8]>::split_first_chunk_mut)
    else {
        return 0;
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream packs the close code as uint16_t"
    )]
    let code = code as u16;
    *code_bytes = code.to_be_bytes();
    reason_bytes.copy_from_slice(reason);
    let payload = payload.get(..reason.len() + 2).unwrap_or_default();
    encode_into(payload, 8, flags | FIN, out)
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

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Context {
    pub fragment_length: usize,
    pub package_max_length: usize,
    pub fragment_flags: u16,
    pub has_fragment: u8,
    pub closing: u8,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Action {
    #[default]
    Reject,
    Dispatch,
    Start,
    Append,
    Finish,
    Control,
    ClientClose,
    ServerClose,
    Unknown,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PolicyError {
    #[default]
    None,
    Orphan,
    TooBig,
    Overlap,
    ControlTooBig,
    CloseTooBig,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decision {
    pub offset: usize,
    pub length: usize,
    pub action: Action,
    pub error: PolicyError,
    pub ext_flags: u16,
}

fn decide(frame: DecodedFrame, wire: usize, context: Context) -> Decision {
    use Action::{
        Append, ClientClose, Control, Dispatch, Finish, Reject, ServerClose, Start, Unknown,
    };
    use PolicyError as Error;
    let mut result = Decision {
        offset: wire - frame.payload_length,
        length: frame.payload_length,
        ext_flags: u16::from(frame.opcode) << 8 | u16::from(frame.flags),
        ..Decision::default()
    };
    // Keep upstream's wire-size limits and unusual control offsets verbatim.
    // These are compatibility policy, not RFC validation.
    let (action, error) = match frame.opcode {
        0 if context.has_fragment == 0 => (Reject, Error::Orphan),
        0 if context
            .fragment_length
            .checked_add(frame.payload_length)
            .is_none_or(|total| total > context.package_max_length) =>
        {
            (Reject, Error::TooBig)
        }
        0 if frame.flags & FIN != 0 => {
            result.ext_flags = context.fragment_flags | u16::from(FIN);
            (Finish, Error::None)
        }
        0 => (Append, Error::None),
        1 | 2 if frame.flags & FIN == 0 && context.has_fragment != 0 => (Reject, Error::Overlap),
        1 | 2 if frame.flags & FIN == 0 => (Start, Error::None),
        1 | 2 => (Dispatch, Error::None),
        9 | 10 if wire >= 127 => (Reject, Error::ControlTooBig),
        9 | 10 => {
            result.offset = if wire == 2 {
                2
            } else if frame.flags & MASK != 0 {
                6
            } else {
                2
            };
            result.length = wire - result.offset;
            (Control, Error::None)
        }
        8 if wire - 2 > 125 => (Reject, Error::CloseTooBig),
        8 if context.closing != 0 => (ServerClose, Error::None),
        8 => (ClientClose, Error::None),
        _ => (Unknown, Error::None),
    };
    result.action = action;
    result.error = error;
    result
}

/// Decide protocol actions without callbacks, allocation or retained state;
/// `None` when the frame's lengths don't fit the wire's.
#[inline]
#[must_use]
pub fn decide_frame(frame: &DecodedFrame, wire: usize, context: &Context) -> Option<Decision> {
    if frame.header_length < if frame.flags & MASK != 0 { 6 } else { 2 }
        || frame.header_length > wire
        || frame.payload_length > wire - frame.header_length
    {
        return None;
    }
    Some(decide(*frame, wire, *context))
}

/// [`decide_frame`] over C pointers.
///
/// # Safety
/// Inputs are valid, aligned readable values; output is aligned, exclusive and
/// writable, disjoint from inputs. All are borrowed for this call only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_ws_decide(
    frame: *const DecodedFrame,
    wire_length: usize,
    context: *const Context,
    output: *mut Decision,
) -> i32 {
    // SAFETY: null or, per the caller, valid and aligned.
    let frame = unsafe { frame.as_ref() };
    // SAFETY: as above.
    let context = unsafe { context.as_ref() };
    let (Some(frame), Some(context)) = (frame, context) else {
        return -1;
    };
    match decide_frame(frame, wire_length, context) {
        Some(decision) if !output.is_null() => {
            // SAFETY: non-null; the caller lends an aligned, exclusive output.
            unsafe { output.write(decision) };
            0
        }
        _ => -1,
    }
}

/// Normalize outgoing PHP frame flags and choose compression. The low byte is
/// the normalized flags; bit 8 requests native zlib compression. The caller
/// still owns zlib, its buffer, warnings and PHP conversions.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ws_pack_policy(opcode: u8, flags: u8, length: usize) -> u16 {
    const COMPRESS: u8 = 2;
    let (flags, compress) = match opcode {
        // The pinned sw_unset_bit macro lacks parentheses: its compound
        // COMPRESS | RSV1 argument clears only COMPRESS. Preserve RSV1.
        8..=10 => ((flags & !COMPRESS) | FIN, false),
        0 => (flags, false),
        _ => (
            flags,
            flags & (COMPRESS | FIN) == (COMPRESS | FIN) && length != 0,
        ),
    };
    u16::from(flags) | (u16::from(compress) << 8)
}

/// Choose native delivery for an incoming PHP message. Settings bits 0, 1 and 2
/// enable close, ping and pong callbacks respectively. Return 0 to deliver, 1
/// to discard, or 2 to send an automatic pong and discard. No state is retained.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ws_message_policy(opcode: u8, settings: u8) -> u8 {
    match opcode {
        8 if settings & 1 == 0 => 1,
        9 if settings & 2 == 0 => 2,
        10 if settings & 4 == 0 => 1,
        _ => 0,
    }
}
