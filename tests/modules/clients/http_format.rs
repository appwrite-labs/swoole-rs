//! The former `http_format_witness.cpp`, check for check: `http_format` against the
//! pinned request formatter calls (`swoole_rs_oracle::clients::http_format`: the pinned
//! `sw_snprintf` and form formats) at the same texts, numbers and bounded capacities.
//! The export the gate called is replaced by the safe function that was its body.
use std::ffi::c_int;

use swoole_rs_clients::http_format::{Format, http_format};
use swoole_rs_oracle::clients as reference;

/// The pinned call into `output` (none for a null one of no capacity): its result and
/// errno afterwards, from errno 17.
fn native(
    kind: u32,
    texts: [&[u8]; 4],
    number: usize,
    output: Option<&mut [u8]>,
) -> (usize, c_int) {
    let (pointers, lengths) = (
        texts.map(|text| text.as_ptr().cast()),
        texts.map(<[u8]>::len),
    );
    let (at, capacity) = output.map_or((std::ptr::null_mut(), 0), |output| {
        (output.as_mut_ptr().cast(), output.len())
    });
    let mut error = 0;
    // SAFETY: four live texts with their lengths, `capacity` writable bytes and a local.
    let written = unsafe {
        reference::http_format(
            kind,
            pointers.as_ptr(),
            lengths.as_ptr(),
            number,
            at,
            capacity,
            &raw mut error,
        )
    };
    (written, error)
}

/// The Rust formatter phpclients calls, with its error mapped to the errno
/// `sw_snprintf` sets, from errno 17.
fn rust(kind: Format, texts: [&[u8]; 4], number: usize, output: &mut [u8]) -> (usize, c_int) {
    let result = http_format(kind, texts.map(Some), number, output);
    let error = match result.error {
        0 => 17,
        2 => libc::EOVERFLOW,
        _ => libc::EINVAL,
    };
    (result.written, error)
}

#[test]
fn bounded_capacities_match_upstream() {
    let lengths = [0, 1, 7, 38, 39, 40, 100, 2040, 2048, 2050];
    let numbers = [0, 1, 9, 10, 99, 100, 65535, usize::MAX];
    let capacities = [0, 1, 2, 7, 15, 16, 19, 20, 21, 22, 23, 39, 40, 64, 2048];
    let mut cases = 0;
    for byte in 0..=255u8 {
        for length in lengths {
            let (mut a, mut b) = (vec![b'b'; 39], vec![byte; length]);
            let (mut c, d) = (vec![b'f'; length], vec![b't'; length]);
            a[usize::from(byte) % 39] = byte;
            if length > 1 {
                b[length / 2] = 0;
                c[length / 2] = 0;
            }
            if length > 2 {
                b[length - 1] = b':';
            }
            for kind in Format::ALL {
                for capacity in capacities {
                    let (mut x, mut y) = ([0x5a_u8; 4096], [0x5a_u8; 4096]);
                    let number = if kind == Format::ProxyUri {
                        usize::from(byte) * 257
                    } else {
                        numbers[usize::from(byte) % numbers.len()]
                    };
                    let texts = [&a[..], &b, &c, &d];
                    let expected = native(kind as u32, texts, number, Some(&mut x[..capacity]));
                    let actual = rust(kind, texts, number, &mut y[..capacity]);
                    assert!(
                        expected == actual && x == y,
                        "bounded {}:{byte}:{length}:{capacity}:{expected:?}:{actual:?}",
                        kind as u32
                    );
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 230_400);
}

#[test]
fn null_output_matches_upstream() {
    for kind in Format::ALL {
        let texts: [&[u8]; 4] = [b"boundary", b"host", b"file", b"application/octet-stream"];
        let number = if kind == Format::ProxyUri {
            65535
        } else {
            usize::MAX
        };
        assert_eq!(
            native(kind as u32, texts, number, None),
            rust(kind, texts, number, &mut []),
            "{}",
            kind as u32
        );
    }
}

/// The witness's Rust-only case of a span that is null with a length, which the safe
/// function takes as `None`. Its other case, a null output of capacity 1, has no safe
/// form: a slice is never null.
#[test]
fn an_invalid_span_is_an_error() {
    let mut defensive = [0x5a_u8; 16];
    let invalid = http_format(Format::MultipartType, [None; 4], 0, &mut defensive[..]);
    assert_eq!((invalid.error, invalid.written, defensive[0]), (1, 0, 0));
}
