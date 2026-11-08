//! Complete-frame codec and the pinned server's message/control policy.
//! Allocation, connection state, PHP and callbacks stay with the native caller.
use crate::{swr_v1_ws_frame_length, swr_v1_ws_mask, swr_v1_ws_write_header};

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
            // SAFETY: `payload` is exclusive and the key is a local copy.
            unsafe { swr_v1_ws_mask(payload.as_mut_ptr(), payload.len(), key.as_ptr()) };
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

/// Encode into native-owned storage with no allocation; errors leave it intact.
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
    let size = swr_v1_ws_encoded_size(length, flags);
    if size == 0
        || swr_v1_ws_control_invalid(opcode, flags, length)
        || output.is_null()
        || capacity < size
        || capacity > isize::MAX as usize
        || (data.is_null() && length != 0 && flags & HEADER_ONLY == 0)
    {
        return 0;
    }
    // SAFETY: All ranges/capacities validated before the first write.
    let mut position =
        unsafe { swr_v1_ws_write_header(length as u64, opcode, flags, output, capacity) };
    // `capacity >= size` (checked above), and `size` covers header, key and payload.
    if flags & MASK != 0 {
        // SAFETY: the key's 4 bytes follow the header, inside `size`.
        let key = unsafe { output.add(position) };
        // SAFETY: `MASK_KEY` is a 4-byte static, disjoint from the caller's output.
        unsafe { std::ptr::copy_nonoverlapping(MASK_KEY.as_ptr(), key, 4) };
        position += 4;
    }
    if flags & HEADER_ONLY == 0 && length != 0 {
        // SAFETY: the payload's `length` bytes follow the header and key, inside `size`.
        let payload = unsafe { output.add(position) };
        // SAFETY: `data` lends `length` readable bytes, disjoint from the output.
        unsafe { std::ptr::copy_nonoverlapping(data, payload, length) };
        if flags & MASK != 0 {
            // SAFETY: `payload` holds `length` writable bytes just copied; the key is static.
            unsafe { swr_v1_ws_mask(payload, length, MASK_KEY.as_ptr()) };
        }
    }
    size
}

/// Pack close code/reason, preserving truncation of the signed code to 16 bits.
/// Code plus reason is a control payload, so reasons over 123 bytes are rejected.
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
    if length > 125 || (reason.is_null() && length != 0) {
        return 0;
    }
    let mut payload = [0u8; 127];
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream packs the close code as uint16_t"
    )]
    let code = code as u16;
    let Some((code_bytes, reason_bytes)) = payload.split_first_chunk_mut() else {
        return 0;
    };
    *code_bytes = code.to_be_bytes();
    if length != 0
        && let Some(reason_bytes) = reason_bytes.get_mut(..length)
    {
        // SAFETY: Reason is borrowed and bounded by 125 bytes.
        reason_bytes.copy_from_slice(unsafe { std::slice::from_raw_parts(reason, length) });
    }
    // SAFETY: `payload` is a local holding `length + 2 <= 127` bytes; the caller lends
    // `capacity` writable output bytes, disjoint from it.
    unsafe {
        swr_v1_ws_encode(
            payload.as_ptr(),
            length + 2,
            8,
            flags | FIN,
            output,
            capacity,
        )
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

/// Decide protocol actions without callbacks, allocation or retained state.
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
    if frame.is_null() || context.is_null() || output.is_null() {
        return -1;
    }
    // SAFETY: non-null (checked above); the caller lends a valid, aligned frame.
    let frame = unsafe { frame.read() };
    if frame.header_length < if frame.flags & MASK != 0 { 6 } else { 2 }
        || frame.header_length > wire_length
        || frame.payload_length > wire_length - frame.header_length
    {
        return -1;
    }
    // SAFETY: non-null (checked above); the caller lends a valid, aligned context.
    let context = unsafe { context.read() };
    // SAFETY: non-null (checked above); the caller lends an aligned, exclusive output.
    unsafe { output.write(decide(frame, wire_length, context)) };
    0
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
