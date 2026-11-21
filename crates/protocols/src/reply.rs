//! PHP RESP wire fragments. Zend conversion/recursion and native buffer growth
//! stay outside Rust; borrowed payload bytes are appended only after return.

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ReplyParts {
    pub prefix: [u8; 32],
    pub prefix_length: u32,
    pub body_length: usize,
    pub suffix_length: u32,
}
impl ReplyParts {
    fn bytes(&mut self, bytes: &[u8]) {
        let start = self.prefix_length as usize;
        // Every reply prefix fits the fixed buffer: a flag, 20 digits and CRLF at most.
        if let Some(slot) = self.prefix.get_mut(start..start + bytes.len())
            && let Ok(length) = u32::try_from(bytes.len())
        {
            slot.copy_from_slice(bytes);
            self.prefix_length += length;
        }
    }
    fn number(&mut self, value: u64) {
        let mut buffer = [0; 20];
        self.bytes(crate::redis::decimal(value, &mut buffer));
    }
    fn header(&mut self, flag: u8, value: u64) {
        self.bytes(&[flag]);
        self.number(value);
        self.bytes(b"\r\n");
    }
}

/// Kinds match PHP reply constants: error0/nil1/status2/int3/bulk4/set5/map6.
/// Additive kind8 encodes a string map key;9 an unsigned integer map key.
/// `number` carries the signed integer's bits, array count, or unsigned key.
/// Unlike the native primitive formatter, present empty status/error stays empty
/// and empty bulk still emits `$0\r\n\r\n`. Map keys preserve printf NUL truncation.
#[must_use]
pub fn reply_parts(kind: u8, present: bool, number: u64, value: &[u8]) -> Option<ReplyParts> {
    let mut result = ReplyParts::default();
    match kind {
        0 | 2 => {
            result.bytes(if kind == 0 { b"-" } else { b"+" });
            if present {
                result.body_length = value
                    .iter()
                    .position(|&byte| byte == 0)
                    .unwrap_or(value.len());
            } else {
                result.bytes(if kind == 0 { b"ERR" } else { b"OK" });
            }
            result.suffix_length = 2;
        }
        3 => {
            let integer = number.cast_signed();
            result.bytes(if integer < 0 { b":-" } else { b":" });
            result.number(integer.unsigned_abs());
            result.bytes(b"\r\n");
        }
        4 | 8 => {
            result.header(b'$', value.len() as u64);
            result.body_length = if kind == 8 {
                value
                    .iter()
                    .position(|&byte| byte == 0)
                    .unwrap_or(value.len())
            } else {
                value.len()
            };
            result.suffix_length = 2;
        }
        5 | 6 => {
            // Native printf receives an unsigned32 count as signed32, including
            // map count multiplication wrapping before the signed conversion.
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_possible_wrap,
                reason = "native printf of an unsigned32 count as signed32, described above"
            )]
            let count = (number as u32).wrapping_mul(if kind == 6 { 2 } else { 1 }) as i32;
            result.bytes(if count < 0 { b"*-" } else { b"*" });
            result.number(u64::from(count.unsigned_abs()));
            result.bytes(b"\r\n");
        }
        1 => result.bytes(b"$-1\r\n"),
        9 => {
            let mut digits = [0; 20];
            let key = crate::redis::decimal(number, &mut digits);
            result.header(b'$', key.len() as u64);
            result.bytes(key);
            result.bytes(b"\r\n");
        }
        _ => return None,
    }
    Some(result)
}

#[cfg(test)]
#[expect(
    clippy::cast_sign_loss,
    reason = "tests drive the native integer interfaces with literal and loop values"
)]
mod tests {
    use super::*;
    fn encode(kind: u8, present: bool, number: u64, data: &[u8]) -> Vec<u8> {
        let parts = reply_parts(kind, present, number, data).unwrap();
        let mut bytes = parts.prefix[..parts.prefix_length as usize].to_vec();
        bytes.extend_from_slice(&data[..parts.body_length]);
        bytes.extend_from_slice(&b"\r\n"[..parts.suffix_length as usize]);
        bytes
    }
    #[test]
    fn php_defaults_empty_binary_and_map_key_policy() {
        assert_eq!(encode(0, false, 0, b""), b"-ERR\r\n");
        assert_eq!(encode(2, false, 0, b""), b"+OK\r\n");
        assert_eq!(encode(0, true, 0, b""), b"-\r\n");
        assert_eq!(encode(2, true, 0, b"a\0b"), b"+a\r\n");
        assert_eq!(encode(4, true, 0, b""), b"$0\r\n\r\n");
        assert_eq!(encode(4, true, 0, b"a\0b"), b"$3\r\na\0b\r\n");
        assert_eq!(encode(8, true, 0, b"k\0tail"), b"$6\r\nk\r\n");
        assert_eq!(encode(1, false, 0, b"ignored"), b"$-1\r\n");
    }
    #[test]
    fn signed_unsigned_and_native_count_wrapping() {
        for value in [i64::MIN, -1, 0, 1, i64::MAX] {
            assert_eq!(
                encode(3, true, value as u64, b""),
                format!(":{value}\r\n").as_bytes()
            );
        }
        assert_eq!(
            encode(9, true, u64::MAX, b""),
            b"$20\r\n18446744073709551615\r\n"
        );
        assert_eq!(encode(5, true, u64::from(u32::MAX), b""), b"*-1\r\n");
        assert_eq!(encode(6, true, 1 << 31, b""), b"*0\r\n");
        assert_eq!(encode(6, true, (1 << 30) + 1, b""), b"*-2147483646\r\n");
    }
}
