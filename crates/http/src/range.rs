//! Incremental legacy byte-range parsing; native constructs response headers/tasks.
use super::{HttpSpan, INVALID};
#[repr(C)]
#[derive(Default)]
pub(crate) struct HttpRangeState {
    pub(crate) cursor: usize,
    pub(crate) total: usize,
    pub(crate) started: u32,
    pub(crate) done: u32,
}
fn decimal(input: &[u8], at: &mut usize) -> Option<usize> {
    let start = *at;
    let mut value = 0usize;
    while let Some(byte @ b'0'..=b'9') = input.get(*at).copied() {
        value = value
            .checked_mul(10)?
            .checked_add(usize::from(byte - b'0'))?;
        *at += 1;
    }
    (*at != start).then_some(value)
}
fn spaces(input: &[u8], at: &mut usize) {
    while input.get(*at) == Some(&b' ') {
        *at += 1;
    }
}
fn next(input: &[u8], size: usize, state: &mut HttpRangeState) -> Result<Option<HttpSpan>, i32> {
    if state.cursor > input.len() {
        return Err(INVALID);
    }
    if state.done != 0 {
        return Ok(None);
    }
    loop {
        let mut at = state.cursor;
        spaces(input, &mut at);
        let (start, mut end);
        if input.get(at) == Some(&b'-') {
            at += 1;
            let suffix = decimal(input, &mut at).ok_or(INVALID)?;
            spaces(input, &mut at);
            if !matches!(input.get(at), None | Some(b',' | b'\r')) {
                return Err(INVALID);
            }
            start = size.saturating_sub(suffix);
            end = size;
        } else {
            start = decimal(input, &mut at).ok_or(INVALID)?;
            spaces(input, &mut at);
            if input.get(at) != Some(&b'-') {
                return Err(INVALID);
            }
            at += 1;
            spaces(input, &mut at);
            if matches!(input.get(at), None | Some(b',')) {
                end = size;
            } else {
                end = decimal(input, &mut at).ok_or(INVALID)?;
                spaces(input, &mut at);
                if !matches!(input.get(at), None | Some(b',' | b'\r')) {
                    return Err(INVALID);
                }
                end = if end >= size { size } else { end + 1 };
            }
        }
        if input.get(at) == Some(&b',') && !matches!(input.get(at + 1), None | Some(b'\r')) {
            state.cursor = at + 1;
        } else {
            state.cursor = at;
            state.done = 1;
        }
        if start < end {
            let length = end - start;
            state.total = state.total.checked_add(length).ok_or(INVALID)?;
            return Ok(Some(HttpSpan {
                offset: start,
                length,
            }));
        }
        if start == 0 {
            state.done = 1;
        }
        if state.done != 0 {
            return Ok(None);
        }
    }
}
impl HttpRangeState {
    pub(crate) fn step(&mut self, input: &[u8], size: usize, span: &mut HttpSpan) -> i32 {
        *span = HttpSpan::default();
        if self.started == 0 {
            self.started = 1;
            if input.is_empty() {
                self.done = 1;
                return 0;
            }
            if !crate::case_prefix(input, b"bytes=") {
                self.done = 1;
                return 2;
            }
            self.cursor = 6;
        }
        match next(input, size, self) {
            Ok(Some(part)) => {
                *span = part;
                1
            }
            Ok(None) => 0,
            Err(status) => {
                self.done = 1;
                status
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn parse(input: &[u8], size: usize) -> (i32, Vec<(usize, usize)>) {
        let mut state = HttpRangeState::default();
        let mut output = HttpSpan::default();
        let mut parts = Vec::new();
        loop {
            let status = state.step(input, size, &mut output);
            if status != 1 {
                return (status, parts);
            }
            parts.push((output.offset, output.length));
        }
    }
    #[test]
    fn legacy_ranges_include_skips_suffixes_and_partial_errors() {
        assert_eq!(
            parse(b"bytes=0-0, 5-9, -2", 10),
            (0, vec![(0, 1), (5, 5), (8, 2)])
        );
        assert_eq!(parse(b"bytes=100-200,2-", 10), (0, vec![(2, 8)]));
        assert_eq!(parse(b"bytes=0-1,bad", 10), (INVALID, vec![(0, 2)]));
        assert_eq!(parse(b"bytes=-0", 10), (0, vec![]));
        assert_eq!(parse(b"items=0-1", 10), (2, vec![]));
        assert_eq!(parse(b"bytes=0-1,", 10), (0, vec![(0, 2)]));
        assert_eq!(parse(b"bytes=-1", 0), (0, vec![]));
    }
    #[test]
    fn decimal_and_total_overflow_do_not_emit_a_part() {
        let huge = format!("bytes=0-{},0-1", usize::MAX);
        assert_eq!(
            parse(huge.as_bytes(), usize::MAX),
            (INVALID, vec![(0, usize::MAX)])
        );
        assert_eq!(
            parse(b"bytes=999999999999999999999999999999-", 10),
            (INVALID, vec![])
        );
    }
}
