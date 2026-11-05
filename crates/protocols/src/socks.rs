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

/// # Safety
/// Response is borrowed/readable; result is aligned/exclusive/disjoint writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socks_transition(
    state: u8,
    method: u8,
    data: *const u8,
    length: usize,
    result: *mut SocksTransition,
) -> i32 {
    if result.is_null() {
        return -1;
    }
    // SAFETY: `data` lends `length` initialized bytes (null only when empty) for this call.
    let Some(response) = (unsafe { crate::input(data, length) }) else {
        return -1;
    };
    // SAFETY: `result` is non-null (checked above), aligned and writable per the export's contract.
    unsafe { result.write(socks_transition(state, method, response)) };
    0
}

/// # Safety
/// Borrowed username/password are disjoint from exclusive writable output.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socks_auth(
    user: *const u8,
    user_length: usize,
    password: *const u8,
    password_length: usize,
    output: *mut u8,
    capacity: usize,
) -> i64 {
    // SAFETY: `user` lends `user_length` initialized bytes (null only when empty) for this call.
    let user = unsafe { crate::input(user, user_length) };
    // SAFETY: `password` lends `password_length` initialized bytes (null only when empty).
    let password = unsafe { crate::input(password, password_length) };
    let (Some(user), Some(password)) = (user, password) else {
        return -1;
    };
    let Some(size) = user_length
        .checked_add(password_length)
        .and_then(|n| n.checked_add(3))
    else {
        return -1;
    };
    // SAFETY: `output` lends `capacity` exclusive writable bytes, disjoint from the inputs.
    let Some(output) = (unsafe { crate::output(output, capacity, size) }) else {
        return -1;
    };
    // Upstream's Socks5Proxy::create refuses credentials over 250 bytes; a longer one
    // would wrap its one-byte length, so refuse it here too.
    let (Ok(user_byte), Ok(password_byte)) =
        (u8::try_from(user_length), u8::try_from(password_length))
    else {
        return -1;
    };
    crate::concat(output, &[&[1, user_byte], user, &[password_byte], password]);
    i64::try_from(size).unwrap_or(-1)
}

/// Encode CONNECT with a borrowed domain (type 3), or already validated native
/// IPv4/IPv6 bytes (types 1/4). Native address validation preserves errno/errors.
/// Returns -2 for a domain over 255 bytes (upstream b58d6342e), before any write.
///
/// # Safety
/// Address is readable; output is exclusive writable and disjoint from input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socks_connect(
    address_type: u8,
    address: *const u8,
    length: usize,
    port: u16,
    output: *mut u8,
    capacity: usize,
) -> i64 {
    // SAFETY: `address` lends `length` initialized bytes (null only when empty) for this call.
    let Some(address) = (unsafe { crate::input(address, length) }) else {
        return -1;
    };
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
    // SAFETY: `output` lends `capacity` exclusive writable bytes, disjoint from the inputs.
    let Some(output) = (unsafe { crate::output(output, capacity, size) }) else {
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

/// # Safety
/// Output covers three writable bytes. Method output is aligned/exclusive and
/// disjoint; no other aliases are used while writing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socks_negotiate(
    has_user: u8,
    method: *mut u8,
    output: *mut u8,
    capacity: usize,
) -> i64 {
    if method.is_null() {
        return -1;
    }
    // SAFETY: `output` lends `capacity` exclusive writable bytes, disjoint from the inputs.
    let Some(output) = (unsafe { crate::output(output, capacity, 3) }) else {
        return -1;
    };
    let selected = if has_user == 0 { 0 } else { 2 };
    output.copy_from_slice(&[5, 1, selected]);
    // SAFETY: `method` is non-null (checked above), aligned and writable per the export's contract.
    unsafe { method.write(selected) };
    3
}

/// Factory credentials differ from direct auth packing: either missing field
/// disables both credentials; either field over250 bytes rejects the factory.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_socks_config(user_length: usize, password_length: usize) -> i32 {
    if user_length > 250 || password_length > 250 {
        0
    } else if user_length != 0 && password_length != 0 {
        2
    } else {
        1
    }
}

/// Stable borrowed C strings; native logging executes after this call returns.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_socks_strerror(code: i32) -> *const std::ffi::c_char {
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
    .as_ptr()
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
                assert_eq!(swr_v1_socks_config(user, password), expected);
            }
        }
        assert_eq!(swr_v1_socks_config(usize::MAX, 0), 0);
        assert_eq!(swr_v1_socks_config(0, usize::MAX), 0);
        unsafe {
            assert_eq!(
                std::ffi::CStr::from_ptr(swr_v1_socks_strerror(5)).to_bytes(),
                b"Connection refused by destination host"
            );
            assert_eq!(
                std::ffi::CStr::from_ptr(swr_v1_socks_strerror(i32::MIN)).to_bytes(),
                b"Unknown error"
            );
            let mut output = [0xa5; 512];
            let destination = [b'x'; 256];
            assert_eq!(
                swr_v1_socks_connect(
                    3,
                    destination.as_ptr(),
                    256,
                    0,
                    output.as_mut_ptr(),
                    output.len()
                ),
                -2
            );
            assert_eq!(output, [0xa5; 512]);
            assert_eq!(
                swr_v1_socks_connect(
                    3,
                    destination.as_ptr(),
                    255,
                    u16::MAX,
                    output.as_mut_ptr(),
                    output.len()
                ),
                262
            );
            assert_eq!(&output[..5], &[5, 1, 0, 3, 255]);
            assert_eq!(&output[260..262], &[255, 255]);
        }
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
