//! SHA-1 (FIPS 180-4), the one hash the WebSocket handshake needs.
//!
//! RFC 6455 mandates SHA-1 for the `Sec-WebSocket-Accept` token, so this is a
//! protocol constant, not a security choice: the server handshake ([`crate::accept`])
//! and the client's check of it (`swoole_rs_clients::ws`).
//!
//! Hand-rolled, as upstream uses PHP's own SHA-1: OpenSSL 3's `SHA1()` fetches
//! its method through the provider registry on each call, which slowed every
//! handshake enough to reorder concurrent upgrades (#2088).

/// Initial hash state (FIPS 180-4 §5.3.1).
const INIT: [u32; 5] = [
    0x6745_2301,
    0xEFCD_AB89,
    0x98BA_DCFE,
    0x1032_5476,
    0xC3D2_E1F0,
];

/// Compute the 20-byte SHA-1 digest of `data`.
#[must_use]
pub fn digest(data: &[u8]) -> [u8; 20] {
    let mut h = INIT;
    let mut schedule = [0u32; 80];

    let (blocks, rest) = data.as_chunks::<64>();
    for block in blocks {
        compress(&mut h, &mut schedule, block);
    }

    // Padding: the remainder, a 0x80 byte, zeros, then the 64-bit big-endian bit
    // length. It spills into a second block when the remainder leaves no room.
    let bit_len = u64::try_from(data.len())
        .unwrap_or(u64::MAX)
        .wrapping_mul(8);
    let mut tail = [[0u8; 64]; 2];
    for (slot, &byte) in tail
        .as_flattened_mut()
        .iter_mut()
        .zip(rest.iter().chain(&[0x80]))
    {
        *slot = byte;
    }
    let used = if rest.len() < 56 { 1 } else { 2 };
    if let Some(length) = tail
        .get_mut(used - 1)
        .and_then(|block| block.last_chunk_mut())
    {
        *length = bit_len.to_be_bytes();
    }
    for block in tail.iter().take(used) {
        compress(&mut h, &mut schedule, block);
    }

    let mut out = [0u8; 20];
    let (words, _) = out.as_chunks_mut::<4>();
    for (word, slot) in h.iter().zip(words) {
        *slot = word.to_be_bytes();
    }
    out
}

/// Fold one 64-byte block into the hash state.
fn compress(h: &mut [u32; 5], w: &mut [u32; 80], block: &[u8; 64]) {
    let (block_words, _) = block.as_chunks::<4>();
    for (word, bytes) in w.iter_mut().zip(block_words) {
        *word = u32::from_be_bytes(*bytes);
    }
    for i in 16..80 {
        if let Some([w16, _, w14, _, _, _, _, _, w8, _, _, _, _, w3, _, _, word]) =
            w.get_mut(i - 16..=i)
        {
            *word = (*w3 ^ *w8 ^ *w14 ^ *w16).rotate_left(1);
        }
    }

    let [mut a, mut b, mut c, mut d, mut e] = *h;
    for (i, &word) in w.iter().enumerate() {
        let (f, k) = match i {
            0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
            20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
            _ => (b ^ c ^ d, 0xCA62_C1D6),
        };
        let tmp = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(word);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = tmp;
    }

    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
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
