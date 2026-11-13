use std::cell::Cell;
use std::ffi::{CStr, c_int};
use std::ptr::{null, null_mut};
use swoole_rs_corebase::log;
use swoole_rs_websocket_ffi::frame_api::mask;

thread_local! {
    /// Warnings `websocket::mask` logged on this thread that it rejected its input.
    static REJECTED: Cell<u32> = const { Cell::new(0) };
}

fn record(kind: c_int, _: c_int, _: c_int, origin: &CStr, message: &CStr) {
    if (kind, origin, message)
        == (
            log::LOG_WARNING,
            c"websocket::mask",
            c"Rust WebSocket mask rejected input",
        )
    {
        REJECTED.set(REJECTED.get() + 1);
    }
}

/// `websocket::mask`, and whether it took its input: 0, or -1 when it warned that it
/// rejected it. Those are the statuses the mask kernel returned to it while the two
/// were separate functions.
///
/// # Safety
/// As `websocket::mask`.
unsafe fn mask_status(data: *mut u8, length: usize, key: *const u8) -> i32 {
    log::swr_v1_core_set_log_host(record);
    let before = REJECTED.get();
    // SAFETY: as the caller promises.
    unsafe { mask(data.cast(), length, key.cast()) };
    let warnings = REJECTED.get() - before;
    assert!(warnings <= 1, "{warnings} warnings from one mask");
    if warnings == 0 { 0 } else { -1 }
}

#[test]
fn invalid_mask_pointers_and_lengths_do_not_write() {
    let mut output = [0xa5; 10];
    unsafe {
        assert_eq!(mask_status(null_mut(), 0, null()), 0);
        assert_eq!(mask_status(null_mut(), 1, output.as_ptr()), -1);
        assert_eq!(mask_status(output.as_mut_ptr(), 1, null()), -1);
        assert_eq!(
            mask_status(output.as_mut_ptr(), usize::MAX, output.as_ptr()),
            -1
        );
        assert_eq!(output, [0xa5; 10]);
    }
}

#[test]
fn binary_mask_matches_known_vector_and_is_an_involution() {
    // RFC 6455 section 5.7's masked Hello frame.
    let key = [0x37, 0xfa, 0x21, 0x3d];
    let mut hello = *b"Hello";
    unsafe { mask_status(hello.as_mut_ptr(), hello.len(), key.as_ptr()) };
    assert_eq!(hello, [0x7f, 0x9f, 0x4d, 0x51, 0x58]);
    for length in 1..=1024 {
        let mut bytes: Vec<_> = (0..length + 2).map(|x: usize| x.to_le_bytes()[0]).collect();
        let original = bytes.clone();
        assert_eq!(
            unsafe { mask_status(bytes[1..].as_mut_ptr(), length, key.as_ptr()) },
            0
        );
        for i in 0..length {
            assert_eq!(bytes[i + 1], original[i + 1] ^ key[i % 4]);
        }
        assert_eq!(bytes[0], original[0]);
        assert_eq!(bytes[length + 1], original[length + 1]);
        assert_eq!(
            unsafe { mask_status(bytes[1..].as_mut_ptr(), length, key.as_ptr()) },
            0
        );
        assert_eq!(bytes, original);
    }
}

#[test]
fn masking_copies_overlapping_key_before_writing() {
    // Every byte alignment and word tail, including keys partly overlapping
    // either payload boundary: the key is read before any payload write.
    for alignment in 0..8 {
        for length in 0..=33 {
            let start = 8 + alignment;
            for key_offset in start - 4..=start + length {
                let mut bytes: Vec<u8> = (0..64u8)
                    .map(|i| i.wrapping_mul(37).wrapping_add(19))
                    .collect();
                let original = bytes.clone();
                let base = bytes.as_mut_ptr();
                // SAFETY: payload and key both lie inside `bytes`.
                let result = unsafe { mask_status(base.add(start), length, base.add(key_offset)) };
                assert_eq!(result, 0);
                for (i, value) in bytes.iter().enumerate() {
                    let expected = if (start..start + length).contains(&i) {
                        original[i] ^ original[key_offset + (i - start) % 4]
                    } else {
                        original[i]
                    };
                    assert_eq!(
                        *value, expected,
                        "alignment {alignment} length {length} key {key_offset}"
                    );
                }
            }
        }
    }
}
