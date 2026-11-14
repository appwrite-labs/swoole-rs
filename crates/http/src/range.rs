//! Incremental legacy byte-range parsing for the owned static HTTP context.
use super::HttpSpan;

/// One parsed part, completion, or a header whose unit must be ignored.
pub(crate) enum RangeStep {
    Part(HttpSpan),
    Done,
    Ignore,
}

#[derive(Default)]
pub(crate) struct HttpRangeState {
    cursor: usize,
    total: usize,
    started: bool,
    done: bool,
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
fn next(input: &[u8], size: usize, state: &mut HttpRangeState) -> Result<RangeStep, ()> {
    if state.cursor > input.len() {
        return Err(());
    }
    if state.done {
        return Ok(RangeStep::Done);
    }
    loop {
        let mut at = state.cursor;
        spaces(input, &mut at);
        let (start, mut end);
        if input.get(at) == Some(&b'-') {
            at += 1;
            let suffix = decimal(input, &mut at).ok_or(())?;
            spaces(input, &mut at);
            if !matches!(input.get(at), None | Some(b',' | b'\r')) {
                return Err(());
            }
            start = size.saturating_sub(suffix);
            end = size;
        } else {
            start = decimal(input, &mut at).ok_or(())?;
            spaces(input, &mut at);
            if input.get(at) != Some(&b'-') {
                return Err(());
            }
            at += 1;
            spaces(input, &mut at);
            if matches!(input.get(at), None | Some(b',')) {
                end = size;
            } else {
                end = decimal(input, &mut at).ok_or(())?;
                spaces(input, &mut at);
                if !matches!(input.get(at), None | Some(b',' | b'\r')) {
                    return Err(());
                }
                end = if end >= size { size } else { end + 1 };
            }
        }
        if input.get(at) == Some(&b',') && !matches!(input.get(at + 1), None | Some(b'\r')) {
            state.cursor = at + 1;
        } else {
            state.cursor = at;
            state.done = true;
        }
        if start < end {
            let length = end - start;
            state.total = state.total.checked_add(length).ok_or(())?;
            return Ok(RangeStep::Part(HttpSpan {
                offset: start,
                length,
            }));
        }
        if start == 0 {
            state.done = true;
        }
        if state.done {
            return Ok(RangeStep::Done);
        }
    }
}
impl HttpRangeState {
    pub(crate) fn step(&mut self, input: &[u8], size: usize) -> Result<RangeStep, ()> {
        if !self.started {
            self.started = true;
            if input.is_empty() {
                self.done = true;
                return Ok(RangeStep::Done);
            }
            if !crate::case_prefix(input, b"bytes=") {
                self.done = true;
                return Ok(RangeStep::Ignore);
            }
            self.cursor = 6;
        }
        let step = next(input, size, self);
        self.done |= step.is_err();
        step
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::INVALID;
    fn parse(input: &[u8], size: usize) -> (i32, Vec<(usize, usize)>) {
        let mut state = HttpRangeState::default();
        let mut parts = Vec::new();
        loop {
            match state.step(input, size) {
                Ok(RangeStep::Part(part)) => parts.push((part.offset, part.length)),
                Ok(RangeStep::Done) => return (0, parts),
                Ok(RangeStep::Ignore) => return (2, parts),
                Err(()) => return (INVALID, parts),
            }
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
