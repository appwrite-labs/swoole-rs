//! The former `codec.cc`, check for check: the Rust codec against the pinned C++
//! codec bodies and PHP policy snippets (`swoole_rs_oracle::websocket`), over the
//! same opcode, flag and length inputs. Each C export the gate called is replaced
//! by the safe function that was its body (README.md maps them).
use std::mem::MaybeUninit;

use swoole_rs_oracle::websocket as reference;
use swoole_rs_websocket::{
    Action, Context, Decision, DecodedFrame, Delivery, INVALID_CONTROL, PolicyError,
    control_invalid, decide_frame, decode_in_place, encode_close_into, encode_parts, encoded_size,
    message_policy, pack_policy,
};

const FILL: u8 = 0xa5;

/// `count` bytes of the fill the C++ gate pre-loaded its outputs with.
fn filled(count: usize) -> Vec<MaybeUninit<u8>> {
    vec![MaybeUninit::new(FILL); count]
}

fn bytes(storage: &[MaybeUninit<u8>]) -> Vec<u8> {
    storage
        .iter()
        // SAFETY: `filled` initialized every byte and the codec only writes bytes.
        .map(|byte| unsafe { byte.assume_init() })
        .collect()
}

/// Pinned `encode` into a fresh String: its result and the String's bytes.
fn reference_encode(payload: &[u8], opcode: u8, flags: u8) -> (bool, Vec<u8>) {
    let mut out = vec![0u8; payload.len() + 16];
    let mut written = 0;
    // SAFETY: `payload` and `out` are live for their lengths; `written` is a local.
    let accepted = unsafe {
        reference::encode(
            payload.as_ptr(),
            payload.len(),
            opcode,
            flags,
            out.as_mut_ptr(),
            out.len(),
            &raw mut written,
        )
    };
    out.truncate(written);
    (accepted, out)
}

/// Pinned `decode` over `data` in place.
fn reference_decode(data: &mut [u8]) -> (bool, reference::Frame) {
    let mut frame = reference::Frame::default();
    // SAFETY: `data` is live and writable for its length; `frame` is a local.
    let success = unsafe { reference::decode(data.as_mut_ptr(), data.len(), &raw mut frame) };
    (success, frame)
}

#[test]
fn layouts() {
    assert_eq!(size_of::<DecodedFrame>(), 24);
    assert_eq!(size_of::<Context>(), 24);
    assert_eq!(size_of::<Decision>(), 32);
}

#[test]
fn php_policy() {
    let mut cases = 0;
    for opcode in 0..=255u8 {
        for flags in 0..=255u8 {
            for length in [0, 1, 65536, usize::MAX] {
                // The reference packs the flags and, in bit 8, compression.
                let (packed, compress) = pack_policy(opcode, flags, length);
                assert_eq!(
                    u16::from(packed) | (u16::from(compress) << 8),
                    reference::pack_policy(opcode, flags, length)
                );
                cases += 1;
            }
            // The reference's 0 delivers, 1 discards, 2 pongs.
            let delivery = match message_policy(opcode, flags) {
                Delivery::Deliver => 0,
                Delivery::Discard => 1,
                Delivery::Pong => 2,
            };
            assert_eq!(delivery, reference::message_policy(opcode, flags));
            cases += 1;
        }
    }
    assert_eq!(cases, 256 * 256 * 5);
}

#[test]
fn encode_and_decode_prefixes() {
    let (mut encoded, mut decoded) = (0, 0);
    for length in [0usize, 1, 7, 8, 125, 126, 65535, 65536] {
        #[expect(clippy::cast_possible_truncation, reason = "the gate's byte pattern")]
        let payload: Vec<u8> = (0..length).map(|i| (i * 37 + 19) as u8).collect();
        for flags in 0..=255u8 {
            for opcode in 0..16u8 {
                let (accepted, oracle) = reference_encode(&payload, opcode, flags);
                let size = encoded_size(length, flags);
                let mut wire = filled(size + 2);
                // The export lent no payload to a header-only encode.
                let data: &[u8] = if flags & 64 == 0 { &payload } else { &[] };
                let written = encode_parts(length, data, opcode, flags, &mut wire[1..=size]);
                let wire = bytes(&wire);
                // Close/ping/pong must be final and at most 125 bytes; both sides reject before writing.
                if control_invalid(opcode, flags, length) {
                    assert!(!accepted && oracle.is_empty() && written == 0);
                    assert!(wire.iter().all(|&byte| byte == FILL));
                    encoded += 1;
                    continue;
                }
                assert_eq!(written, size);
                assert!(wire[0] == FILL && wire[size + 1] == FILL);
                assert!(oracle.len() == size && oracle == wire[1..=size]);
                assert_eq!(accepted, !((flags & 32 != 0) && (flags & 64 != 0)));
                encoded += 1;
                // Every header prefix and incomplete/complete body, plus trailing bytes.
                if flags & 64 != 0 {
                    continue;
                }
                let header = size - length;
                let mut prefixes: Vec<usize> = (2..=header).collect();
                if length != 0 {
                    prefixes.push(size - 1);
                }
                prefixes.push(size);
                prefixes.push(size + 1);
                for prefix in prefixes {
                    // Exactly the advertised prefix, so a sanitizer sees reads past
                    // an incomplete header or body, not only writes.
                    let mut native = wire[1..=prefix].to_vec();
                    let mut rust = native.clone();
                    let (success, expected) = reference_decode(&mut native);
                    let (total, actual) = decode_in_place(&mut rust);
                    assert_eq!(
                        success,
                        total > 0 && usize::try_from(total).is_ok_and(|total| total <= prefix)
                    );
                    assert_eq!(actual.opcode, expected.opcode);
                    assert_eq!(actual.flags, expected.flags);
                    assert_eq!(actual.length_code, expected.length_code);
                    assert_eq!(native, rust);
                    if success {
                        assert_eq!(actual.header_length, usize::from(expected.header_length));
                        assert_eq!(actual.payload_length as u64, expected.payload_length);
                        if length != 0 && flags & 32 != 0 {
                            assert_eq!(actual.mask_key, expected.mask_key);
                        }
                    }
                    decoded += 1;
                }
            }
        }
    }
    assert_eq!(encoded, 8 * 256 * 16);
    assert!(decoded > 0);
}

#[test]
fn close_frames() {
    let mut closes = 0;
    #[expect(clippy::cast_possible_truncation, reason = "the gate's byte pattern")]
    let reason: [u8; 126] = std::array::from_fn(|i| (i + 128) as u8);
    for code in [i32::MIN, -1, 0, 1000, 65535, 65536, i32::MAX] {
        for length in [0usize, 1, 123, 124, 125, 126] {
            for flags in 0..=255u8 {
                let mut oracle = [0u8; 144];
                let mut oracle_length = 0;
                // SAFETY: `reason` holds `length` bytes, `oracle` its length; the count is a local.
                let accepted = unsafe {
                    reference::pack_close(
                        code,
                        reason.as_ptr(),
                        length,
                        flags,
                        oracle.as_mut_ptr(),
                        oracle.len(),
                        &raw mut oracle_length,
                    )
                };
                let mut wire = filled(144);
                let size = encode_close_into(code, &reason[..length], flags, &mut wire[1..143]);
                let wire = bytes(&wire);
                // Code plus reason is a control payload: at most 125 bytes.
                if length > 123 {
                    assert!(size == 0 && oracle_length == 0 && !accepted);
                } else {
                    assert!(size == oracle_length && wire[1..=size] == oracle[..size]);
                    assert_eq!(accepted, !((flags & 32 != 0) && (flags & 64 != 0)));
                }
                assert!(wire[0] == FILL && wire[size + 1] == FILL);
                closes += 1;
            }
        }
    }
    assert_eq!(closes, 7 * 6 * 256);
}

/// The decoder reads all RSV bits, including combinations the encoder clears.
#[test]
fn decoder_reads_every_first_byte() {
    for first in 0..=255u8 {
        for second in [0u8, 128] {
            let (length, complete) = if second == 0 { (2, 2i64) } else { (6, 6) };
            let mut native = [first, second, 1, 2, 3, 4];
            let mut rust = native;
            // Compressed (RSV1) close/ping/pong frames are rejected before unmasking.
            let opcode = first & 15;
            let invalid = first & 64 != 0 && (8..=10).contains(&opcode);
            let (success, expected) = reference_decode(&mut native[..length]);
            assert_eq!(success, !invalid);
            let (total, actual) = decode_in_place(&mut rust[..length]);
            assert_eq!(total, if invalid { INVALID_CONTROL } else { complete });
            assert_eq!(native, rust);
            assert!(actual.flags == expected.flags && actual.opcode == expected.opcode);
        }
    }
}

/// Observed policy: wire-length control bounds, permissive FIN/RSV, orphan and
/// overlapping fragments, a FIN message during active fragmentation, the exact
/// aggregate limit and the preserved initial fragment opcode and flags.
#[test]
fn decision_policy() {
    fn check(
        opcode: u8,
        flags: u8,
        wire: usize,
        payload: usize,
        context: Context,
        action: Action,
        error: PolicyError,
    ) {
        let frame = DecodedFrame {
            opcode,
            flags,
            payload_length: payload,
            header_length: if flags & 32 != 0 { 6 } else { 2 },
            ..DecodedFrame::default()
        };
        let result = decide_frame(&frame, wire, &context).expect("a frame within the wire");
        assert!(result.action == action && result.error == error);
        if action == Action::Finish {
            assert_eq!(result.ext_flags, context.fragment_flags | 1);
        }
    }
    use Action::{
        Append, ClientClose, Control, Dispatch, Finish, Reject, ServerClose, Start, Unknown,
    };
    use PolicyError as Error;
    let none = Context::default();
    for flags in [0u8, 1, 32, 33, 61] {
        let header = if flags & 32 != 0 { 6 } else { 2 };
        let fin = flags & 1 != 0;
        check(0, flags, header + 1, 1, none, Reject, Error::Orphan);
        let mut active = Context {
            fragment_length: 65535,
            package_max_length: 65536,
            fragment_flags: 0x021c,
            has_fragment: 1,
            closing: 0,
        };
        check(
            0,
            flags,
            header + 1,
            1,
            active,
            if fin { Finish } else { Append },
            Error::None,
        );
        check(0, flags, header + 2, 2, active, Reject, Error::TooBig);
        active.fragment_length = usize::MAX;
        check(0, flags, header + 1, 1, active, Reject, Error::TooBig);
        for opcode in [1, 2] {
            check(
                opcode,
                flags,
                header + 1,
                1,
                none,
                if fin { Dispatch } else { Start },
                Error::None,
            );
            check(
                opcode,
                flags,
                header + 1,
                1,
                active,
                if fin { Dispatch } else { Reject },
                if fin { Error::None } else { Error::Overlap },
            );
        }
        for opcode in [9, 10] {
            check(
                opcode,
                flags,
                126,
                126 - header,
                active,
                Control,
                Error::None,
            );
            check(
                opcode,
                flags,
                127,
                127 - header,
                active,
                Reject,
                Error::ControlTooBig,
            );
        }
        check(
            8,
            flags,
            127,
            127 - header,
            active,
            ClientClose,
            Error::None,
        );
        check(
            8,
            flags,
            128,
            128 - header,
            active,
            Reject,
            Error::CloseTooBig,
        );
        active.closing = 1;
        check(8, flags, header, 0, active, ServerClose, Error::None);
        check(15, flags, header, 0, active, Unknown, Error::None);
    }
}

#[test]
fn short_output_is_left_untouched() {
    for capacity in 0..7 {
        let mut untouched = filled(16);
        assert_eq!(encode_parts(1, b"x", 1, 33, &mut untouched[..capacity]), 0);
        assert!(bytes(&untouched).iter().all(|&byte| byte == FILL));
    }
}

#[test]
fn rejected_arguments() {
    assert_eq!(encoded_size(usize::MAX, 0), 0);
    assert_eq!(decode_in_place(&mut []).0, 0);
    let context = Context::default();
    let empty = DecodedFrame {
        flags: 32,
        header_length: 2,
        ..DecodedFrame::default()
    };
    assert_eq!(decide_frame(&empty, 2, &context), None);
}

/// Upstream accepts a non-minimal extended ping header but dispatches from
/// offset two, including the extended-length bytes in application data.
#[test]
fn extended_ping_dispatches_from_offset_two() {
    let extended = DecodedFrame {
        opcode: 9,
        header_length: 4,
        payload_length: 1,
        ..DecodedFrame::default()
    };
    let decision = decide_frame(&extended, 5, &Context::default()).expect("a complete frame");
    assert!(decision.action == Action::Control && decision.offset == 2 && decision.length == 3);
}
