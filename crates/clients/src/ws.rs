//! WebSocket client handshake: does the server's `Sec-WebSocket-Accept` match
//! the key we sent? The Zend header extraction and the `Upgrade`/`Connection`
//! token checks stay native; Rust owns computing the expected accept token and
//! the comparison.

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

    // 4*ceil(20/3) = 28 characters and the encoder's trailing NUL.
    let mut encoded = [0u8; 29];
    let written = swoole_rs_base64::encode(&digest, &mut encoded);

    let mut accept = [0u8; ACCEPT_LEN];
    for (slot, &byte) in accept.iter_mut().zip(&encoded).take(written) {
        *slot = byte;
    }
    accept
}

/// Whether `accept`, the server's `Sec-WebSocket-Accept`, is
/// `base64(SHA-1(key + GUID))` for `key`, the client's `Sec-WebSocket-Key`. The
/// token is a fixed 28 bytes, so any other length never matches.
#[must_use]
pub fn accept_valid(key: &[u8], accept: &[u8]) -> bool {
    expected_accept(key) == accept
}

#[cfg(test)]
mod tests {
    use super::{ACCEPT_LEN, accept_valid, expected_accept};

    fn verify(key: &[u8], accept: &[u8]) -> u8 {
        u8::from(accept_valid(key, accept))
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
        assert_eq!(verify(b"", &token), 1);
    }
}
