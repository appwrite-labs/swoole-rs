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

/// What a RESP span holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RespKind {
    /// A string parameter.
    #[default]
    String,
    Integer,
    Null,
    /// The first string: the command.
    Command,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RespSpan {
    pub offset: usize,
    pub length: usize,
    pub integer: i32,
    pub kind: RespKind,
}

/// Render the native PHP handler property key: ASCII lowercase `_handler_`,
/// stopping at the command's first NUL and truncating to 63 bytes. The native
/// caller keeps raw command-length validation and Zend registry ownership.
/// Returns the key, zero-filled past its length.
#[must_use]
pub fn resp_handler_key(bytes: &[u8]) -> ([u8; 64], usize) {
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
    (key, length)
}

/// Where a RESP walk is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RespPhase {
    /// Before the array header.
    #[default]
    Start,
    /// Before an item.
    Item,
    /// After a bulk string's length, before its bytes.
    Bulk,
}

/// A RESP walk's state between events. The default starts a new walk.
#[derive(Clone, Copy, Debug, Default)]
pub struct RespCursor {
    pub position: usize,
    pub length: i32,
    pub phase: RespPhase,
}

impl RespCursor {
    #[inline]
    fn next(&mut self, bytes: &[u8], range: &mut bool) -> Result<Option<RespSpan>, ()> {
        while let Some(rest) = bytes.get(self.position..)
            && let Some(&first) = rest.first()
        {
            if self.phase == RespPhase::Start && first == b'*' {
                self.position += number(rest, range).ok_or(())?.consumed;
                self.phase = RespPhase::Item;
            } else if self.phase != RespPhase::Bulk && first == b'$' {
                let value = number(rest, range).ok_or(())?;
                self.position += value.consumed;
                if value.value == -1 {
                    return Ok(Some(RespSpan {
                        kind: RespKind::Null,
                        ..RespSpan::default()
                    }));
                }
                self.length = value.value;
                self.phase = RespPhase::Bulk;
            } else if self.phase != RespPhase::Bulk && first == b':' {
                let value = number(rest, range).ok_or(())?;
                self.position += value.consumed;
                return Ok(Some(RespSpan {
                    integer: value.value,
                    kind: RespKind::Integer,
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
                self.phase = RespPhase::Item;
                return Ok(Some(span));
            }
        }
        Ok(None)
    }
}

/// Walk a whole RESP command, emitting each span but nulls, which the native vector
/// parser omits: the span count, or `None` when the command is malformed. `range`
/// is set when a number overflowed a signed long, even if a delimiter then failed.
pub fn resp_spans(bytes: &[u8], mut emit: impl FnMut(RespSpan), range: &mut bool) -> Option<usize> {
    let mut cursor = RespCursor::default();
    let mut count = 0;
    while let Some(span) = cursor.next(bytes, range).ok()? {
        // The native vector parser omits nulls; PHP keeps them as parameters.
        if span.kind != RespKind::Null {
            emit(span);
            count += 1;
        }
    }
    Some(count)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RespPhpState {
    pub cursor: RespCursor,
    pub command_seen: bool,
}

/// A RESP command that doesn't parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Malformed;

/// One PHP request event, or `None` at the end. The first string is the
/// [`RespKind::Command`]; integers and nulls preceding it remain parameters. `range` reports native ERANGE without touching errno.
/// Invalid input leaves `state` unchanged.
///
/// # Errors
/// [`Malformed`] when the command doesn't parse.
pub fn resp_php_next(
    bytes: &[u8],
    state: &mut RespPhpState,
    range: &mut bool,
) -> Result<Option<RespSpan>, Malformed> {
    let mut current = *state;
    let event = current.cursor.next(bytes, range).map_err(|()| Malformed)?;
    let event = event.map(|mut event| {
        if event.kind == RespKind::String && !current.command_seen {
            event.kind = RespKind::Command;
            current.command_seen = true;
        }
        event
    });
    *state = current;
    Ok(event)
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
/// No output queries the exact size; one too short is left unwritten, returning 0.
#[must_use]
pub fn resp_format(kind: u8, value: &[u8], integer: i64, output: Option<&mut [u8]>) -> usize {
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
        4 if !value.is_empty() => (decimal(value.len() as u64, &mut digits), value, b"\r\n"),
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
    let Some(output) = output else {
        return size;
    };
    let Some(out) = output.get_mut(..size) else {
        return 0;
    };
    crate::concat(out, &parts);
    size
}

/// `swRedis_receive_state`: what the parser reads next. Zero is the first state, so a
/// zeroed request starts a message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum RespState {
    /// The `*N` line count.
    #[default]
    TotalLine = 0,
    /// A `$N` length or an `:N` integer.
    Length = 1,
    /// A bulk string's bytes.
    String = 2,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RespRequest {
    pub state: RespState,
    pub lines_total: i32,
    pub lines_received: i32,
    pub bytes_total: i32,
    pub bytes_received: i32,
    pub offset: i32,
}

/// What the receive loop does with its buffer next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RespAction {
    Malformed,
    /// Read again, growing the buffer if it is full.
    ReadAgain,
    /// Wait for the next readable event.
    Wait,
    /// Dispatch the whole buffer.
    Dispatch,
    TooBig,
}

/// Legacy receive-state policy, and whether the request's offset moved. The caller
/// does the I/O, growth and callbacks.
pub fn resp_receive(
    bytes: &[u8],
    capacity: usize,
    maximum: usize,
    request: &mut RespRequest,
) -> (RespAction, bool) {
    resp_receive_range(bytes, capacity, maximum, request, &mut false)
}
/// Size to extend a RESP receive buffer to after [`RespAction::ReadAgain`] (upstream 0c954199e):
/// 0 unless it is full and below the maximum; the maximum once it is half way;
/// otherwise double, then `swoole_size_align` (which always adds up to a whole
/// page, even to an aligned value) and cap.
#[must_use]
pub fn resp_extend_size(length: usize, size: usize, maximum: usize, page: usize) -> usize {
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

/// [`resp_receive`], also reporting in `range` a number that overflowed, even when a
/// later protocol delimiter is malformed.
pub fn resp_receive_range(
    bytes: &[u8],
    capacity: usize,
    maximum: usize,
    request: &mut RespRequest,
    range: &mut bool,
) -> (RespAction, bool) {
    if !bytes.ends_with(b"\r\n") {
        return (
            if capacity >= maximum && bytes.len() == capacity {
                RespAction::TooBig
            } else {
                RespAction::ReadAgain
            },
            false,
        );
    }
    let mut position = 0;
    let mut set_offset = false;
    loop {
        let Some(rest) = bytes.get(position..) else {
            return (RespAction::Malformed, set_offset);
        };
        let Some(&first) = rest.first() else {
            return (RespAction::Malformed, set_offset);
        };
        if request.state == RespState::TotalLine && first == b'*' {
            let Some(value) = number(rest, range) else {
                return (RespAction::Malformed, set_offset);
            };
            position += value.consumed;
            request.lines_total = value.value;
            request.state = RespState::Length;
        } else if request.state <= RespState::Length && first == b'$' {
            let Some(value) = number(rest, range) else {
                return (RespAction::Malformed, set_offset);
            };
            position += value.consumed;
            if value.value < 0 {
                if position == bytes.len() {
                    return (RespAction::Malformed, set_offset);
                }
                continue;
            }
            if (value.value.unsigned_abs() as usize)
                .checked_add(position)
                .is_none_or(|n| n > maximum)
            {
                return (RespAction::TooBig, set_offset);
            }
            request.bytes_total = value.value;
            request.state = RespState::String;
        } else if request.state <= RespState::Length && first == b':' {
            let Some(value) = number(rest, range) else {
                return (RespAction::Malformed, set_offset);
            };
            position += value.consumed;
        } else {
            let Some(remaining) = request.bytes_total.checked_sub(request.bytes_received) else {
                return (RespAction::Malformed, set_offset);
            };
            if (bytes.len() - position) < remaining.max(0).unsigned_abs() as usize {
                // Fewer bytes than `remaining`, an i32, are left.
                let Some(received) = i32::try_from(bytes.len() - position)
                    .ok()
                    .and_then(|left| request.bytes_received.checked_add(left))
                else {
                    return (RespAction::Malformed, set_offset);
                };
                request.bytes_received = received;
                return (RespAction::Wait, set_offset);
            }
            let Some(next) = usize::try_from(request.bytes_total)
                .ok()
                .and_then(|n| position.checked_add(n + 2))
            else {
                return (RespAction::Malformed, set_offset);
            };
            // Past the end is fine: the line still counts, and the next pass fails.
            position = next;
            request.bytes_total = 0;
            let Some(lines) = request.lines_received.checked_add(1) else {
                return (RespAction::Malformed, set_offset);
            };
            request.lines_received = lines;
            request.state = RespState::Length;
            set_offset = true;
            if request.lines_received == request.lines_total {
                return (RespAction::Dispatch, set_offset);
            }
        }
        if position >= bytes.len() {
            return (RespAction::Malformed, set_offset);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extend_size_follows_upstream_growth() {
        assert_eq!(resp_extend_size(10, 64, 1 << 20, 4096), 0);
        assert_eq!(resp_extend_size(1 << 20, 1 << 20, 1 << 20, 4096), 0);
        assert_eq!(resp_extend_size(4096, 4096, 1 << 20, 4096), 12288);
        // Upstream's growth sequence from 8192 with an 8 MiB maximum.
        let mut size = 8192;
        let mut sizes = vec![size];
        while size < 8 << 20 {
            size = resp_extend_size(size, size, 8 << 20, 4096);
            sizes.push(size);
        }
        assert_eq!(
            sizes,
            [
                8192, 20480, 45056, 94208, 192_512, 389_120, 782_336, 1_568_768, 3_141_632,
                6_287_360, 8_388_608
            ]
        );
        assert_eq!(resp_extend_size(5000, 5000, 1 << 20, 4096), 12288);
        assert_eq!(resp_extend_size(600_000, 600_000, 1 << 20, 4096), 1 << 20);
        assert_eq!(resp_extend_size(300_000, 300_000, 500_000, 4096), 500_000);
    }
    #[test]
    fn handler_keys_preserve_nul_truncation_and_ascii() {
        for input in [
            b"".as_slice(),
            b"EcHo\0ignored",
            &[b'A'; 1024],
            &[255, b'A', 128],
        ] {
            let (output, length) = resp_handler_key(input);
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
    }
    #[test]
    fn php_cursor_preserves_typed_order_and_transactional_errors() {
        let wire = b"*2\r\n$-1\r\n:9223372036854775808\r\n$4\r\nECHO\r\n$0\r\n\r\n";
        let mut state = RespPhpState::default();
        let mut kinds = Vec::new();
        let mut ranges = Vec::new();
        loop {
            let mut range = false;
            let Ok(Some(event)) = resp_php_next(wire, &mut state, &mut range) else {
                break;
            };
            kinds.push(event.kind);
            ranges.push(range);
            if event.kind == RespKind::Integer {
                assert_eq!(event.integer, -1);
            }
            if event.kind == RespKind::Command {
                assert_eq!(&wire[event.offset..event.offset + event.length], b"ECHO");
            }
        }
        assert_eq!(
            kinds,
            [
                RespKind::Null,
                RespKind::Integer,
                RespKind::Command,
                RespKind::String
            ]
        );
        assert_eq!(ranges, [false, true, false, false]);
        let malformed = b"$9223372036854775808x\r\n";
        state = RespPhpState::default();
        let mut range = false;
        assert!(resp_php_next(malformed, &mut state, &mut range).is_err());
        assert!(range);
        assert_eq!(state.cursor.position, 0);
        assert_eq!(state.cursor.phase, RespPhase::Start);
        assert!(!state.command_seen);
    }
    fn scan(bytes: &[u8], emit: impl FnMut(RespSpan)) -> Option<usize> {
        resp_spans(bytes, emit, &mut false)
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
            let mut range = false;
            let _ = resp_spans(wire.as_bytes(), |_| {}, &mut range);
            assert_eq!(range, expected, "{wire:?}");
        }
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
    fn format_queries_its_size_and_leaves_a_short_output_alone() {
        assert_eq!(resp_format(4, b"abc", 0, None), 9);
        let mut out = [0; 9];
        assert_eq!(resp_format(4, b"abc", 0, Some(&mut out)), 9);
        assert_eq!(&out, b"$3\r\nabc\r\n");
        let mut short = [7; 8];
        assert_eq!(resp_format(4, b"abc", 0, Some(&mut short)), 0);
        assert_eq!(short, [7; 8]);
        assert_eq!(resp_format(8, b"", -42, None), 6);
        assert_eq!(resp_format(3, b"x", 0, None), 0);
    }
    #[test]
    fn full_request_and_partial_receive_state() {
        let mut request = RespRequest::default();
        assert_eq!(
            resp_receive(b"*1\r\n$3\r\nGET\r\n", 64, 64, &mut request),
            (RespAction::Dispatch, true)
        );
        let mut request = RespRequest::default();
        assert_eq!(
            resp_receive(b"*1\r\n$10\r\nabc\r\n", 64, 64, &mut request),
            (RespAction::Wait, false)
        );
        assert_eq!(request.state, RespState::String);
        assert_eq!(request.bytes_received, 5);
        assert_eq!(
            resp_receive(b"abcd", 4, 4, &mut RespRequest::default()),
            (RespAction::TooBig, false)
        );
    }
    #[test]
    fn last_line_counts_when_its_crlf_runs_past_the_end() {
        // Upstream: a byte that isn't `$` or `:` is an empty string, and the line
        // completes the request even though its CRLF would end past the buffer.
        assert_eq!(
            resp_receive(b"*2\r\n\n\r\n", 64, 64, &mut RespRequest::default()),
            (RespAction::Dispatch, true)
        );
        assert_eq!(
            resp_receive(b"*3\r\n\n\r\n", 64, 64, &mut RespRequest::default()),
            (RespAction::Malformed, true)
        );
    }
}
