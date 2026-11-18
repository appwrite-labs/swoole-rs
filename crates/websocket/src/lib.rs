//! Allocation-free WebSocket codec and message/control policy. Native callers
//! retain storage, connection ownership, PHP dispatch and socket I/O; crates/ports'
//! `frame_api` adapts it to upstream's records.
#![forbid(unsafe_code)]
// The fuzz image's nightly predates `MaybeUninit::write_copy_of_slice` going stable.
#![cfg_attr(fuzzing, feature(maybe_uninit_write_slice))]

mod codec;
pub mod sha1;
pub use codec::*;

/// A frame opcode, including reserved values carried through native boundaries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Opcode {
    #[default]
    Continuation,
    Text,
    Binary,
    Close,
    Ping,
    Pong,
    Reserved(u8),
}

impl Opcode {
    /// Interpret a raw opcode without discarding reserved values.
    #[must_use]
    pub const fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Continuation,
            1 => Self::Text,
            2 => Self::Binary,
            8 => Self::Close,
            9 => Self::Ping,
            10 => Self::Pong,
            other => Self::Reserved(other),
        }
    }

    /// The opcode's original code.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Continuation => 0,
            Self::Text => 1,
            Self::Binary => 2,
            Self::Close => 8,
            Self::Ping => 9,
            Self::Pong => 10,
            Self::Reserved(code) => code,
        }
    }

    /// Close, ping or pong.
    #[must_use]
    pub const fn is_control(self) -> bool {
        matches!(self, Self::Close | Self::Ping | Self::Pong)
    }
}

/// RFC 6455's handshake GUID (`SW_WEBSOCKET_GUID`).
const GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// `Sec-WebSocket-Accept` for a `Sec-WebSocket-Key`: base64(SHA-1(key + GUID)),
/// as the server handshake computes it (every byte of the key, NULs included).
#[must_use]
pub fn accept(key: &[u8]) -> [u8; 28] {
    let mut input = Vec::with_capacity(key.len() + GUID.len());
    input.extend_from_slice(key);
    input.extend_from_slice(GUID);
    let digest = sha1::digest(&input);
    // 4 * ceil(20 / 3) = 28 characters and the encoder's trailing NUL.
    let mut encoded = [0u8; 29];
    swoole_rs_base64::encode(&digest, &mut encoded);
    let mut accept = [0u8; 28];
    accept.copy_from_slice(&encoded[..28]);
    accept
}

/// A complete frame header's sizes, whether or not its payload has arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameLength {
    pub total: usize,
    pub header_length: usize,
}

/// Measuring a header preserves its incremental length even when it is incomplete
/// or its payload cannot fit in a Rust slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LengthError {
    IncompleteHeader { header_length: Option<usize> },
    Overflow { header_length: usize },
}

impl LengthError {
    /// The header length, once at least its first two bytes have arrived.
    #[must_use]
    pub const fn header_length(self) -> Option<usize> {
        match self {
            Self::IncompleteHeader { header_length } => header_length,
            Self::Overflow { header_length } => Some(header_length),
        }
    }
}

/// Measure a frame's header and payload, retaining incremental header progress.
///
/// # Errors
/// The header is incomplete or the total exceeds Rust's maximum slice length.
#[inline]
pub fn frame_length(bytes: &[u8]) -> Result<FrameLength, LengthError> {
    let [_, second, ref extended @ ..] = *bytes else {
        return Err(LengthError::IncompleteHeader {
            header_length: None,
        });
    };
    let (mut header, mut payload) = match second & 0x7f {
        126 => (4, 0),
        127 => (10, 0),
        short => (2, u64::from(short)),
    };
    if header == 4
        && let Some(&extended) = extended.first_chunk()
    {
        payload = u64::from(u16::from_be_bytes(extended));
    } else if header == 10
        && let Some(&extended) = extended.first_chunk()
    {
        payload = u64::from_be_bytes(extended);
    }
    if bytes.len() >= header && second & 0x80 != 0 {
        header += 4;
    }
    if bytes.len() < header {
        return Err(LengthError::IncompleteHeader {
            header_length: Some(header),
        });
    }
    let total = usize::try_from(payload)
        .ok()
        .and_then(|payload| payload.checked_add(header))
        .filter(|total| isize::try_from(*total).is_ok())
        .ok_or(LengthError::Overflow {
            header_length: header,
        })?;
    Ok(FrameLength {
        total,
        header_length: header,
    })
}

/// A wire header without a mask key or payload, and its length.
#[must_use]
pub fn header(payload_length: u64, opcode: Opcode, flags: FrameFlags) -> ([u8; 10], usize) {
    let mut header = [0u8; 10];
    header[0] = (opcode.code() & 0xf)
        | (u8::from(flags.is_final()) << 7)
        | (u8::from(flags.is_compressed()) << 6);
    header[1] = u8::from(flags.is_masked()) << 7;
    let size = if let Ok(short) = u8::try_from(payload_length)
        && short < 126
    {
        header[1] |= short;
        2
    } else if let Ok(short) = u16::try_from(payload_length) {
        header[1] |= 126;
        header[2..4].copy_from_slice(&short.to_be_bytes());
        4
    } else {
        header[1] |= 127;
        header[2..10].copy_from_slice(&payload_length.to_be_bytes());
        10
    };
    (header, size)
}

/// XOR a payload with a four-byte key.
pub fn mask(payload: &mut [u8], key: [u8; 4]) {
    let word = u64::from(u32::from_ne_bytes(key));
    let word_mask = word | (word << 32);
    let (words, tail) = payload.as_chunks_mut::<8>();
    for bytes in words {
        *bytes = (u64::from_ne_bytes(*bytes) ^ word_mask).to_ne_bytes();
    }
    // Each complete word consumes two whole keys, so the tail restarts at 0.
    for (byte, key) in tail.iter_mut().zip(key.iter().cycle()) {
        *byte ^= key;
    }
}

#[cfg(test)]
mod handshake_tests {
    #[test]
    fn accept_matches_rfc_6455() {
        // RFC 6455 section 1.3's example.
        assert_eq!(
            &super::accept(b"dGhlIHNhbXBsZSBub25jZQ=="),
            b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=",
            "the RFC's accept"
        );
    }
}

#[cfg(kani)]
#[path = "../../../tests/modules/websocket/kani.rs"]
mod proofs;
