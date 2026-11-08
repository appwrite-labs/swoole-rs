//! Differential tests against the pinned upstream C++ (`swoole-rs-oracle`).

use proptest::prelude::*;
use swoole_rs_base64::{swr_v1_base64_decode, swr_v1_base64_encode};
use swoole_rs_oracle::base64 as upstream;

const RED_ZONE: usize = 17;

/// Return counts and every output byte must match, including provisional
/// decoder writes and red zones on both sides. Input is read one byte past an
/// allocation start so neither side sees aligned buffers.
fn compare(bytes: &[u8], encode: bool) {
    let mut shifted = vec![0xa5];
    shifted.extend_from_slice(bytes);
    let input = shifted.as_ptr().wrapping_add(1);
    let capacity = if encode {
        bytes.len().div_ceil(3) * 4 + 1
    } else {
        bytes.len() + 1
    };
    for sentinel in [0x00, 0xa5, 0xff] {
        let mut expected = vec![sentinel; capacity + 2 * RED_ZONE];
        let mut actual = expected.clone();
        // SAFETY: both outputs have `capacity` writable bytes after the red zone,
        // and zero-length encode (undefined upstream) is never passed.
        let (want, got) = unsafe {
            let (e, a) = (
                expected.as_mut_ptr().add(RED_ZONE),
                actual.as_mut_ptr().add(RED_ZONE),
            );
            if encode {
                (
                    upstream::encode(input, bytes.len(), e),
                    swr_v1_base64_encode(input, bytes.len(), a),
                )
            } else {
                (
                    upstream::decode(input, bytes.len(), e),
                    swr_v1_base64_decode(input, bytes.len(), a),
                )
            }
        };
        assert_eq!(got, want, "return for encode={encode} input={bytes:?}");
        assert!(
            actual == expected,
            "output for encode={encode} input={bytes:?} sentinel={sentinel}"
        );
    }
}

/// `TEST(base64, encode)` from `core-tests/src/protocol/base64.cpp`, with a
/// deterministic fixture in place of `swoole_random_bytes`.
#[test]
fn base64_encode() {
    let inbuf: Vec<u8> = (0..1023u32)
        .map(|i| (i * 37 + 19).to_le_bytes()[0])
        .collect();
    let mut outbuf = [0u8; 2048];
    let n = inbuf.len();
    // SAFETY: 1023 readable bytes in, room for their 1364 encoded bytes and NUL out.
    let n2 = unsafe { swr_v1_base64_encode(inbuf.as_ptr(), n, outbuf.as_mut_ptr()) };
    assert!(n2 > n);
}

/// `TEST(base64, decode)` from `core-tests/src/protocol/base64.cpp`.
#[test]
fn base64_decode() {
    let inbuf = b"aGVsbG8gd29ybGQ=";
    let mut outbuf = [0u8; 2048];
    // SAFETY: the input's readable bytes in, ample room out.
    let n2 = unsafe { swr_v1_base64_decode(inbuf.as_ptr(), inbuf.len(), outbuf.as_mut_ptr()) };
    assert_eq!(&outbuf[..n2], b"hello world");
}

#[test]
fn every_one_and_two_byte_input() {
    for a in 0..=255u8 {
        compare(&[a], true);
        compare(&[a], false);
        for b in 0..=255u8 {
            compare(&[a, b], true);
            compare(&[a, b], false);
        }
    }
}

#[test]
fn every_short_string_over_each_byte_class() {
    // Both alphabet ends, digits, both symbols, padding, an invalid ASCII byte
    // and a high byte, through two full quartets of length five.
    const SYMBOLS: &[u8] = b"AZaz09+/=-\x80";
    let mut strings = vec![vec![]];
    for _ in 0..5 {
        strings = strings
            .iter()
            .flat_map(|s| SYMBOLS.iter().map(move |&c| [s.as_slice(), &[c]].concat()))
            .collect();
        for s in &strings {
            compare(s, false);
        }
    }
    compare(b"", false);
}

#[test]
fn every_byte_in_every_position_through_two_quartets() {
    for length in 1..=8 {
        for position in 0..length {
            for byte in 0..=255u8 {
                let mut input = b"QUJDREVG"[..length].to_vec();
                input[position] = byte;
                compare(&input, false);
            }
        }
    }
}

fn base64_like() -> impl Strategy<Value = Vec<u8>> {
    let byte = prop_oneof![
        8 => prop::sample::select(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/".to_vec()),
        1 => Just(b'='),
        1 => any::<u8>(),
    ];
    prop::collection::vec(byte, 0..2048)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn encode_matches_upstream(bytes in prop::collection::vec(any::<u8>(), 1..2048)) {
        compare(&bytes, true);
    }

    #[test]
    fn decode_matches_upstream(bytes in base64_like()) {
        compare(&bytes, false);
    }

    #[test]
    fn decode_of_encoded_matches_upstream(bytes in prop::collection::vec(any::<u8>(), 1..2048), cut in 0usize..4) {
        let mut encoded = vec![0; bytes.len().div_ceil(3) * 4 + 1];
        // SAFETY: `encoded` has the full encoded size plus NUL.
        let length = unsafe { upstream::encode(bytes.as_ptr(), bytes.len(), encoded.as_mut_ptr()) };
        encoded.truncate(length.saturating_sub(cut));
        compare(&encoded, false);
    }
}
