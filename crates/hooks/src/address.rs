//! The address part of a hooked stream socket's name (`tcp://host:port`,
//! `udp://[v6]:port`), as `parse_ip_address_ex` splits it. Native code copies the
//! host and formats the error text.

/// Where the host is and the port, or why there is none.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AddressPlan {
    /// 0 parsed; 1 a bracketed IPv6 address without `]:`; 2 no `:` before the last byte.
    pub status: i32,
    pub host_offset: usize,
    pub host_length: usize,
    pub port: i32,
}

/// `strtol(text, nullptr, 10)` cast to `int`, as `sw_atoi` reads the port: leading
/// whitespace, an optional sign, then digits, saturating at the `long` range.
fn atoi(text: &[u8]) -> i32 {
    let mut rest = text
        .iter()
        .skip_while(|byte| matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
        .peekable();
    let negative = rest.next_if(|&&byte| byte == b'-').is_some();
    if !negative {
        rest.next_if(|&&byte| byte == b'+');
    }
    let mut value: i64 = 0;
    for &byte in rest.take_while(|byte| byte.is_ascii_digit()) {
        let digit = i64::from(byte - b'0');
        value = if negative {
            value.saturating_mul(10).saturating_sub(digit)
        } else {
            value.saturating_mul(10).saturating_add(digit)
        };
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's static_cast<int>(long)"
    )]
    let port = value as i32;
    port
}

/// The plan of `name`.
#[must_use]
pub fn parse(name: &[u8]) -> AddressPlan {
    let failed = |status| AddressPlan {
        status,
        ..AddressPlan::default()
    };
    if let [b'[', inner @ .., _] = name {
        // `]` is searched in name[1..len - 1], so the byte after it exists.
        let Some(close) = inner.iter().position(|&b| b == b']') else {
            return failed(1);
        };
        // `close` indexes `inner`, which starts one byte into `name`.
        let Some([b':', port @ ..]) = name.get(close + 2..) else {
            return failed(1);
        };
        return AddressPlan {
            status: 0,
            host_offset: 1,
            host_length: close,
            port: atoi(port),
        };
    }
    match name
        .get(..name.len().saturating_sub(1))
        .and_then(|head| head.iter().position(|&b| b == b':'))
    {
        Some(colon) => AddressPlan {
            status: 0,
            host_offset: 0,
            host_length: colon,
            port: atoi(name.get(colon + 1..).unwrap_or_default()),
        },
        None => failed(2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(name: &[u8]) -> Option<(&[u8], i32)> {
        let plan = parse(name);
        (plan.status == 0).then(|| (&name[plan.host_offset..][..plan.host_length], plan.port))
    }

    #[test]
    fn hosts_and_ports() {
        assert_eq!(parsed(b"127.0.0.1:80"), Some((&b"127.0.0.1"[..], 80)));
        assert_eq!(parsed(b"[fe80::1]:443"), Some((&b"fe80::1"[..], 443)));
        assert_eq!(parsed(b"[::1]:"), Some((&b"::1"[..], 0)));
        assert_eq!(parsed(b"host: -12x"), Some((&b"host"[..], -12)));
        assert_eq!(parsed(b"h:99999999999"), Some((&b"h"[..], 1_215_752_191)));
        assert_eq!(parsed(b"h:a:1"), Some((&b"h"[..], 0)));
    }

    #[test]
    fn failures_match_upstream() {
        assert_eq!(parse(b"[fe80::1]").status, 1);
        assert_eq!(parse(b"[fe80::1]x80").status, 1);
        assert_eq!(parse(b"[:").status, 1);
        assert_eq!(parse(b"localhost").status, 2);
        assert_eq!(parse(b"host:").status, 2);
        assert_eq!(parse(b"[").status, 2);
        assert_eq!(parse(b"").status, 2);
    }
}
