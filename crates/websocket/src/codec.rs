//! Complete-frame codec and the pinned server's message/control policy.
//! Allocation, connection state, PHP and callbacks stay with the native caller.
use std::mem::MaybeUninit;

use crate::{LengthError, Opcode, frame_length, header, mask};

const FIN: u8 = 1;
const RSV1: u8 = 4;
const MASK: u8 = 32;
const HEADER_ONLY: u8 = 64;
const MASK_KEY: [u8; 4] = *b"258E";
#[derive(Clone, Copy, Debug, Default)]
pub struct DecodedFrame {
    pub payload_length: usize,
    pub header_length: usize,
    pub opcode: Opcode,
    pub flags: u8,
    pub length_code: u8,
    pub mask_key: [u8; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeErrorKind {
    IncompleteHeader,
    IncompleteBody,
    LengthOverflow,
    CompressedControl,
}

/// A rejected frame retains the header facts read before the error.
#[derive(Clone, Copy, Debug)]
pub struct DecodeError {
    pub kind: DecodeErrorKind,
    pub frame: DecodedFrame,
}

fn flags(first: u8, second: u8) -> u8 {
    ((first >> 7) & FIN)
        | ((first >> 4) & 4)
        | ((first >> 2) & 8)
        | (first & 16)
        | ((second >> 2) & MASK)
}

/// Decode one frame, unmasking only a complete payload. Trailing bytes are left
/// untouched.
///
/// # Errors
/// Incomplete input, an unrepresentable length, or a complete compressed control
/// frame. Errors retain the decoded header and leave the payload untouched.
#[inline]
pub fn decode_in_place(data: &mut [u8]) -> Result<DecodedFrame, DecodeError> {
    let length = data.len();
    let measured = frame_length(data);
    let header = measured.map_or_else(
        |error| error.header_length().unwrap_or(0),
        |frame| frame.header_length,
    );
    let mut frame = DecodedFrame {
        header_length: header,
        ..DecodedFrame::default()
    };
    if let [first, second, ..] = *data {
        frame.opcode = Opcode::from_code(first & 15);
        frame.flags = flags(first, second);
        frame.length_code = second & 127;
    }
    let size = measured
        .map_err(|error| DecodeError {
            kind: match error {
                LengthError::IncompleteHeader { .. } => DecodeErrorKind::IncompleteHeader,
                LengthError::Overflow { .. } => DecodeErrorKind::LengthOverflow,
            },
            frame,
        })?
        .total;
    frame.payload_length = size - frame.header_length;
    if length < size {
        return Err(DecodeError {
            kind: DecodeErrorKind::IncompleteBody,
            frame,
        });
    }
    if frame.opcode.is_control() && frame.flags & RSV1 != 0 {
        return Err(DecodeError {
            kind: DecodeErrorKind::CompressedControl,
            frame,
        });
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
    Ok(frame)
}

/// Close, ping and pong frames must be final and carry at most 125 bytes.
#[must_use]
pub fn control_invalid(opcode: u8, flags: u8, length: usize) -> bool {
    Opcode::from_code(opcode).is_control() && (flags & FIN == 0 || length > 125)
}

/// Why a frame could not be encoded. Rejected inputs leave the output untouched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    LengthOverflow,
    InvalidControl,
    PayloadLength,
    OutputTooSmall,
}

/// Actual encoded size, including the pinned header-only behavior.
///
/// # Errors
/// The encoded length cannot fit in a slice.
pub fn encoded_size(length: usize, flags: u8) -> Result<usize, EncodeError> {
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
        .ok_or(EncodeError::LengthOverflow)
}

/// One frame of a `length`-byte payload into `out`; `data`, the payload, is
/// unused for a header-only frame. Returns the number of initialized output bytes.
///
/// # Errors
/// An overflowing length, invalid control frame, mismatched payload length or
/// short output. Errors leave `out` untouched.
pub fn encode_parts(
    length: usize,
    data: &[u8],
    opcode: u8,
    flags: u8,
    out: &mut [MaybeUninit<u8>],
) -> Result<usize, EncodeError> {
    let size = encoded_size(length, flags)?;
    if control_invalid(opcode, flags, length) {
        return Err(EncodeError::InvalidControl);
    }
    if flags & HEADER_ONLY == 0 && length != data.len() {
        return Err(EncodeError::PayloadLength);
    }
    let (header, header_length) = header(length as u64, opcode, flags);
    let (head, rest) = out
        .get_mut(..size)
        .and_then(|out| out.split_at_mut_checked(header_length))
        .ok_or(EncodeError::OutputTooSmall)?;
    head.write_copy_of_slice(header.get(..header_length).unwrap_or_default());
    let payload = match rest.split_first_chunk_mut::<4>() {
        Some((key, payload)) if flags & MASK != 0 => {
            key.write_copy_of_slice(&MASK_KEY);
            payload
        }
        _ => rest,
    };
    // Not header-only: `size` left exactly the payload's bytes.
    if flags & HEADER_ONLY == 0 {
        let payload = payload.write_copy_of_slice(data);
        if flags & MASK != 0 {
            mask(payload, MASK_KEY);
        }
    }
    Ok(size)
}

/// One frame of `data` into `out`, with no allocation.
///
/// # Errors
/// As for [`encode_parts`].
pub fn encode_into(
    data: &[u8],
    opcode: u8,
    flags: u8,
    out: &mut [MaybeUninit<u8>],
) -> Result<usize, EncodeError> {
    encode_parts(data.len(), data, opcode, flags, out)
}

/// A close frame of `code` and `reason` into `out`, as [`encode_into`]. The
/// signed code truncates to 16 bits as upstream's; code plus reason is a
/// control payload, so reasons over 123 bytes are rejected.
///
/// # Errors
/// An invalid control payload or short output.
pub fn encode_close_into(
    code: i32,
    reason: &[u8],
    flags: u8,
    out: &mut [MaybeUninit<u8>],
) -> Result<usize, EncodeError> {
    let mut payload = [0u8; 127];
    let Some((code_bytes, reason_bytes)) = payload
        .get_mut(..reason.len() + 2)
        .and_then(<[u8]>::split_first_chunk_mut)
    else {
        return Err(EncodeError::InvalidControl);
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

#[derive(Clone, Copy, Debug, Default)]
pub struct Context {
    pub fragment_length: usize,
    pub package_max_length: usize,
    pub fragment_flags: u16,
    pub has_fragment: bool,
    pub closing: bool,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decision {
    pub offset: usize,
    pub length: usize,
    pub action: Action,
    pub error: PolicyError,
    pub ext_flags: u16,
}

#[inline]
fn decide(frame: DecodedFrame, wire: usize, context: Context) -> Decision {
    use Action::{
        Append, ClientClose, Control, Dispatch, Finish, Reject, ServerClose, Start, Unknown,
    };
    use Opcode::{Binary, Close, Continuation, Ping, Pong, Reserved, Text};
    use PolicyError as Error;
    let mut result = Decision {
        offset: wire - frame.payload_length,
        length: frame.payload_length,
        ext_flags: (u16::from(frame.opcode.code()) << 8) | u16::from(frame.flags),
        ..Decision::default()
    };
    // Keep upstream's wire-size limits and unusual control offsets verbatim.
    // These are compatibility policy, not RFC validation.
    let (action, error) = match frame.opcode {
        Continuation if !context.has_fragment => (Reject, Error::Orphan),
        Continuation
            if context
                .fragment_length
                .checked_add(frame.payload_length)
                .is_none_or(|total| total > context.package_max_length) =>
        {
            (Reject, Error::TooBig)
        }
        Continuation if frame.flags & FIN != 0 => {
            result.ext_flags = context.fragment_flags | u16::from(FIN);
            (Finish, Error::None)
        }
        Continuation => (Append, Error::None),
        Text | Binary if frame.flags & FIN == 0 && context.has_fragment => (Reject, Error::Overlap),
        Text | Binary if frame.flags & FIN == 0 => (Start, Error::None),
        Text | Binary => (Dispatch, Error::None),
        Ping | Pong if wire >= 127 => (Reject, Error::ControlTooBig),
        Ping | Pong => {
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
        Close if wire - 2 > 125 => (Reject, Error::CloseTooBig),
        Close if context.closing => (ServerClose, Error::None),
        Close => (ClientClose, Error::None),
        Reserved(_) => (Unknown, Error::None),
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

/// Normalize outgoing PHP frame flags and choose compression: the normalized flags,
/// and whether to compress with native zlib. The caller still owns zlib, its
/// buffer, warnings and PHP conversions.
#[must_use]
pub fn pack_policy(opcode: u8, flags: u8, length: usize) -> (u8, bool) {
    const COMPRESS: u8 = 2;
    match Opcode::from_code(opcode) {
        // The pinned sw_unset_bit macro lacks parentheses: its compound
        // COMPRESS | RSV1 argument clears only COMPRESS. Preserve RSV1.
        opcode if opcode.is_control() => ((flags & !COMPRESS) | FIN, false),
        Opcode::Continuation => (flags, false),
        _ => (
            flags,
            flags & (COMPRESS | FIN) == (COMPRESS | FIN) && length != 0,
        ),
    }
}

/// What happens to an incoming PHP message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// To the `onMessage` callback.
    Deliver,
    Discard,
    /// Answer with an automatic pong, then discard.
    Pong,
}

/// Choose native delivery for an incoming PHP message. Settings bits 0, 1 and 2
/// enable close, ping and pong callbacks respectively. No state is retained.
#[must_use]
pub fn message_policy(opcode: u8, settings: u8) -> Delivery {
    match Opcode::from_code(opcode) {
        Opcode::Close if settings & 1 == 0 => Delivery::Discard,
        Opcode::Ping if settings & 2 == 0 => Delivery::Pong,
        Opcode::Pong if settings & 4 == 0 => Delivery::Discard,
        _ => Delivery::Deliver,
    }
}
