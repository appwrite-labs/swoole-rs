//! Parse a complete bytes range set before preparing the file sends.
use super::HttpSpan;

pub(crate) const MAX_RANGES: usize = 16;

fn trim(mut bytes: &[u8]) -> &[u8] {
    while let [b' ' | b'\t', rest @ ..] = bytes {
        bytes = rest;
    }
    while let [rest @ .., b' ' | b'\t'] = bytes {
        bytes = rest;
    }
    bytes
}

fn decimal(bytes: &[u8]) -> Option<usize> {
    if bytes.is_empty() {
        return None;
    }
    bytes.iter().try_fold(0usize, |value, byte| {
        let digit = byte.checked_sub(b'0').filter(|&digit| digit < 10)?;
        value.checked_mul(10)?.checked_add(usize::from(digit))
    })
}

fn part(bytes: &[u8], size: usize) -> Option<HttpSpan> {
    let bytes = trim(bytes);
    let (start, end) = if let Some(suffix) = bytes.strip_prefix(b"-") {
        (size.saturating_sub(decimal(suffix)?), size)
    } else {
        let mut fields = bytes.splitn(2, |&b| b == b'-');
        let start = decimal(trim(fields.next()?))?;
        let last = trim(fields.next()?);
        let end = if last.is_empty() {
            size
        } else {
            let last = decimal(last)?;
            if start > last {
                return None;
            }
            last.saturating_add(1).min(size)
        };
        (start, end)
    };
    Some(HttpSpan {
        offset: start,
        length: end.saturating_sub(start),
    })
}

/// None selects the full file. Even after a limit is exceeded, validate the
/// remaining syntax so a malformed range still gets a 416.
pub(crate) fn select<'a>(
    bytes: &[u8],
    size: usize,
    storage: &'a mut [HttpSpan; MAX_RANGES],
) -> Result<Option<&'a [HttpSpan]>, ()> {
    let mut selected = 0;
    let mut remaining = size;
    let mut excessive = false;
    for (index, bytes) in bytes.split(|&b| b == b',').enumerate() {
        let span = part(bytes, size).ok_or(())?;
        excessive |= index >= MAX_RANGES || span.length > remaining;
        if !excessive && span.length != 0 {
            remaining -= span.length;
            *storage.get_mut(selected).ok_or(())? = span;
            selected += 1;
        }
    }
    if excessive {
        Ok(None)
    } else if selected == 0 {
        Err(())
    } else {
        storage.get(..selected).map(Some).ok_or(())
    }
}
