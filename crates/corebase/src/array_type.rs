//! Byte ranges in a Swoole typed-array definition.

/// An omitted key denotes a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    List,
    String,
    Int,
}

/// Offsets are relative to the start of this (possibly nested) definition.
/// The original header stores them as unsigned 16-bit integers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Definition {
    pub key: Key,
    pub offset: u16,
    pub len: u16,
}

impl Definition {
    /// The value name in the bytes beginning at this definition.
    #[must_use]
    pub fn value(self, text: &[u8]) -> &[u8] {
        let start = usize::from(self.offset);
        text.get(start..start + usize::from(self.len))
            .unwrap_or_default()
    }
}

/// Split a definition as upstream's `ArrayTypeValue::parse` does. Its comma
/// search ends at the first NUL even when the supplied length ends earlier.
///
/// # Errors
/// An invalid key returns the absolute position just past its comma.
pub fn split(text: &[u8], start: usize, len: usize) -> Result<Definition, usize> {
    let text = text.get(start..).unwrap_or_default();
    let (key, offset) = match text
        .iter()
        .take_while(|&&b| b != 0)
        .position(|&b| b == b',')
    {
        Some(comma) => {
            let name = text.get(1..comma).unwrap_or_default();
            let key = if name.eq_ignore_ascii_case(b"string") {
                Key::String
            } else if name.eq_ignore_ascii_case(b"int") {
                Key::Int
            } else {
                return Err(start + comma + 1);
            };
            (key, u16::try_from(comma + 1).unwrap_or(u16::MAX))
        }
        None => (Key::List, 1),
    };
    let len = u16::try_from(len)
        .unwrap_or(u16::MAX)
        .wrapping_sub(offset)
        .wrapping_sub(1);
    Ok(Definition { key, offset, len })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_key_and_value_until_the_first_nul() {
        for (text, key, value) in [
            (&b"<int>"[..], Key::List, &b"int"[..]),
            (b"<STRING,<int>>", Key::String, b"<int>"),
            (b"<int,>", Key::Int, b""),
            (b"<int\0,string>", Key::List, b"int\0,string"),
        ] {
            let result = split(text, 0, text.len());
            assert_eq!(
                result.map(|d| (d.key, d.value(text))),
                Ok((key, value)),
                "definition {text:?}"
            );
        }
    }

    #[test]
    fn invalid_keys_report_an_absolute_offset() {
        assert_eq!(split(b"<float,int>\0", 0, 11), Err(7), "outer key");
        assert_eq!(split(b"<int,<x,int>>\0", 5, 7), Err(8), "nested key");
    }
}
