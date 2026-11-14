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

fn number(output: &mut Vec<u8>, value: u64) {
    let mut digits = [0; 20];
    output.extend_from_slice(crate::redis::decimal(value, &mut digits));
}

fn header(output: &mut Vec<u8>, flag: u8, value: u64) {
    output.push(flag);
    number(output, value);
    output.extend_from_slice(b"\r\n");
}

fn integer(output: &mut Vec<u8>, flag: u8, value: i64) {
    output.push(flag);
    if value < 0 {
        output.push(b'-');
    }
    number(output, value.unsigned_abs());
}

fn c_text(value: &[u8]) -> &[u8] {
    value.split(|&byte| byte == 0).next().unwrap_or_default()
}

impl Reply<'_> {
    /// Append the PHP reply's bytes. Present empty status/error stays empty;
    /// bulk strings are binary, while text and string map keys stop at NUL.
    pub fn append_to(self, output: &mut Vec<u8>) {
        match self {
            Self::Error(value) | Self::Status(value) => {
                let error = matches!(self, Self::Error(_));
                output.push(if error { b'-' } else { b'+' });
                output.extend_from_slice(c_text(value.unwrap_or(if error {
                    b"ERR"
                } else {
                    b"OK"
                })));
            }
            Self::Nil => output.extend_from_slice(b"$-1"),
            Self::Integer(value) => integer(output, b':', value),
            Self::Bulk(value) | Self::StringKey(value) => {
                // A string map key's declared size includes bytes after its first NUL.
                header(output, b'$', value.len() as u64);
                output.extend_from_slice(if matches!(self, Self::StringKey(_)) {
                    c_text(value)
                } else {
                    value
                });
            }
            Self::Set(count) | Self::Map(count) => {
                // Native printf receives an unsigned32 count as signed32, after
                // the map count multiplication wraps.
                let count = count.wrapping_mul(if matches!(self, Self::Map(_)) { 2 } else { 1 });
                integer(output, b'*', i64::from(count.cast_signed()));
            }
            Self::IntegerKey(value) => {
                let mut digits = [0; 20];
                return Reply::Bulk(crate::redis::decimal(value, &mut digits)).append_to(output);
            }
        }
        output.extend_from_slice(b"\r\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
