//! The address part of a hooked stream socket's name (`tcp://host:port`,
//! `udp://[v6]:port`), as `parse_ip_address_ex` splits it. Native code copies the
//! host and formats the error text.

/// Why a name has no address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressError {
    /// A bracketed IPv6 address without `]:`.
    Ipv6,
    /// No `:` before the last byte.
    Address,
}

/// The host and port of `name`.
///
/// # Errors
/// The [`AddressError`] when it has none.
pub fn parse(name: &[u8]) -> Result<(&[u8], i32), AddressError> {
    if let [b'[', inner @ .., _] = name {
        // `]` is searched in name[1..len - 1], so the byte after it exists.
        let close = inner
            .iter()
            .position(|&b| b == b']')
            .ok_or(AddressError::Ipv6)?;
        // `close` indexes `inner`, which starts one byte into `name`.
        let Some([b':', port @ ..]) = name.get(close + 2..) else {
            return Err(AddressError::Ipv6);
        };
        let host = inner.get(..close).unwrap_or_default();
        return Ok((host, swoole_rs_corebase::atoi(port)));
    }
    let colon = name
        .get(..name.len().saturating_sub(1))
        .and_then(|head| head.iter().position(|&b| b == b':'))
        .ok_or(AddressError::Address)?;
    let port = swoole_rs_corebase::atoi(name.get(colon + 1..).unwrap_or_default());
    Ok((name.get(..colon).unwrap_or_default(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(name: &[u8]) -> Option<(&[u8], i32)> {
        parse(name).ok()
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
        assert_eq!(parse(b"[fe80::1]"), Err(AddressError::Ipv6));
        assert_eq!(parse(b"[fe80::1]x80"), Err(AddressError::Ipv6));
        assert_eq!(parse(b"[:"), Err(AddressError::Ipv6));
        assert_eq!(parse(b"localhost"), Err(AddressError::Address));
        assert_eq!(parse(b"host:"), Err(AddressError::Address));
        assert_eq!(parse(b"["), Err(AddressError::Address));
        assert_eq!(parse(b""), Err(AddressError::Address));
    }
}
