//! Allocation-free WebSocket codec and message/control policy. Native callers
//! retain storage, connection ownership, PHP dispatch and socket I/O; their C
//! ABI is crates/websocket-ffi.
#![forbid(unsafe_code)]
// The fuzz image's nightly predates `MaybeUninit::write_copy_of_slice` going stable.
#![cfg_attr(fuzzing, feature(maybe_uninit_write_slice))]

mod codec;
pub mod sha1;
pub use codec::*;

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

/// A frame's header plus payload length over the `bytes` received so far: 0 while
/// the header is incomplete, -1 when it overflows `ssize_t`. The header length is
/// reported as soon as the bytes read so far settle it (`None` under two bytes),
/// as upstream's incremental output.
#[inline]
#[must_use]
pub fn frame_length(bytes: &[u8]) -> (i64, Option<u32>) {
    let [_, second, ref extended @ ..] = *bytes else {
        return (0, None);
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
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a header is at most 14 bytes"
    )]
    let reported = Some(header as u32);
    // Subtract before adding: even a positive payload can overflow ssize_t once
    // the header is included. Upstream's signed addition is undefined there.
    if bytes.len() < header {
        (0, reported)
    } else if payload > (isize::MAX as u64) - header as u64 {
        (-1, reported)
    } else {
        (
            i64::try_from(payload + header as u64).unwrap_or(-1),
            reported,
        )
    }
}

/// A wire header without a mask key or payload, and its length.
#[must_use]
pub fn header(payload_length: u64, opcode: u8, flags: u8) -> ([u8; 10], usize) {
    let mut header = [0u8; 10];
    header[0] = (opcode & 0xf) | (u8::from(flags & 1 != 0) << 7) | (u8::from(flags & 4 != 0) << 6);
    header[1] = u8::from(flags & 32 != 0) << 7;
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
