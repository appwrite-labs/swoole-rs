//! WebSocket client handshake: does the server's `Sec-WebSocket-Accept` match
//! the key we sent? The Zend header extraction and the `Upgrade`/`Connection`
//! token checks stay native; Rust owns computing the expected accept token and
//! the comparison.

use swoole_rs_base64::swr_v1_base64_encode;

use swoole_rs_websocket::sha1;

/// RFC 6455 handshake GUID, appended to the client key before hashing.
const GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// base64(SHA-1(key + GUID)) is always 28 characters for a 20-byte digest.
const ACCEPT_LEN: usize = 28;

/// Compute the expected `Sec-WebSocket-Accept` token for `key`.
///
/// Upstream builds the hash input with `sw_snprintf(buf, 128, "%s%s", key, GUID)`,
/// so the key stops at its first NUL and the concatenation is truncated to 127
/// bytes before hashing. Both quirks are reproduced here for byte-exactness.
fn expected_accept(key: &[u8]) -> [u8; ACCEPT_LEN] {
    let mut input = [0u8; 127];
    let mut used = 0;
    for (slot, &byte) in input.iter_mut().zip(crate::c_str(key).iter().chain(GUID)) {
        *slot = byte;
        used += 1;
    }

    let digest = sha1::digest(input.get(..used).unwrap_or_default());

    // swr_v1_base64_encode needs 4*ceil(20/3)+1 = 29 output bytes and writes a
    // trailing NUL past the 28 it counts.
    let mut encoded = [0u8; 29];
    // SAFETY: `digest` lends 20 readable bytes and `encoded` is a distinct
    // 29-byte stack buffer, meeting the encoder's size contract; neither pointer
    // is retained.
    let written =
        unsafe { swr_v1_base64_encode(digest.as_ptr(), digest.len(), encoded.as_mut_ptr()) };

    let mut accept = [0u8; ACCEPT_LEN];
    for (slot, &byte) in accept.iter_mut().zip(&encoded).take(written) {
        *slot = byte;
    }
    accept
}

/// Verify a server WebSocket handshake accept token against our request key.
///
/// Returns 1 when `base64(SHA-1(key + GUID))` equals `accept`, else 0. `key` is
/// the client's `Sec-WebSocket-Key` request header; `accept` is the server's
/// `Sec-WebSocket-Accept` response header. The accept token is a fixed 28 bytes,
/// so any other length can never match.
///
/// # Safety
/// `key` must point to `key_len` readable bytes or be null when `key_len` is 0;
/// `accept` must point to `accept_len` readable bytes or be null when
/// `accept_len` is 0. Neither pointer is retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_ws_accept_valid(
    key: *const u8,
    key_len: usize,
    accept: *const u8,
    accept_len: usize,
) -> u8 {
    if accept_len != ACCEPT_LEN || accept.is_null() {
        return 0;
    }
    let key_slice: &[u8] = if key_len == 0 {
        &[]
    } else if key.is_null() {
        return 0;
    } else {
        // SAFETY: the caller lends `key_len` readable bytes at non-null `key`.
        unsafe { core::slice::from_raw_parts(key, key_len) }
    };
    // SAFETY: the caller lends `accept_len` readable bytes at non-null `accept`.
    let accept_slice = unsafe { core::slice::from_raw_parts(accept, accept_len) };

    u8::from(expected_accept(key_slice) == accept_slice)
}

#[cfg(test)]
mod tests {
    use super::{ACCEPT_LEN, expected_accept, swr_v1_clients_ws_accept_valid};

    fn verify(key: &[u8], accept: &[u8]) -> u8 {
        // SAFETY: both slices lend their full length of readable bytes.
        unsafe {
            swr_v1_clients_ws_accept_valid(key.as_ptr(), key.len(), accept.as_ptr(), accept.len())
        }
    }

    #[test]
    fn rfc6455_example() {
        // RFC 6455 §1.3: key "dGhlIHNhbXBsZSBub25jZQ==" yields this accept token.
        let key = b"dGhlIHNhbXBsZSBub25jZQ==";
        let accept = b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";
        assert_eq!(&expected_accept(key), accept);
        assert_eq!(verify(key, accept), 1);
    }

    #[test]
    fn wrong_accept_rejected() {
        let key = b"dGhlIHNhbXBsZSBub25jZQ==";
        assert_eq!(verify(key, b"s3pPLMBiTxaQ9kYGzzhZRbK+xOx="), 0);
    }

    #[test]
    fn non_28_length_never_matches() {
        let key = b"dGhlIHNhbXBsZSBub25jZQ==";
        assert_eq!(verify(key, b"short"), 0);
        assert_eq!(verify(key, b""), 0);
        // A 28-char prefix of a longer token still fails on length.
        assert_eq!(verify(key, b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=="), 0);
    }

    #[test]
    fn key_stops_at_nul() {
        // Upstream's "%s" copies the key only up to a NUL, so bytes after it do
        // not change the token.
        let mut padded = b"dGhlIHNhbXBsZSBub25jZQ==\0ignored".to_vec();
        let accept = b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";
        assert_eq!(verify(&padded, accept), 1);
        padded.truncate(ACCEPT_LEN); // sanity: still contains the NUL-terminated key
        assert_eq!(&expected_accept(b"dGhlIHNhbXBsZSBub25jZQ==\0x"), accept);
    }

    #[test]
    fn empty_key_is_handled() {
        // No key: just base64(SHA-1(GUID)). Must not panic; token is deterministic.
        let token = expected_accept(b"");
        assert_eq!(token.len(), ACCEPT_LEN);
        // SAFETY: null key with zero length is the documented empty case.
        assert_eq!(
            unsafe {
                swr_v1_clients_ws_accept_valid(core::ptr::null(), 0, token.as_ptr(), token.len())
            },
            1
        );
    }
}
