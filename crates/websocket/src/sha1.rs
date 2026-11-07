//! SHA-1, the one hash the WebSocket handshake needs, from the OpenSSL the
//! extension already links (crates/tls).
//!
//! RFC 6455 mandates SHA-1 for the `Sec-WebSocket-Accept` token, so this is a
//! protocol constant, not a security choice: the server handshake ([`crate::accept`])
//! and the client's check of it (`swoole_rs_clients::ws`).

/// Compute the 20-byte SHA-1 digest of `data`.
#[must_use]
pub fn digest(data: &[u8]) -> [u8; 20] {
    let mut out = [0u8; 20];
    // SAFETY: `data` lends `data.len()` readable bytes and `out` is the 20-byte
    // output SHA-1 writes; OpenSSL retains neither pointer.
    unsafe { openssl_sys::SHA1(data.as_ptr(), data.len(), out.as_mut_ptr()) };
    out
}

#[cfg(test)]
mod tests {
    use super::digest;

    fn hex(bytes: &[u8; 20]) -> String {
        bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .concat()
    }

    #[test]
    fn fips_known_answers() {
        // FIPS 180-4 example vectors plus the empty string.
        assert_eq!(
            hex(&digest(b"")),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
        assert_eq!(
            hex(&digest(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(&digest(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn spans_block_boundary() {
        // The 56-byte §5.3.1 vector above already forces a second padding block
        // (0x80 + length no longer fit after the data). The million-'a' vector
        // exercises the many-block loop.
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            hex(&digest(&million)),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
    }
}
