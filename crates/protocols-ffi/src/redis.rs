//! RESP's C ABI over crates/protocols' codec. Besides the module's native gate,
//! the oracle's candidate redis.cc (the protocols overlay patch) calls them.
use swoole_rs_protocols::{
    RespPhpState, RespRequest, RespSpan, resp_extend_size, resp_format, resp_handler_key,
    resp_php_next, resp_receive_range, resp_spans,
};

/// [`resp_handler_key`]: the key's length, or zero on invalid input; always
/// terminates on success.
/// # Safety
/// Input is borrowed/readable. Output covers 64 exclusive writable bytes,
/// disjoint from input. No pointers are retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_resp_handler_key(
    data: *const u8,
    length: usize,
    output: *mut u8,
) -> usize {
    // SAFETY: `data` lends `length` initialized bytes (null only when empty) for this call.
    let Some(bytes) = (unsafe { crate::input(data, length) }) else {
        return 0;
    };
    if output.is_null() {
        return 0;
    }
    let (key, length) = resp_handler_key(bytes);
    // SAFETY: `output` is non-null (checked above) and covers 64 exclusive writable bytes,
    // disjoint from the local 64-byte `key`.
    unsafe { std::ptr::copy_nonoverlapping(key.as_ptr(), output, key.len()) };
    length
}

/// [`resp_php_next`]: 1 event, 0 end, -1 invalid. Invalid input leaves state and
/// span unchanged.
/// # Safety
/// Input is borrowed/readable; state/span are aligned exclusive writable POD,
/// initialized with zero state. Range is optional, writable, and disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_resp_php_next(
    data: *const u8,
    length: usize,
    state: Option<&mut RespPhpState>,
    span: Option<&mut RespSpan>,
    mut range: Option<&mut u8>,
) -> i32 {
    set(&mut range, 0);
    let (Some(state), Some(span)) = (state, span) else {
        return -1;
    };
    // SAFETY: `data` lends `length` initialized bytes (null only when empty) for this call.
    let Some(bytes) = (unsafe { crate::input(data, length) }) else {
        return -1;
    };
    let mut current = *state;
    let mut overflow = false;
    let result = resp_php_next(bytes, &mut current, &mut overflow);
    set(&mut range, u8::from(overflow));
    let Ok(event) = result else {
        return -1;
    };
    *state = current;
    let Some(event) = event else {
        return 0;
    };
    *span = event;
    1
}

/// Writes an optional out flag.
fn set(flag: &mut Option<&mut u8>, value: u8) {
    if let Some(flag) = flag {
        **flag = value;
    }
}

/// # Safety
/// Input is borrowed/readable. Output is aligned, disjoint and writable for
/// capacity spans; null/zero capacity performs validation and returns count.
/// Invalid input (-1) or short output (-2) leaves output unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_resp_parse(
    data: *const u8,
    length: usize,
    output: *mut RespSpan,
    capacity: usize,
) -> i64 {
    // SAFETY: forwards the caller's input and output contract; the range output is omitted.
    unsafe { swr_v1_resp_parse_range(data, length, output, capacity, None) }
}

/// The optional range output reports libc-compatible signed-long overflow,
/// including numbers whose following delimiter is malformed. Existing errno
/// belongs to native code; a zero range flag must not clear it.
/// # Safety
/// Same buffer contract as parse; range is null or exclusive writable/disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_resp_parse_range(
    data: *const u8,
    length: usize,
    output: *mut RespSpan,
    capacity: usize,
    mut range: Option<&mut u8>,
) -> i64 {
    set(&mut range, 0);
    // SAFETY: `data` lends `length` initialized bytes (null only when empty) for this call.
    let Some(bytes) = (unsafe { crate::input(data, length) }) else {
        return -1;
    };
    let mut overflow = false;
    let count = resp_spans(bytes, |_| {}, &mut overflow);
    set(&mut range, u8::from(overflow));
    let Some(count) = count else { return -1 };
    if output.is_null() {
        return if capacity == 0 {
            i64::try_from(count).unwrap_or(-1)
        } else {
            -2
        };
    }
    if capacity < count || count > isize::MAX as usize / size_of::<RespSpan>() {
        return -2;
    }
    // SAFETY: `output` is non-null (checked above) with `capacity >= count` writable slots.
    let mut slots = unsafe { crate::output(output, count) }.iter_mut();
    let _ = resp_spans(
        bytes,
        |span| {
            if let Some(slot) = slots.next() {
                *slot = span;
            }
        },
        &mut overflow,
    );
    i64::try_from(count).unwrap_or(-1)
}

/// Scan once, writing at most capacity spans and returning the full count.
/// Unlike parse, an error may leave a valid output prefix; callers consume spans
/// only after success. This supports a small native stack buffer without a size
/// query, while the original transactional parse ABI remains unchanged.
/// # Safety
/// Input/output/range are disjoint borrowed readable/exclusive writable ranges.
/// Null output requires zero capacity; range is optional.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_resp_scan(
    data: *const u8,
    length: usize,
    output: *mut RespSpan,
    capacity: usize,
    mut range: Option<&mut u8>,
) -> i64 {
    set(&mut range, 0);
    if (output.is_null() && capacity != 0)
        || capacity > isize::MAX as usize / std::mem::size_of::<RespSpan>()
    {
        return -1;
    }
    // SAFETY: `data` lends `length` initialized bytes (null only when empty) for this call.
    let Some(bytes) = (unsafe { crate::input(data, length) }) else {
        return -1;
    };
    // SAFETY: `output` has `capacity` writable slots (none when null, checked above).
    let mut slots = unsafe { crate::output(output, capacity) }.iter_mut();
    let mut overflow = false;
    let count = resp_spans(
        bytes,
        |span| {
            if let Some(slot) = slots.next() {
                *slot = span;
            }
        },
        &mut overflow,
    );
    set(&mut range, u8::from(overflow));
    count.map_or(-1, |count| i64::try_from(count).unwrap_or(-1))
}

/// [`resp_format`]. Output=null/capacity=0 queries exact size; insufficient
/// capacity writes nothing.
///
/// # Safety
/// Input bytes are readable; non-null output is exclusive, disjoint writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_resp_format(
    kind: u8,
    data: *const u8,
    length: usize,
    integer: i64,
    output: *mut u8,
    capacity: usize,
) -> usize {
    // SAFETY: `data` lends `length` initialized bytes (null only when empty) for this call.
    let Some(value) = (unsafe { crate::input(data, length) }) else {
        return 0;
    };
    if output.is_null() && capacity == 0 {
        return resp_format(kind, value, integer, None);
    }
    // SAFETY: `output` lends `capacity` exclusive writable bytes, disjoint from the inputs.
    resp_format(
        kind,
        value,
        integer,
        Some(unsafe { crate::output(output, capacity) }),
    )
}

/// # Safety
/// Buffer is borrowed/readable; request and `set_offset` are aligned exclusive
/// writable values, disjoint from the buffer and one another.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_resp_receive(
    data: *const u8,
    length: usize,
    capacity: usize,
    maximum: usize,
    request: Option<&mut RespRequest>,
    set_offset: Option<&mut u8>,
) -> i32 {
    // SAFETY: forwards the caller's input and output contract; the range output is omitted.
    unsafe { swr_v1_resp_receive_range(data, length, capacity, maximum, request, set_offset, None) }
}

/// [`resp_receive_range`]: its action, with `set_offset` written; -1 for invalid input.
/// # Safety
/// Same contract as receive; range is null or exclusive writable/disjoint.
/// Reports overflow even when a later protocol delimiter is malformed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_resp_receive_range(
    data: *const u8,
    length: usize,
    capacity: usize,
    maximum: usize,
    request: Option<&mut RespRequest>,
    set_offset: Option<&mut u8>,
    mut range: Option<&mut u8>,
) -> i32 {
    set(&mut range, 0);
    let (Some(request), Some(set_offset)) = (request, set_offset) else {
        return -1;
    };
    if length > capacity {
        return -1;
    }
    // SAFETY: `data` lends `length` initialized bytes (null only when empty) for this call.
    let Some(bytes) = (unsafe { crate::input(data, length) }) else {
        return -1;
    };
    let mut overflow = false;
    let (action, offset) = resp_receive_range(bytes, capacity, maximum, request, &mut overflow);
    set(&mut range, u8::from(overflow));
    *set_offset = u8::from(offset);
    action.cast_signed()
}

/// [`resp_extend_size`].
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_resp_extend_size(
    length: usize,
    size: usize,
    maximum: usize,
    page: usize,
) -> usize {
    resp_extend_size(length, size, maximum, page)
}

#[cfg(test)]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "tests drive the native integer interfaces with literal and loop values"
)]
mod tests {
    use super::*;
    #[test]
    fn handler_keys_reject_invalid_outputs() {
        let mut output = [99; 64];
        assert_eq!(
            unsafe { swr_v1_resp_handler_key(b"EcHo".as_ptr(), 4, output.as_mut_ptr()) },
            13
        );
        assert_eq!(&output[..14], b"_handler_echo\0");
        let mut output = [99; 64];
        assert_eq!(
            unsafe { swr_v1_resp_handler_key(std::ptr::null(), 1, output.as_mut_ptr()) },
            0
        );
        assert_eq!(output, [99; 64]);
        assert_eq!(
            unsafe { swr_v1_resp_handler_key(std::ptr::null(), 0, std::ptr::null_mut()) },
            0
        );
    }
    #[test]
    fn php_next_leaves_state_and_span_on_error() {
        let malformed = b"$9223372036854775808x\r\n";
        let mut state = RespPhpState::default();
        let mut event = RespSpan {
            kind: 99,
            ..RespSpan::default()
        };
        let mut range = 0;
        assert_eq!(
            unsafe {
                swr_v1_resp_php_next(
                    malformed.as_ptr(),
                    malformed.len(),
                    Some(&mut state),
                    Some(&mut event),
                    Some(&mut range),
                )
            },
            -1
        );
        assert_eq!((range, state.cursor.position, event.kind), (1, 0, 99));
    }
    #[test]
    fn single_scan_bounds_prefix_and_preserves_transactional_abi() {
        let wire = b":1\r\n:2\r\n:3\r\n";
        for capacity in 0..=4 {
            let mut output = [RespSpan {
                offset: 99,
                length: 77,
                integer: 42,
                kind: 3,
            }; 4];
            let mut range = 99;
            assert_eq!(
                unsafe {
                    swr_v1_resp_scan(
                        wire.as_ptr(),
                        wire.len(),
                        output.as_mut_ptr(),
                        capacity,
                        Some(&mut range),
                    )
                },
                3
            );
            assert_eq!(range, 0);
            for (index, span) in output.iter().enumerate() {
                if index < capacity.min(3) {
                    assert_eq!(span.integer, (index + 1) as i32);
                } else {
                    assert_eq!((span.offset, span.kind), (99, 3));
                }
            }
        }
        let wire = b":1\r\n:99999999999999999999999x\r\n";
        let mut output = [RespSpan::default(); 2];
        let mut range = 0;
        assert_eq!(
            unsafe {
                swr_v1_resp_scan(
                    wire.as_ptr(),
                    wire.len(),
                    output.as_mut_ptr(),
                    2,
                    Some(&mut range),
                )
            },
            -1
        );
        assert_eq!((output[0].integer, range), (1, 1));
        output[0].integer = 42;
        assert_eq!(
            unsafe { swr_v1_resp_parse(wire.as_ptr(), wire.len(), output.as_mut_ptr(), 2) },
            -1
        );
        assert_eq!(output[0].integer, 42);
    }

    /// `redis::format`, as `tests/modules/protocols/redis_reply.inc` builds it.
    fn format(kind: u8, value: &[u8]) -> Vec<u8> {
        let (data, length) = (value.as_ptr(), value.len());
        let size = unsafe { swr_v1_resp_format(kind, data, length, 0, std::ptr::null_mut(), 0) };
        let mut out = vec![0; size];
        let written = unsafe { swr_v1_resp_format(kind, data, length, 0, out.as_mut_ptr(), size) };
        assert_eq!(written, size);
        out
    }

    /// `core-tests/src/protocol/redis.cpp`, `TEST(redis, format)`: `REPLY_STATUS` (2)
    /// and `REPLY_ERROR` (0) of an empty value.
    #[test]
    fn format_empty_status_and_error() {
        assert_eq!(format(2, b""), b"+OK\r\n");
        assert_eq!(format(0, b""), b"-ERR\r\n");
    }

    /// `TEST(redis, parse)`: `redis::parse(":3\r\n")[0] == "3"`. `redis_reply.inc`'s
    /// `parse` prints an integer span (kind 1) in decimal.
    #[test]
    fn parse_integer_reply() {
        let wire = b":3\r\n";
        let mut spans = [RespSpan::default(); 16];
        let mut range = 0;
        let count = unsafe {
            swr_v1_resp_scan(
                wire.as_ptr(),
                wire.len(),
                spans.as_mut_ptr(),
                16,
                Some(&mut range),
            )
        };
        assert_eq!((count, range), (1, 0));
        assert_eq!(spans[0].kind, 1);
        assert_eq!(spans[0].integer.to_string(), "3");
    }
}
