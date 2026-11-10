//! Differential tests against the pinned upstream C++ (`swoole-rs-oracle`).

use swoole_rs_http::cookie_next;

/// The cases of the former `tests/modules/http/cookie_flow_gate.cc`: the same callback sequence and
/// scratch bytes as the pinned `parse_cookie`, and one callback when it stops.
#[test]
fn ported_cookie_flow_gate() {
    /// The pinned `parse_cookie`'s records and in-place scratch bytes for `input`,
    /// stopping after the first record when `first_only`.
    fn pinned_cookies(input: &[u8], first_only: bool) -> (Vec<u8>, Vec<u8>) {
        let (mut records, mut scratch) = (vec![0; 8 * input.len() + 16], vec![0; input.len() + 1]);
        // SAFETY: `records` and `scratch` have the capacities `cookies` documents.
        let size = unsafe {
            swoole_rs_oracle::http::cookies(
                input.as_ptr(),
                input.len(),
                records.as_mut_ptr(),
                first_only,
                scratch.as_mut_ptr(),
            )
        };
        records.truncate(size);
        (records, scratch)
    }
    /// `cookie_next` over a NUL-terminated copy of `input`, recorded as
    /// `pinned_cookies` records them, and the copy afterwards.
    fn rust_cookies(input: &[u8], first_only: bool) -> (Vec<u8>, Vec<u8>, usize) {
        let mut buffer = [input, &[0]].concat();
        let (mut cursor, mut records, mut count) = (0, Vec::new(), 0);
        while let Some(cookie) = cookie_next(&mut buffer, &mut cursor).unwrap() {
            for (offset, length) in [
                (cookie.key_offset, cookie.key_length),
                (cookie.value_offset, cookie.value_length),
            ] {
                records.extend_from_slice(&u32::try_from(length).unwrap().to_ne_bytes());
                records.extend_from_slice(&buffer[offset..offset + length]);
            }
            count += 1;
            if first_only {
                break;
            }
        }
        (records, buffer, count)
    }
    let mut inputs: Vec<String> = [
        "first=one",
        "first=one; second=two; last=three",
        "first=one; flag; empty=; last=three",
        "name=plain; path=/ordinary; domain=example.test",
    ]
    .map(String::from)
    .to_vec();
    for count in 1..=16u8 {
        for width in 1..=8 {
            let fields: Vec<String> = (0..count)
                .map(|i| {
                    format!(
                        "field{i}={}",
                        char::from(b'a' + i).to_string().repeat(width)
                    )
                })
                .collect();
            inputs.push(fields.join("; "));
        }
    }
    let mut cases = 0;
    for input in &inputs {
        for stop in [false, true] {
            let (records, scratch, count) = rust_cookies(input.as_bytes(), stop);
            assert_eq!(
                (records, scratch),
                pinned_cookies(input.as_bytes(), stop),
                "{input:?} stop={stop}"
            );
            if stop {
                assert_eq!(count, 1, "{input:?} stops after one callback");
            }
            cases += 1;
        }
    }
    assert_eq!(cases, 264);
}
