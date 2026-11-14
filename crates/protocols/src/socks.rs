/// The request a handshake step sends next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocksSend {
    /// The username/password request.
    Auth,
    /// The CONNECT request.
    Connect,
}

/// Why a handshake step failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocksError {
    /// The reply is shorter than two bytes.
    Short,
    /// The reply's version is not the one this state expects.
    Version,
    /// The server picked another method than the one offered.
    Method,
    /// The username/password was refused.
    Auth,
    /// The server refused the CONNECT with this reply code.
    Server(u8),
}

/// A handshake step: the next state and what to send, or why it failed. Native
/// callback execution happens after this decision. State advances before
/// send/packing, including a failed send, as it does in the pinned implementation.
/// An unknown state stays where it is and sends nothing.
///
/// # Errors
/// The [`SocksError`] that ends the handshake.
pub fn socks_transition(
    state: u8,
    method: u8,
    response: &[u8],
) -> Result<(u8, Option<SocksSend>), SocksError> {
    let [version, reply, ..] = *response else {
        return Err(SocksError::Short);
    };
    match state {
        1 if version != 5 => Err(SocksError::Version),
        1 if method != reply => Err(SocksError::Method),
        1 if method == 2 => Ok((2, Some(SocksSend::Auth))),
        1 => Ok((3, Some(SocksSend::Connect))),
        2 if version != 1 => Err(SocksError::Version),
        2 if reply != 0 => Err(SocksError::Auth),
        2 => Ok((3, Some(SocksSend::Connect))),
        3 if version != 5 => Err(SocksError::Version),
        3 if reply != 0 => Err(SocksError::Server(reply)),
        3 => Ok((4, None)),
        _ => Ok((state, None)),
    }
}

/// The username/password request in `output`: its size, or `None` when `output` is
/// too short or either credential is over 255 bytes.
#[must_use]
pub fn socks_auth(user: &[u8], password: &[u8], output: &mut [u8]) -> Option<usize> {
    let size = user.len().checked_add(password.len())?.checked_add(3)?;
    let output = output.get_mut(..size)?;
    // Upstream's Socks5Proxy::create refuses credentials over 250 bytes; a longer one
    // would wrap its one-byte length, so refuse it here too.
    let user_byte = u8::try_from(user.len()).ok()?;
    let password_byte = u8::try_from(password.len()).ok()?;
    crate::concat(output, &[&[1, user_byte], user, &[password_byte], password]);
    Some(size)
}

/// Why [`socks_connect`] refused to encode a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectRefusal {
    /// A domain over 255 bytes (upstream b58d6342e).
    DomainTooLong,
    /// Another address type or length, or an output too short.
    Invalid,
}

/// Encode CONNECT with a borrowed domain (type 3), or already validated native
/// IPv4/IPv6 bytes (types 1/4). Native address validation preserves errno/errors.
/// Returns the request's size.
/// # Errors
/// The [`ConnectRefusal`], before any write.
pub fn socks_connect(
    address_type: u8,
    address: &[u8],
    port: u16,
    output: &mut [u8],
) -> Result<usize, ConnectRefusal> {
    let length = address.len();
    if !matches!((address_type, length), (1, 4) | (4, 16) | (3, _)) {
        return Err(ConnectRefusal::Invalid);
    }
    let domain_length = u8::try_from(length);
    if address_type == 3 && domain_length.is_err() {
        return Err(ConnectRefusal::DomainTooLong);
    }
    let offset = if address_type == 3 { 5usize } else { 4usize };
    let size = length
        .checked_add(offset + 2)
        .ok_or(ConnectRefusal::Invalid)?;
    let output = output.get_mut(..size).ok_or(ConnectRefusal::Invalid)?;
    let domain_length = [domain_length.unwrap_or(0)];
    let domain_length: &[u8] = if address_type == 3 {
        &domain_length
    } else {
        &[]
    };
    crate::concat(
        output,
        &[
            &[5, 1, 0, address_type],
            domain_length,
            address,
            &port.to_be_bytes(),
        ],
    );
    Ok(size)
}

/// The three-byte method negotiation in `output`: the method it offers, or `None`
/// when `output` is shorter than three bytes.
#[must_use]
pub fn socks_negotiate(has_user: bool, output: &mut [u8]) -> Option<u8> {
    let selected = if has_user { 2 } else { 0 };
    output.get_mut(..3)?.copy_from_slice(&[5, 1, selected]);
    Some(selected)
}

/// Which credentials a SOCKS5 proxy keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocksConfig {
    /// A credential is over 250 bytes: the proxy is refused.
    Refused,
    /// One is missing, so neither is kept.
    Anonymous,
    /// Both are kept.
    Credentials,
}

/// Factory credentials differ from direct auth packing: either missing field
/// disables both credentials; either field over 250 bytes rejects the factory.
#[must_use]
pub fn socks_config(user_length: usize, password_length: usize) -> SocksConfig {
    if user_length > 250 || password_length > 250 {
        SocksConfig::Refused
    } else if user_length != 0 && password_length != 0 {
        SocksConfig::Credentials
    } else {
        SocksConfig::Anonymous
    }
}

/// `Socks5Proxy::strerror`: the text of a server reply code.
#[must_use]
pub fn socks_strerror(code: i32) -> &'static std::ffi::CStr {
    match code {
        1 => c"General failure",
        2 => c"Connection not allowed by ruleset",
        3 => c"Network unreachable",
        4 => c"Host unreachable",
        5 => c"Connection refused by destination host",
        6 => c"TTL expired",
        7 => c"command not supported / protocol error",
        8 => c"address type not supported",
        _ => c"Unknown error",
    }
}

/// Preserve the deferred check of proxy-host length, not destination length.
/// -2 reports only that bound; the frozen CONNECT encoder keeps its old contract.
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn factory_limits_deferred_proxy_bound_and_stable_error_text() {
        for user in 0..=251 {
            for password in 0..=251 {
                let expected = if user > 250 || password > 250 {
                    SocksConfig::Refused
                } else if user != 0 && password != 0 {
                    SocksConfig::Credentials
                } else {
                    SocksConfig::Anonymous
                };
                assert_eq!(socks_config(user, password), expected);
            }
        }
        assert_eq!(socks_config(usize::MAX, 0), SocksConfig::Refused);
        assert_eq!(socks_config(0, usize::MAX), SocksConfig::Refused);
        assert_eq!(
            socks_strerror(5).to_bytes(),
            b"Connection refused by destination host"
        );
        assert_eq!(socks_strerror(i32::MIN).to_bytes(), b"Unknown error");
        let mut output = [0xa5; 512];
        let destination = [b'x'; 256];
        assert_eq!(
            socks_connect(3, &destination, 0, &mut output),
            Err(ConnectRefusal::DomainTooLong)
        );
        assert_eq!(output, [0xa5; 512]);
        assert_eq!(
            socks_connect(3, &destination[..255], u16::MAX, &mut output),
            Ok(262)
        );
        assert_eq!(&output[..5], &[5, 1, 0, 3, 255]);
        assert_eq!(&output[260..262], &[255, 255]);
    }
    #[test]
    fn state_changes_precede_native_send_and_preserve_unknown_states() {
        assert_eq!(
            socks_transition(1, 2, &[5, 2]),
            Ok((2, Some(SocksSend::Auth)))
        );
        assert_eq!(
            socks_transition(2, 2, &[1, 0]),
            Ok((3, Some(SocksSend::Connect)))
        );
        assert_eq!(socks_transition(3, 2, &[5, 0]), Ok((4, None)));
        for state in [0, 4, 255] {
            assert_eq!(socks_transition(state, 2, &[0, 255]), Ok((state, None)));
        }
        assert_eq!(socks_transition(4, 0, &[]), Err(SocksError::Short));
        assert_eq!(socks_transition(1, 0, &[4, 0]), Err(SocksError::Version));
        assert_eq!(socks_transition(1, 0, &[5, 2]), Err(SocksError::Method));
        assert_eq!(socks_transition(2, 2, &[1, 1]), Err(SocksError::Auth));
        assert_eq!(socks_transition(3, 2, &[5, 8]), Err(SocksError::Server(8)));
    }
}
