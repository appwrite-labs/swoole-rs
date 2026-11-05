//! RESP parsing preserves the pinned command parser's number conversion and
//! empty/null bulk behavior. Borrowed spans leave string ownership native.
#[derive(Clone, Copy, Debug)]
struct Number {
    value: i32,
    consumed: usize,
}

fn number(bytes: &[u8], range: &mut bool) -> Option<Number> {
    let bytes = bytes.get(1..)?;
    let mut end = 0;
    while bytes
        .get(end)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | 11 | 12 | b'\r'))
    {
        end += 1;
    }
    let negative = bytes.get(end) == Some(&b'-');
    if matches!(bytes.get(end), Some(b'+' | b'-')) {
        end += 1;
    }
    let start = end;
    let limit = if negative {
        (i64::MAX as u64) + 1
    } else {
        i64::MAX as u64
    };
    let mut value = 0u64;
    while let Some(byte) = bytes.get(end).filter(|byte| byte.is_ascii_digit()) {
        let digit = u64::from(*byte - b'0');
        if value > (limit - digit) / 10 {
            *range = true;
        }
        value = value.saturating_mul(10).saturating_add(digit).min(limit);
        end += 1;
    }
    if end == start {
        end = 0;
    }
    if bytes.get(end..end + 2) != Some(b"\r\n") {
        return None;
    }
    let signed = if negative {
        value.wrapping_neg().cast_signed()
    } else {
        value.cast_signed()
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "C's int ret = strtol(..) narrows the long"
    )]
    let value = signed as i32;
    Some(Number {
        value,
        consumed: end + 3,
    })
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RespSpan {
    pub offset: usize,
    pub length: usize,
    pub integer: i32,
    pub kind: u32,
}

/// Render the native PHP handler property key: ASCII lowercase `_handler_`,
/// stopping at the command's first NUL and truncating to 63 bytes. The native
/// caller keeps raw command-length validation and Zend registry ownership.
/// Returns key length, or zero on invalid input; always terminates on success.
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
    let mut key = [0u8; 64];
    let prefix = b"_handler_";
    let name = bytes
        .iter()
        .take(63 - prefix.len())
        .take_while(|&&byte| byte != 0)
        .map(u8::to_ascii_lowercase);
    let mut length = 0;
    for (slot, byte) in key.iter_mut().zip(prefix.iter().copied().chain(name)) {
        *slot = byte;
        length += 1;
    }
    // SAFETY: `output` is non-null (checked above) and covers 64 exclusive writable bytes,
    // disjoint from the local 64-byte `key`.
    unsafe { std::ptr::copy_nonoverlapping(key.as_ptr(), output, key.len()) };
    length
}

/// Borrowed RESP walk state. Native callers may allocate between events; no
/// Rust frame or allocation survives a call. All-zero initializes a new walk.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RespCursor {
    pub position: usize,
    pub length: i32,
    pub state: u8,
}

impl RespCursor {
    #[inline]
    fn next(&mut self, bytes: &[u8], range: &mut bool) -> Result<Option<RespSpan>, ()> {
        if self.state > 2 {
            return Err(());
        }
        while let Some(rest) = bytes.get(self.position..)
            && let Some(&first) = rest.first()
        {
            if self.state == 0 && first == b'*' {
                self.position += number(rest, range).ok_or(())?.consumed;
                self.state = 1;
            } else if self.state <= 1 && first == b'$' {
                let value = number(rest, range).ok_or(())?;
                self.position += value.consumed;
                if value.value == -1 {
                    return Ok(Some(RespSpan {
                        kind: 2,
                        ..RespSpan::default()
                    }));
                }
                self.length = value.value;
                self.state = 2;
            } else if self.state <= 1 && first == b':' {
                let value = number(rest, range).ok_or(())?;
                self.position += value.consumed;
                return Ok(Some(RespSpan {
                    integer: value.value,
                    kind: 1,
                    ..RespSpan::default()
                }));
            } else {
                // Native falls through for unrecognized prefixes, initially
                // producing empty strings. Its two trailer bytes are unchecked.
                let length = usize::try_from(self.length).map_err(|_| ())?;
                let end = self.position.checked_add(length).ok_or(())?;
                if end > bytes.len() {
                    return Err(());
                }
                let span = RespSpan {
                    offset: self.position,
                    length,
                    ..RespSpan::default()
                };
                self.position = end.checked_add(2).ok_or(())?;
                self.state = 1;
                return Ok(Some(span));
            }
        }
        Ok(None)
    }
}

fn scan_range(bytes: &[u8], mut emit: impl FnMut(RespSpan), range: &mut bool) -> Option<usize> {
    let mut cursor = RespCursor::default();
    let mut count = 0;
    while let Some(span) = cursor.next(bytes, range).ok()? {
        // The native vector parser omits nulls; PHP keeps them as parameters.
        if span.kind != 2 {
            emit(span);
            count += 1;
        }
    }
    Some(count)
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RespPhpState {
    pub cursor: RespCursor,
    pub command_seen: u8,
}

/// Return one PHP request event: 1 event, 0 end, -1 invalid. Span kinds are
/// 0 string parameter, 1 integer, 2 null, 3 first string (command). Integers and
/// nulls preceding the command remain parameters. Range reports native ERANGE
/// without touching errno. Invalid input leaves state and span unchanged.
/// # Safety
/// Input is borrowed/readable; state/span are aligned exclusive writable POD,
/// initialized with zero state. Range is optional, writable, and disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_resp_php_next(
    data: *const u8,
    length: usize,
    state: *mut RespPhpState,
    span: *mut RespSpan,
    range: *mut u8,
) -> i32 {
    if !range.is_null() {
        // SAFETY: `range` is non-null (checked above), aligned and writable per the export's contract.
        unsafe { range.write(0) };
    }
    if state.is_null() || span.is_null() {
        return -1;
    }
    // SAFETY: `data` lends `length` initialized bytes (null only when empty) for this call.
    let Some(bytes) = (unsafe { crate::input(data, length) }) else {
        return -1;
    };
    // SAFETY: `state` is non-null (checked above), aligned and initialized per the export's contract.
    let mut current = unsafe { state.read() };
    if current.command_seen > 1 {
        return -1;
    }
    let mut overflow = false;
    let result = current.cursor.next(bytes, &mut overflow);
    if !range.is_null() {
        // SAFETY: `range` is non-null (checked above), aligned and writable per the export's contract.
        unsafe { range.write(u8::from(overflow)) };
    }
    match result {
        Ok(Some(mut event)) => {
            if event.kind == 0 && current.command_seen == 0 {
                event.kind = 3;
                current.command_seen = 1;
            }
            // SAFETY: `state` is non-null (checked above), aligned, exclusive and writable.
            unsafe { state.write(current) };
            // SAFETY: `span` is non-null (checked above), aligned, exclusive and writable.
            unsafe { span.write(event) };
            1
        }
        Ok(None) => {
            // SAFETY: `state` is non-null (checked above), aligned and writable per the export's contract.
            unsafe { state.write(current) };
            0
        }
        Err(()) => -1,
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
    unsafe { swr_v1_resp_parse_range(data, length, output, capacity, std::ptr::null_mut()) }
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
    range: *mut u8,
) -> i64 {
    if !range.is_null() {
        // SAFETY: `range` is non-null (checked above), aligned and writable per the export's contract.
        unsafe { range.write(0) };
    }
    // SAFETY: `data` lends `length` initialized bytes (null only when empty) for this call.
    let Some(bytes) = (unsafe { crate::input(data, length) }) else {
        return -1;
    };
    let mut overflow = false;
    let count = scan_range(bytes, |_| {}, &mut overflow);
    if !range.is_null() {
        // SAFETY: `range` is non-null (checked above), aligned and writable per the export's contract.
        unsafe { range.write(u8::from(overflow)) };
    }
    let Some(count) = count else { return -1 };
    if output.is_null() {
        return if capacity == 0 {
            i64::try_from(count).unwrap_or(-1)
        } else {
            -2
        };
    }
    if capacity < count || count > isize::MAX as usize / std::mem::size_of::<RespSpan>() {
        return -2;
    }
    let mut index = 0;
    scan_range(
        bytes,
        |span| {
            // SAFETY: the scan emits exactly `count <= capacity` spans, and `output` is
            // non-null (checked above) with `capacity` writable slots.
            unsafe { output.wrapping_add(index).write(span) };
            index += 1;
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
    range: *mut u8,
) -> i64 {
    if !range.is_null() {
        // SAFETY: `range` is non-null (checked above), aligned and writable per the export's contract.
        unsafe { range.write(0) };
    }
    if (output.is_null() && capacity != 0)
        || capacity > isize::MAX as usize / std::mem::size_of::<RespSpan>()
    {
        return -1;
    }
    // SAFETY: `data` lends `length` initialized bytes (null only when empty) for this call.
    let Some(bytes) = (unsafe { crate::input(data, length) }) else {
        return -1;
    };
    let mut overflow = false;
    let mut index = 0;
    let count = scan_range(
        bytes,
        |span| {
            if index < capacity {
                // SAFETY: `index < capacity`; `output` is non-null whenever `capacity != 0`
                // (checked above) and has `capacity` writable slots.
                unsafe { output.wrapping_add(index).write(span) };
            }
            index += 1;
        },
        &mut overflow,
    );
    if !range.is_null() {
        // SAFETY: `range` is non-null (checked above), aligned and writable per the export's contract.
        unsafe { range.write(u8::from(overflow)) };
    }
    count.map_or(-1, |count| i64::try_from(count).unwrap_or(-1))
}

pub(crate) fn decimal(mut value: u64, buffer: &mut [u8; 20]) -> &[u8] {
    let mut offset = buffer.len();
    for slot in buffer.iter_mut().rev() {
        *slot = b'0' + (value % 10) as u8;
        offset -= 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    buffer.get(offset..).unwrap_or_default()
}

/// kind=0 error,2 status,4 bulk,7 explicit nil,8 integer. Other string overloads
/// produce no output. Integer formatting ignores the native `ReplyType`.
/// Output=null/capacity=0 queries exact size; insufficient capacity writes nothing.
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
    let mut digits = [0u8; 20];
    let (prefix, body, suffix): (&[u8], &[u8], &[u8]) = match kind {
        0 | 2 => {
            let body = if value.is_empty() {
                if kind == 0 { &b"ERR"[..] } else { &b"OK"[..] }
            } else {
                value.split(|b| *b == 0).next().unwrap_or_default()
            };
            (if kind == 0 { b"-" } else { b"+" }, body, b"\r\n")
        }
        4 if !value.is_empty() => (decimal(length as u64, &mut digits), value, b"\r\n"),
        7 => (b"", b"$-1", b"\r\n"),
        8 => (
            if integer < 0 { b":-" } else { b":" },
            decimal(integer.unsigned_abs(), &mut digits),
            b"\r\n",
        ),
        _ => return 0,
    };
    // Bulk strings wrap their length prefix in '$' and a header CRLF.
    let (dollar, header): (&[u8], &[u8]) = if kind == 4 {
        (b"$", b"\r\n")
    } else {
        (b"", b"")
    };
    let parts = [dollar, prefix, header, body, suffix];
    let Some(size) = parts
        .iter()
        .try_fold(0usize, |size, part| size.checked_add(part.len()))
    else {
        return 0;
    };
    if output.is_null() && capacity == 0 {
        return size;
    }
    // SAFETY: `output` lends `capacity` exclusive writable bytes, disjoint from the inputs.
    let Some(out) = (unsafe { crate::output(output, capacity, size) }) else {
        return 0;
    };
    crate::concat(out, &parts);
    size
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RespRequest {
    pub state: u8,
    pub lines_total: i32,
    pub lines_received: i32,
    pub bytes_total: i32,
    pub bytes_received: i32,
    pub offset: i32,
}

/// Legacy receive-state policy. Returns 0 malformed,1 read again,2 wait for next
/// event,3 dispatch whole buffer,4 package too big. Native does I/O/growth/callbacks.
pub fn resp_receive(
    bytes: &[u8],
    capacity: usize,
    maximum: usize,
    request: &mut RespRequest,
) -> (u32, bool) {
    resp_receive_range(bytes, capacity, maximum, request, &mut false)
}
/// Size to extend a RESP receive buffer to after action 1 (upstream 0c954199e):
/// 0 unless it is full and below the maximum; the maximum once it is half way;
/// otherwise double, then `swoole_size_align` (which always adds up to a whole
/// page, even to an aligned value) and cap.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_resp_extend_size(
    length: usize,
    size: usize,
    maximum: usize,
    page: usize,
) -> usize {
    if length != size || size >= maximum {
        0
    } else if size >= maximum / 2 {
        maximum
    } else {
        let doubled = size.saturating_mul(2);
        let aligned = if page == 0 {
            doubled
        } else {
            doubled.saturating_add(page - doubled % page)
        };
        aligned.min(maximum)
    }
}

fn resp_receive_range(
    bytes: &[u8],
    capacity: usize,
    maximum: usize,
    request: &mut RespRequest,
    range: &mut bool,
) -> (u32, bool) {
    if !bytes.ends_with(b"\r\n") {
        return (
            if capacity >= maximum && bytes.len() == capacity {
                4
            } else {
                1
            },
            false,
        );
    }
    let mut position = 0;
    let mut set_offset = false;
    loop {
        let Some(rest) = bytes.get(position..) else {
            return (0, set_offset);
        };
        let Some(&first) = rest.first() else {
            return (0, set_offset);
        };
        if request.state == 0 && first == b'*' {
            let Some(value) = number(rest, range) else {
                return (0, set_offset);
            };
            position += value.consumed;
            request.lines_total = value.value;
            request.state = 1;
        } else if request.state <= 1 && first == b'$' {
            let Some(value) = number(rest, range) else {
                return (0, set_offset);
            };
            position += value.consumed;
            if value.value < 0 {
                if position == bytes.len() {
                    return (0, set_offset);
                }
                continue;
            }
            if (value.value.unsigned_abs() as usize)
                .checked_add(position)
                .is_none_or(|n| n > maximum)
            {
                return (4, set_offset);
            }
            request.bytes_total = value.value;
            request.state = 2;
        } else if request.state <= 1 && first == b':' {
            let Some(value) = number(rest, range) else {
                return (0, set_offset);
            };
            position += value.consumed;
        } else if request.state <= 2 {
            let Some(remaining) = request.bytes_total.checked_sub(request.bytes_received) else {
                return (0, set_offset);
            };
            if (bytes.len() - position) < remaining.max(0).unsigned_abs() as usize {
                // Fewer bytes than `remaining`, an i32, are left.
                let Some(received) = i32::try_from(bytes.len() - position)
                    .ok()
                    .and_then(|left| request.bytes_received.checked_add(left))
                else {
                    return (0, set_offset);
                };
                request.bytes_received = received;
                return (2, set_offset);
            }
            let Some(next) = usize::try_from(request.bytes_total)
                .ok()
                .and_then(|n| position.checked_add(n + 2))
            else {
                return (0, set_offset);
            };
            // Past the end is fine: the line still counts, and the next pass fails.
            position = next;
            request.bytes_total = 0;
            let Some(lines) = request.lines_received.checked_add(1) else {
                return (0, set_offset);
            };
            request.lines_received = lines;
            request.state = 1;
            set_offset = true;
            if request.lines_received == request.lines_total {
                return (3, set_offset);
            }
        } else {
            return (0, set_offset);
        }
        if position >= bytes.len() {
            return (0, set_offset);
        }
    }
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
    request: *mut RespRequest,
    set_offset: *mut u8,
) -> i32 {
    // SAFETY: forwards the caller's input and output contract; the range output is omitted.
    unsafe {
        swr_v1_resp_receive_range(
            data,
            length,
            capacity,
            maximum,
            request,
            set_offset,
            std::ptr::null_mut(),
        )
    }
}
/// # Safety
/// Same contract as receive; range is null or exclusive writable/disjoint.
/// Reports overflow even when a later protocol delimiter is malformed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_resp_receive_range(
    data: *const u8,
    length: usize,
    capacity: usize,
    maximum: usize,
    request: *mut RespRequest,
    set_offset: *mut u8,
    range: *mut u8,
) -> i32 {
    if !range.is_null() {
        // SAFETY: `range` is non-null (checked above), aligned and writable per the export's contract.
        unsafe { range.write(0) };
    }
    if request.is_null() || set_offset.is_null() || length > capacity {
        return -1;
    }
    // SAFETY: `data` lends `length` initialized bytes (null only when empty) for this call.
    let Some(bytes) = (unsafe { crate::input(data, length) }) else {
        return -1;
    };
    let mut overflow = false;
    let (action, offset) = resp_receive_range(
        bytes,
        capacity,
        maximum,
        // SAFETY: `request` is non-null (checked above), aligned, exclusive and writable.
        unsafe { &mut *request },
        &mut overflow,
    );
    if !range.is_null() {
        // SAFETY: `range` is non-null (checked above), aligned and writable per the export's contract.
        unsafe { range.write(u8::from(overflow)) };
    }
    // SAFETY: `set_offset` is non-null (checked above), aligned and writable per the export's contract.
    unsafe { set_offset.write(u8::from(offset)) };
    action.cast_signed()
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
    fn extend_size_follows_upstream_growth() {
        assert_eq!(swr_v1_resp_extend_size(10, 64, 1 << 20, 4096), 0);
        assert_eq!(swr_v1_resp_extend_size(1 << 20, 1 << 20, 1 << 20, 4096), 0);
        assert_eq!(swr_v1_resp_extend_size(4096, 4096, 1 << 20, 4096), 12288);
        // Upstream's growth sequence from 8192 with an 8 MiB maximum.
        let mut size = 8192;
        let mut sizes = vec![size];
        while size < 8 << 20 {
            size = swr_v1_resp_extend_size(size, size, 8 << 20, 4096);
            sizes.push(size);
        }
        assert_eq!(
            sizes,
            [
                8192, 20480, 45056, 94208, 192_512, 389_120, 782_336, 1_568_768, 3_141_632,
                6_287_360, 8_388_608
            ]
        );
        assert_eq!(swr_v1_resp_extend_size(5000, 5000, 1 << 20, 4096), 12288);
        assert_eq!(
            swr_v1_resp_extend_size(600_000, 600_000, 1 << 20, 4096),
            1 << 20
        );
        assert_eq!(
            swr_v1_resp_extend_size(300_000, 300_000, 500_000, 4096),
            500_000
        );
    }
    #[test]
    fn handler_keys_preserve_nul_truncation_ascii_and_invalid_outputs() {
        for input in [
            b"".as_slice(),
            b"EcHo\0ignored",
            &[b'A'; 1024],
            &[255, b'A', 128],
        ] {
            let mut output = [99; 64];
            let length = unsafe {
                swr_v1_resp_handler_key(input.as_ptr(), input.len(), output.as_mut_ptr())
            };
            let expected = [
                b"_handler_".as_slice(),
                &input[..input.iter().position(|&b| b == 0).unwrap_or(input.len())],
            ]
            .concat();
            let expected: Vec<_> = expected
                .iter()
                .take(63)
                .map(u8::to_ascii_lowercase)
                .collect();
            assert_eq!(&output[..length], expected);
            assert!(output[length..].iter().all(|&byte| byte == 0));
        }
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
    fn php_cursor_preserves_typed_order_and_transactional_errors() {
        let wire = b"*2\r\n$-1\r\n:9223372036854775808\r\n$4\r\nECHO\r\n$0\r\n\r\n";
        let mut state = RespPhpState::default();
        let mut event = RespSpan::default();
        let mut range = 0;
        let mut kinds = Vec::new();
        let mut ranges = Vec::new();
        while unsafe {
            swr_v1_resp_php_next(
                wire.as_ptr(),
                wire.len(),
                &raw mut state,
                &raw mut event,
                &raw mut range,
            )
        } == 1
        {
            kinds.push(event.kind);
            ranges.push(range);
            if event.kind == 1 {
                assert_eq!(event.integer, -1);
            }
            if event.kind == 3 {
                assert_eq!(&wire[event.offset..event.offset + event.length], b"ECHO");
            }
        }
        assert_eq!(kinds, [2, 1, 3, 0]);
        assert_eq!(ranges, [0, 1, 0, 0]);
        let malformed = b"$9223372036854775808x\r\n";
        state = RespPhpState::default();
        event.kind = 99;
        assert_eq!(
            unsafe {
                swr_v1_resp_php_next(
                    malformed.as_ptr(),
                    malformed.len(),
                    &raw mut state,
                    &raw mut event,
                    &raw mut range,
                )
            },
            -1
        );
        assert_eq!(range, 1);
        assert_eq!(state.cursor.position, 0);
        assert_eq!(state.cursor.state, 0);
        assert_eq!(state.command_seen, 0);
        assert_eq!(event.kind, 99);
    }
    fn scan(bytes: &[u8], emit: impl FnMut(RespSpan)) -> Option<usize> {
        scan_range(bytes, emit, &mut false)
    }
    #[test]
    fn number_grammar_matches_strtol_int_conversion() {
        for (wire, expected) in [
            ("*\r\n", 0),
            ("* +12\r\n", 12),
            ("*\x0b42\r\n", 42),
            ("*4294967295\r\n", -1),
            ("*9223372036854775808\r\n", -1),
            ("*-9223372036854775809\r\n", 0),
        ] {
            assert_eq!(number(wire.as_bytes(), &mut false).unwrap().value, expected);
        }
        assert!(number(b"* \r\n", &mut false).is_none());
    }
    #[test]
    fn overflow_is_reported_even_when_the_delimiter_fails() {
        for (wire, expected) in [
            (":9223372036854775807\r\n", false),
            (":9223372036854775808\r\n", true),
            (":-9223372036854775808\r\n", false),
            (":-9223372036854775809x\r\n", true),
            (":00000000000000000000000001\r\n", false),
        ] {
            let mut range = 99;
            unsafe {
                swr_v1_resp_parse_range(
                    wire.as_ptr(),
                    wire.len(),
                    std::ptr::null_mut(),
                    0,
                    &raw mut range,
                );
            }
            assert_eq!(range, u8::from(expected), "{wire:?}");
        }
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
                        &raw mut range,
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
                    &raw mut range,
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
    #[test]
    fn null_empty_binary_and_integer_commands() {
        let mut spans = Vec::new();
        let wire = b"*4\r\n$3\r\nGET\r\n$-1\r\n$0\r\n\r\n:4294967295\r\n";
        assert_eq!(scan(wire, |span| spans.push(span)), Some(3));
        assert_eq!(
            &wire[spans[0].offset..spans[0].offset + spans[0].length],
            b"GET"
        );
        assert_eq!(spans[1].length, 0);
        assert_eq!(spans[2].integer, -1);
        assert_eq!(scan(b"$99\r\nx\r\n", |_| {}), None);
    }
    #[test]
    fn full_request_and_partial_receive_state() {
        let mut request = RespRequest::default();
        assert_eq!(
            resp_receive(b"*1\r\n$3\r\nGET\r\n", 64, 64, &mut request),
            (3, true)
        );
        let mut request = RespRequest::default();
        assert_eq!(
            resp_receive(b"*1\r\n$10\r\nabc\r\n", 64, 64, &mut request),
            (2, false)
        );
        assert_eq!(request.state, 2);
        assert_eq!(request.bytes_received, 5);
        assert_eq!(
            resp_receive(b"abcd", 4, 4, &mut RespRequest::default()),
            (4, false)
        );
    }
    #[test]
    fn last_line_counts_when_its_crlf_runs_past_the_end() {
        // Upstream: a byte that isn't `$` or `:` is an empty string, and the line
        // completes the request even though its CRLF would end past the buffer.
        assert_eq!(
            resp_receive(b"*2\r\n\n\r\n", 64, 64, &mut RespRequest::default()),
            (3, true)
        );
        assert_eq!(
            resp_receive(b"*3\r\n\n\r\n", 64, 64, &mut RespRequest::default()),
            (0, true)
        );
    }
}
