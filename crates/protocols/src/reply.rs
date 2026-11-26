//! PHP RESP replies. Zend conversion and recursion stay with the caller.

/// A reply with only the payload its wire representation needs.
#[derive(Clone, Copy, Debug)]
pub enum Reply<'a> {
    Error(Option<&'a [u8]>),
    Nil,
    Status(Option<&'a [u8]>),
    Integer(i64),
    Bulk(&'a [u8]),
    Set(u32),
    Map(u32),
    StringKey(&'a [u8]),
    IntegerKey(u64),
}

fn c_text(value: &[u8]) -> &[u8] {
    value.split(|&byte| byte == 0).next().unwrap_or_default()
}

impl Reply<'_> {
    /// Append the PHP reply's bytes. Present empty status/error stays empty;
    /// bulk strings are binary, while text and string map keys stop at NUL.
    pub fn append_to(self, output: &mut Vec<u8>) {
        self.with_parts(|parts| {
            for part in parts {
                #[cfg_attr(protocol_buffer_lint, expect(clippy::disallowed_methods, reason = "each reply copies its payload once plus at most 25 RESP framing bytes"))]
                output.extend_from_slice(part);
            }
        });
    }

    fn with_parts<R>(self, write: impl FnOnce(&[&[u8]]) -> R) -> R {
        let mut digits = [0; 20];
        match self {
            Self::Error(value) => write(&[b"-", c_text(value.unwrap_or(b"ERR")), b"\r\n"]),
            Self::Status(value) => write(&[b"+", c_text(value.unwrap_or(b"OK")), b"\r\n"]),
            Self::Nil => write(&[b"$-1\r\n"]),
            Self::Integer(value) => write(&[
                if value < 0 { b":-" } else { b":" },
                decimal(value.unsigned_abs(), &mut digits),
                b"\r\n",
            ]),
            Self::Bulk(value) | Self::StringKey(value) => write(&[
                b"$",
                decimal(value.len() as u64, &mut digits),
                b"\r\n",
                // A string map key declares its entire length but stops at NUL.
                if matches!(self, Self::StringKey(_)) {
                    c_text(value)
                } else {
                    value
                },
                b"\r\n",
            ]),
            Self::Set(count) | Self::Map(count) => {
                let count = count
                    .wrapping_mul(if matches!(self, Self::Map(_)) { 2 } else { 1 })
                    .cast_signed();
                write(&[
                    if count < 0 { b"*-" } else { b"*" },
                    decimal(u64::from(count.unsigned_abs()), &mut digits),
                    b"\r\n",
                ])
            }
            Self::IntegerKey(value) => Reply::Bulk(decimal(value, &mut digits)).with_parts(write),
        }
    }
}

fn decimal(mut value: u64, buffer: &mut [u8; 20]) -> &[u8] {
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
    let text = (!value.is_empty()).then_some(value);
    let reply = match kind {
        0 => Reply::Error(text),
        2 => Reply::Status(text),
        4 if !value.is_empty() => Reply::Bulk(value),
        7 => Reply::Nil,
        8 => Reply::Integer(integer),
        _ => return 0,
    };
    reply.with_parts(|parts| {
        let Some(size) = parts
            .iter()
            .try_fold(0usize, |n, part| n.checked_add(part.len()))
        else {
            return 0;
        };
        let Some(output) = output else { return size };
        let Some(out) = output.get_mut(..size) else {
            return 0;
        };
        crate::concat(out, parts);
        size
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn encode(reply: Reply<'_>) -> Vec<u8> {
        let mut bytes = Vec::new();
        reply.append_to(&mut bytes);
        bytes
    }
    #[test]
    fn php_defaults_empty_binary_and_map_key_policy() {
        assert_eq!(encode(Reply::Error(None)), b"-ERR\r\n");
        assert_eq!(encode(Reply::Status(None)), b"+OK\r\n");
        assert_eq!(encode(Reply::Error(Some(b""))), b"-\r\n");
        assert_eq!(encode(Reply::Status(Some(b"a\0b"))), b"+a\r\n");
        assert_eq!(encode(Reply::Bulk(b"")), b"$0\r\n\r\n");
        assert_eq!(encode(Reply::Bulk(b"a\0b")), b"$3\r\na\0b\r\n");
        assert_eq!(encode(Reply::StringKey(b"k\0tail")), b"$6\r\nk\r\n");
        assert_eq!(encode(Reply::Nil), b"$-1\r\n");
    }
    #[test]
    fn signed_unsigned_and_native_count_wrapping() {
        for value in [i64::MIN, -1, 0, 1, i64::MAX] {
            assert_eq!(
                encode(Reply::Integer(value)),
                format!(":{value}\r\n").as_bytes()
            );
        }
        assert_eq!(
            encode(Reply::IntegerKey(u64::MAX)),
            b"$20\r\n18446744073709551615\r\n"
        );
        assert_eq!(encode(Reply::Set(u32::MAX)), b"*-1\r\n");
        assert_eq!(encode(Reply::Map(1 << 31)), b"*0\r\n");
        assert_eq!(encode(Reply::Map((1 << 30) + 1)), b"*-2147483646\r\n");
    }
}
