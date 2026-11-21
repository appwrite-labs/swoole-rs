#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SocksTransition {
    pub state: u8,
    pub action: u8,
    pub error: u8,
    pub reply: u8,
}

/// Native callback execution happens after this decision. State advances before
/// send/packing, including a failed send, as it does in the pinned implementation.
#[must_use]
pub fn socks_transition(state: u8, method: u8, response: &[u8]) -> SocksTransition {
    let mut result = SocksTransition {
        state,
        ..SocksTransition::default()
    };
    let [version, reply, ..] = *response else {
        result.error = 1;
        return result;
    };
    result.reply = reply;
    match state {
        1 if version != 5 => result.error = 2,
        1 if method != reply => result.error = 3,
        1 if method == 2 => {
            result.state = 2;
            result.action = 1;
        }
        1 => {
            result.state = 3;
            result.action = 2;
        }
        2 if version != 1 => result.error = 2,
        2 if reply != 0 => result.error = 4,
        2 => {
            result.state = 3;
            result.action = 2;
        }
        3 if version != 5 => result.error = 2,
        3 if reply != 0 => result.error = 5,
        3 => {
            result.state = 4;
            result.action = 3;
        }
        _ => {}
    }
    result
}

/// The username/password request in `output`: its size, or -1 when `output` is
/// too short or either credential is over 255 bytes.
#[must_use]
pub fn socks_auth(user: &[u8], password: &[u8], output: &mut [u8]) -> i64 {
    let Some(size) = user
        .len()
        .checked_add(password.len())
        .and_then(|n| n.checked_add(3))
    else {
        return -1;
    };
    let Some(output) = output.get_mut(..size) else {
        return -1;
    };
    // Upstream's Socks5Proxy::create refuses credentials over 250 bytes; a longer one
    // would wrap its one-byte length, so refuse it here too.
    let (Ok(user_byte), Ok(password_byte)) =
        (u8::try_from(user.len()), u8::try_from(password.len()))
    else {
        return -1;
    };
    crate::concat(output, &[&[1, user_byte], user, &[password_byte], password]);
    i64::try_from(size).unwrap_or(-1)
}

/// Encode CONNECT with a borrowed domain (type 3), or already validated native
/// IPv4/IPv6 bytes (types 1/4). Native address validation preserves errno/errors.
/// Returns -2 for a domain over 255 bytes (upstream b58d6342e), before any write;
/// -1 for another type or length, or an `output` too short.
#[must_use]
pub fn socks_connect(address_type: u8, address: &[u8], port: u16, output: &mut [u8]) -> i64 {
    let length = address.len();
    if !matches!((address_type, length), (1, 4) | (4, 16) | (3, _)) {
        return -1;
    }
    let domain_length = u8::try_from(length);
    if address_type == 3 && domain_length.is_err() {
        return -2;
    }
    let offset = if address_type == 3 { 5usize } else { 4usize };
    let Some(size) = length.checked_add(offset + 2) else {
        return -1;
    };
    let Some(output) = output.get_mut(..size) else {
        return -1;
    };
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
    i64::try_from(size).unwrap_or(-1)
}

/// The three-byte method negotiation in `output`: the method it offers, or `None`
/// when `output` is shorter than three bytes.
#[must_use]
pub fn socks_negotiate(has_user: bool, output: &mut [u8]) -> Option<u8> {
    let selected = if has_user { 2 } else { 0 };
    output.get_mut(..3)?.copy_from_slice(&[5, 1, selected]);
    Some(selected)
}

/// Factory credentials differ from direct auth packing: either missing field
/// disables both credentials; either field over250 bytes rejects the factory.
#[must_use]
pub fn socks_config(user_length: usize, password_length: usize) -> i32 {
    if user_length > 250 || password_length > 250 {
        0
    } else if user_length != 0 && password_length != 0 {
        2
    } else {
        1
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
                    0
                } else if user != 0 && password != 0 {
                    2
                } else {
                    1
                };
                assert_eq!(socks_config(user, password), expected);
            }
        }
        assert_eq!(socks_config(usize::MAX, 0), 0);
        assert_eq!(socks_config(0, usize::MAX), 0);
        assert_eq!(
            socks_strerror(5).to_bytes(),
            b"Connection refused by destination host"
        );
        assert_eq!(socks_strerror(i32::MIN).to_bytes(), b"Unknown error");
        let mut output = [0xa5; 512];
        let destination = [b'x'; 256];
        assert_eq!(socks_connect(3, &destination, 0, &mut output), -2);
        assert_eq!(output, [0xa5; 512]);
        assert_eq!(
            socks_connect(3, &destination[..255], u16::MAX, &mut output),
            262
        );
        assert_eq!(&output[..5], &[5, 1, 0, 3, 255]);
        assert_eq!(&output[260..262], &[255, 255]);
    }
    #[test]
    fn state_changes_precede_native_send_and_preserve_unknown_states() {
        assert_eq!(
            socks_transition(1, 2, &[5, 2]),
            SocksTransition {
                state: 2,
                action: 1,
                error: 0,
                reply: 2
            }
        );
        assert_eq!(socks_transition(2, 2, &[1, 0]).state, 3);
        assert_eq!(socks_transition(3, 2, &[5, 0]).state, 4);
        for state in [0, 4, 255] {
            assert_eq!(socks_transition(state, 2, &[0, 255]).error, 0);
        }
        assert_eq!(socks_transition(4, 0, &[]).error, 1);
        assert_eq!(socks_transition(1, 0, &[4, 0]).error, 2);
        assert_eq!(socks_transition(1, 0, &[5, 2]).error, 3);
        assert_eq!(socks_transition(2, 2, &[1, 1]).error, 4);
        assert_eq!(socks_transition(3, 2, &[5, 8]).error, 5);
    }
}
