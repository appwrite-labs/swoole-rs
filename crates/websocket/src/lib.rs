//! Allocation-free WebSocket codec and message/control policy. Native callers
//! retain storage, connection ownership, PHP dispatch and socket I/O.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

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
    // SAFETY: 20 readable bytes and a disjoint 29-byte output, as the encoder needs.
    unsafe {
        swoole_rs_base64::swr_v1_base64_encode(digest.as_ptr(), digest.len(), encoded.as_mut_ptr())
    };
    let mut accept = [0u8; 28];
    accept.copy_from_slice(&encoded[..28]);
    accept
}

/// Query header plus payload length, preserving upstream's incremental output.
///
/// # Safety
/// `data` must cover `length` readable bytes (null is allowed only at length 0).
/// `header_length` must be a valid, aligned, exclusive writable `u32`, disjoint
/// from the input. Pointers are borrowed only for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_ws_frame_length(
    data: *const u8,
    length: usize,
    header_length: *mut u32,
) -> i64 {
    if header_length.is_null() || length > isize::MAX as usize || (data.is_null() && length != 0) {
        return -1;
    }
    if length < 2 {
        return 0;
    }
    // SAFETY: The caller supplies the readable input and exclusive output.
    let bytes = unsafe { std::slice::from_raw_parts(data, length) };
    let [_, second, ref extended @ ..] = *bytes else {
        return 0;
    };
    let mut header = 2;
    let mut payload = u64::from(second & 0x7f);
    if payload == 126 {
        header = 4;
    } else if payload == 127 {
        header = 10;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a header is at most 14 bytes"
    )]
    let reported = header as u32;
    // SAFETY: the caller lends an aligned, writable `header_length`.
    unsafe { header_length.write(reported) };
    if length < header {
        return 0;
    }
    if header == 4
        && let Some(&extended) = extended.first_chunk()
    {
        payload = u64::from(u16::from_be_bytes(extended));
    } else if header == 10
        && let Some(&extended) = extended.first_chunk()
    {
        payload = u64::from_be_bytes(extended);
    }
    if second & 0x80 != 0 {
        header += 4;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a header is at most 14 bytes"
        )]
        let reported = header as u32;
        // SAFETY: as above.
        unsafe { header_length.write(reported) };
        if length < header {
            return 0;
        }
    }
    // Subtract before adding: even a positive payload can overflow ssize_t once
    // the header is included. Upstream's signed addition is undefined there.
    if payload > (isize::MAX as u64) - header as u64 {
        return -1;
    }
    i64::try_from(payload + header as u64).unwrap_or(-1)
}

/// Write a wire header without a mask key or payload; return 0 without writes
/// when the output is null, oversized, or too short.
///
/// # Safety
/// A non-null `output` must cover `capacity` exclusive writable bytes, borrowed
/// only for this call. No alignment beyond byte alignment is required.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_ws_write_header(
    payload_length: u64,
    opcode: u8,
    flags: u8,
    output: *mut u8,
    capacity: usize,
) -> usize {
    let size = if payload_length < 126 {
        2
    } else if u16::try_from(payload_length).is_ok() {
        4
    } else {
        10
    };
    if output.is_null() || capacity < size || capacity > isize::MAX as usize {
        return 0;
    }
    let mut header = [0u8; 10];
    header[0] = (opcode & 0xf) | (u8::from(flags & 1 != 0) << 7) | (u8::from(flags & 4 != 0) << 6);
    header[1] = u8::from(flags & 32 != 0) << 7;
    match size {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "size 2 means payload_length < 126"
        )]
        2 => header[1] |= payload_length as u8,
        4 => {
            header[1] |= 126;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "size 4 means payload_length fits u16"
            )]
            let short = payload_length as u16;
            header[2..4].copy_from_slice(&short.to_be_bytes());
        }
        _ => {
            header[1] |= 127;
            header[2..10].copy_from_slice(&payload_length.to_be_bytes());
        }
    }
    // SAFETY: The caller's exclusive output has enough capacity. The local
    // header cannot overlap it, and only the encoded header bytes are written.
    unsafe { std::ptr::copy_nonoverlapping(header.as_ptr(), output, size) };
    size
}

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
    let key_word = unsafe { key.cast::<u32>().read_unaligned() };
    let mask = key_word.to_ne_bytes();
    // SAFETY: non-null, `length <= isize::MAX`; the caller lends `length` writable bytes and
    // the key was copied out first, so an overlapping key is fine.
    let payload = unsafe { std::slice::from_raw_parts_mut(data, length) };
    let word_mask = u64::from(key_word) | (u64::from(key_word) << 32);
    let (words, tail) = payload.as_chunks_mut::<8>();
    for bytes in words {
        *bytes = (u64::from_ne_bytes(*bytes) ^ word_mask).to_ne_bytes();
    }
    // Each complete word consumes two whole keys, so the tail restarts at 0.
    for (byte, key) in tail.iter_mut().zip(mask.iter().cycle()) {
        *byte ^= key;
    }
    0
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
