//! SOCKS5's C ABI over crates/protocols' codec. Besides the module's native gate,
//! the oracle's candidate socks5.cc (the protocols overlay patch) calls each one.
use swoole_rs_protocols::{
    SocksTransition, socks_auth, socks_config, socks_connect, socks_negotiate, socks_strerror,
    socks_transition,
};

/// [`socks_transition`].
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

/// [`socks_auth`].
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
    // SAFETY: `output` lends `capacity` exclusive writable bytes, disjoint from the inputs.
    socks_auth(user, password, unsafe { crate::output(output, capacity) })
}

/// [`socks_connect`].
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
    // SAFETY: `output` lends `capacity` exclusive writable bytes, disjoint from the inputs.
    socks_connect(address_type, address, port, unsafe {
        crate::output(output, capacity)
    })
}

/// [`socks_negotiate`]: 3, with the method written, or -1.
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
    // SAFETY: `output` lends `capacity` exclusive writable bytes, disjoint from `method`.
    let Some(selected) = socks_negotiate(has_user != 0, unsafe { crate::output(output, capacity) })
    else {
        return -1;
    };
    // SAFETY: `method` is non-null (checked above), aligned and writable per the export's contract.
    unsafe { method.write(selected) };
    3
}

/// [`socks_config`].
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_socks_config(user_length: usize, password_length: usize) -> i32 {
    socks_config(user_length, password_length)
}

/// [`socks_strerror`]: a static string.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_socks_strerror(code: i32) -> *const std::ffi::c_char {
    socks_strerror(code).as_ptr()
}
