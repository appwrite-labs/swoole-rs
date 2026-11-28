//! Differential tests against the pinned upstream C++ (`swoole-rs-oracle`).

use proptest::prelude::*;
use swoole_rs_crc32::crc32;
use swoole_rs_oracle::crc32 as upstream;

/// Compare on input read one byte past an allocation start, so neither side
/// sees an aligned buffer, and at every offset into an 8-byte window, which
/// exercises the Rust word-at-a-time path and its tail.
fn compare(bytes: &[u8]) {
    let mut shifted = vec![0xa5; 8];
    shifted.extend_from_slice(bytes);
    for offset in 1..8 {
        let mut buffer = vec![0xa5; offset];
        buffer.extend_from_slice(bytes);
        let input = buffer.as_ptr().wrapping_add(offset);
        // SAFETY: `input` points into `buffer`, which holds `bytes.len()` readable bytes after it.
        let want = unsafe { upstream::crc32(input, bytes.len()) };
        let got = crc32(buffer.get(offset..).unwrap_or_default());
        assert_eq!(got, want, "crc32 at offset {offset} of {bytes:?}");
    }
}

/// `TEST(hash, crc32)` from `core-tests/src/core/hash.cpp`.
#[test]
fn hash_crc32() {
    let data = c"hello world, PHP  the best.";
    assert_eq!(crc32(data.to_bytes()), 2_962_796_788);
}

#[test]
fn empty_and_every_one_and_two_byte_input() {
    compare(&[]);
    for a in 0..=255u8 {
        compare(&[a]);
        for b in 0..=255u8 {
            compare(&[a, b]);
        }
    }
}

#[test]
fn every_length_up_to_sixty_four() {
    let bytes: Vec<u8> = (0..64u32).map(|i| (i * 37 + 19).to_le_bytes()[0]).collect();
    for length in 0..=bytes.len() {
        compare(&bytes[..length]);
    }
}

proptest! {
    #[test]
    fn arbitrary_input(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        compare(&bytes);
    }
}
