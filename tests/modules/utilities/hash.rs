// SPDX-License-Identifier: Apache-2.0
// Port of the four GoogleTests in core-tests/src/core/hash.cpp.
use std::ffi::c_char;
use std::time::Duration;
use swoole_rs_corebase::{random_bytes, swoole_random_int, utilities::swoole_hash};

fn check_hash(algorithm: u32) {
    let hash = |input: &[u8]| swoole_hash(input, algorithm, c_char::MIN != 0);
    let mut cases = Vec::with_capacity(8192);
    for _ in 0..8192 {
        let [lo, hi, ..] = swoole_random_int().to_le_bytes();
        let len = 1 + usize::from(u16::from_le_bytes([lo, hi])) % 1024;
        let mut bytes = vec![0_u8; len];
        assert_eq!(random_bytes(&mut bytes), len, "fill the requested length");
        cases.push((hash(&bytes), bytes));
    }
    std::thread::sleep(Duration::from_micros(100));
    for (expected, bytes) in cases {
        assert_eq!(hash(&bytes), expected, "rehash the same bytes");
    }
}

#[test]
fn php() {
    check_hash(0);
}

#[test]
fn jenkins() {
    check_hash(1);
}

#[test]
fn austin() {
    check_hash(2);
}

#[test]
fn crc32() {
    assert_eq!(
        swoole_rs_crc32::crc32(b"hello world, PHP  the best."),
        2_962_796_788,
        "pinned CRC32"
    );
}
