//! `HttpProxy`'s create, pack and handshake over corebase's proxy owner, which the
//! clients and hooks call, over crates/http's CONNECT formatting.
use swoole_rs_corebase::{
    CoreStringView, ProxyState, log, set_errno, swoole_version, swr_v1_core_proxy_configure,
};
use swoole_rs_http::{HttpProxyRequest, proxy_authorization, proxy_handshake, proxy_pack};

use std::ffi::c_char;

const HANDSHAKE_FAILED: i32 = 7103;

/// The bytes of the owner's string slot `index`.
///
/// # Safety
/// No one changes the slot while the bytes are borrowed.
unsafe fn slot(owner: &ProxyState, index: usize) -> &[u8] {
    // SAFETY: the slot is stable, as the caller guarantees.
    let view = unsafe { owner.string(index) };
    // SAFETY: a slot's view names `length` initialized bytes.
    unsafe { crate::bytes(view.data.cast(), view.length) }.unwrap_or_default()
}

/// `HttpProxy::get_auth_str()`'s Base64 credentials into 512 bytes at `output`,
/// NUL-terminated: 1 when they were cut to fit (errno EOVERFLOW), else 0.
/// # Safety
/// owner is live/stable; output lends 512 bytes and written is writable/disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_proxy_owned_authorization(
    owner: *const ProxyState,
    output: *mut u8,
    written: *mut usize,
) -> i32 {
    if output.is_null() || written.is_null() {
        return crate::INVALID;
    }
    // SAFETY: caller lends the live owner's unchanged string slots.
    let owner = unsafe { &*owner };
    // SAFETY: the username is stable through the bounded codec.
    let user = unsafe { slot(owner, 1) };
    // SAFETY: the password is stable through the bounded codec.
    let password = unsafe { slot(owner, 2) };
    let mut encoded = [0u8; 512];
    let (status, length) = proxy_authorization(user, password, &mut encoded);
    // SAFETY: `length + 1 <= 512` bytes into the caller's 512, disjoint from `encoded`.
    unsafe { std::ptr::copy_nonoverlapping(encoded.as_ptr(), output, length + 1) };
    // SAFETY: `written` is non-null (checked above) and writable.
    unsafe { written.write(length) };
    status
}

/// `HttpProxy::create`'s assignments on a fresh owner: the host and port, then the
/// credentials when both are set. Only an allocation can fail, and that aborts as
/// Rust's allocation failures do.
/// # Safety
/// owner is a live owner nothing else uses; the views lend their bytes for the call.
#[expect(
    clippy::too_many_arguments,
    reason = "HttpProxy::create's owner, port and three strings as pointer and length"
)]
pub unsafe fn swr_v1_http_proxy_configure(
    owner: *mut ProxyState,
    host: *const c_char,
    host_length: usize,
    port: i32,
    user: *const c_char,
    user_length: usize,
    password: *const c_char,
    password_length: usize,
) {
    let authenticated = user_length != 0 && password_length != 0;
    let view = |data, length| CoreStringView { data, length };
    let texts = [
        view(host, host_length),
        view(user, user_length),
        view(password, password_length),
    ];
    // SAFETY: the caller's live owner and three borrowed views.
    let result =
        unsafe { swr_v1_core_proxy_configure(owner, texts.as_ptr(), port, authenticated, -1) };
    if result.status != 0 {
        std::process::abort();
    }
}

/// `HttpProxy::get_auth_str()`: the Base64 credentials, empty on failure, errno
/// EOVERFLOW when they were cut to fit.
fn authorization(owner: &ProxyState) -> Vec<u8> {
    let (mut output, mut written) = ([0u8; 512], 0);
    // SAFETY: the live owner, 512 writable bytes and the length slot.
    let status = unsafe {
        swr_v1_http_proxy_owned_authorization(owner, output.as_mut_ptr(), &raw mut written)
    };
    if status < 0 {
        return Vec::new();
    }
    if status == 1 {
        set_errno(libc::EOVERFLOW);
    }
    let encoded = output.get(..written).unwrap_or_default();
    encoded
        .split(|&b| b == 0)
        .next()
        .unwrap_or_default()
        .to_vec()
}

/// `HttpProxy::pack`: the CONNECT request for `host` into `output[..capacity]`, with
/// the authorization when the proxy has a password. Returns the bytes written (the
/// whole length for a zero capacity).
/// # Safety
/// owner is live; host lends its bytes; output is `capacity` writable bytes disjoint from both.
pub unsafe fn swr_v1_http_proxy_connect(
    owner: *const ProxyState,
    host: *const u8,
    host_length: usize,
    output: *mut u8,
    capacity: usize,
) -> usize {
    // SAFETY: the caller's live owner.
    let state = unsafe { &*owner };
    // SAFETY: the live owner's password slot.
    let authenticated = !unsafe { slot(state, 2) }.is_empty();
    let auth = if authenticated {
        authorization(state)
    } else {
        Vec::new()
    };
    // SAFETY: the pinned version, a static string.
    let version = unsafe { std::ffi::CStr::from_ptr(swoole_version()) }.to_bytes();
    // SAFETY: leaf scalar snapshot before formatting.
    let fields = unsafe { *state.fields.get() };
    // SAFETY: the caller lends `host_length` bytes at `host`.
    let Ok(host) = (unsafe { crate::bytes(host, host_length) }) else {
        return 0;
    };
    let request = HttpProxyRequest {
        // SAFETY: the target bytes remain stable throughout formatting.
        target: unsafe { slot(state, 3) },
        host,
        version,
        authorization: &auth,
        port: fields.target_port,
        authenticated,
    };
    let Ok(text) = proxy_pack(&request) else {
        return 0;
    };
    if capacity == 0 {
        return text.len();
    }
    if output.is_null() || capacity > i32::MAX as usize {
        return 0;
    }
    let length = text.len().min(capacity - 1);
    // SAFETY: `length < capacity` writable bytes, disjoint from `text`.
    unsafe { std::ptr::copy_nonoverlapping(text.as_ptr(), output, length) };
    // SAFETY: `length < capacity`, inside the writable output.
    unsafe { output.wrapping_add(length).write(0) };
    length
}

/// `HttpProxy::handshake`: whether `data[..length]` accepts the CONNECT, setting the
/// last error when the proxy refused it.
/// # Safety
/// data lends `length` bytes (null only when empty).
#[must_use]
pub unsafe fn swr_v1_http_proxy_accept(data: *const u8, length: usize) -> bool {
    // SAFETY: the caller's bytes.
    let Ok(input) = (unsafe { crate::bytes(data, length) }) else {
        return false;
    };
    let status = proxy_handshake(input);
    if status == -2 {
        log::set_last_error(HANDSHAKE_FAILED);
    }
    status == 1
}

/// The cases of the former `tests/modules/http/proxy_gate.cc` and `dispatch_gate.cc`,
/// against the pinned `HttpProxy` in swoole-rs-oracle. The candidate is the owner
/// and the functions above, which the hybrid calls in place of `HttpProxy`'s bodies.
#[cfg(test)]
mod tests {
    use super::*;
    use swoole_rs_corebase::{
        PreparedProxy, swr_v1_core_proxy_free, swr_v1_core_proxy_new, swr_v1_core_string_set,
        swr_v1_core_string_view,
    };
    use swoole_rs_oracle::http_proxy as oracle;

    fn errno() -> i32 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }
    fn views<'a>(texts: [&'a [u8]; 3]) -> [CoreStringView; 3] {
        texts.map(|text: &'a [u8]| CoreStringView {
            data: text.as_ptr().cast(),
            length: text.len(),
        })
    }

    /// An `HttpProxy`: its Rust owner and the views the C++ class reads.
    struct Proxy(PreparedProxy);
    impl Proxy {
        fn new() -> Self {
            Self(swr_v1_core_proxy_new(false))
        }
        /// As the gates built theirs: every field assigned, whatever is empty.
        fn with(user: &[u8], password: &[u8], target: &[u8], target_port: i32) -> Self {
            let proxy = Self::new();
            for (index, value) in [(1, user), (2, password), (3, target)] {
                assert_eq!(proxy.set(index, value), 0, "field {index}");
            }
            unsafe { (*proxy.0.fields).target_port = target_port };
            proxy
        }
        /// `HttpProxy::create`.
        fn create(host: &[u8], port: i32, user: &[u8], password: &[u8]) -> Self {
            let proxy = Self::new();
            let [host, user, password] = views([host, user, password]);
            unsafe {
                swr_v1_http_proxy_configure(
                    proxy.0.owner,
                    host.data,
                    host.length,
                    port,
                    user.data,
                    user.length,
                    password.data,
                    password.length,
                );
            };
            proxy
        }
        fn set(&self, index: usize, value: &[u8]) -> i32 {
            unsafe {
                swr_v1_core_string_set(self.0.strings[index], value.as_ptr().cast(), value.len())
            }
        }
        fn get(&self, index: usize) -> Vec<u8> {
            let view = unsafe { swr_v1_core_string_view(self.0.strings[index]) };
            unsafe { std::slice::from_raw_parts(view.data.cast::<u8>(), view.length) }.to_vec()
        }
        fn fields(&self) -> [Vec<u8>; 3] {
            [0, 1, 2].map(|index| self.get(index))
        }
        fn port(&self) -> i32 {
            unsafe { (*self.0.fields).port }
        }
        /// `get_auth_str`, and errno after it from EINTR.
        fn auth(&self) -> (Vec<u8>, i32) {
            set_errno(libc::EINTR);
            (authorization(unsafe { &*self.0.owner }), errno())
        }
        /// `pack` into `output[..capacity]`, and errno after it from EINTR.
        fn pack(&self, host: &[u8], output: &mut [u8], capacity: usize) -> (usize, i32) {
            set_errno(libc::EINTR);
            let written = unsafe {
                swr_v1_http_proxy_connect(
                    self.0.owner,
                    host.as_ptr(),
                    host.len(),
                    output.as_mut_ptr(),
                    capacity,
                )
            };
            (written, errno())
        }
    }
    impl Drop for Proxy {
        fn drop(&mut self) {
            unsafe { swr_v1_core_proxy_free(self.0.owner) };
        }
    }

    fn upstream_auth(user: &[u8], password: &[u8]) -> (Vec<u8>, i32) {
        let (mut output, mut error) = ([0u8; 512], 0);
        let length = unsafe {
            oracle::auth(
                user.as_ptr(),
                user.len(),
                password.as_ptr(),
                password.len(),
                output.as_mut_ptr(),
                &raw mut error,
            )
        };
        (output[..length].to_vec(), error)
    }
    fn upstream_pack(
        [user, password, target, host]: [&[u8]; 4],
        port: i32,
        output: &mut [u8],
        capacity: usize,
    ) -> (usize, i32) {
        let mut error = 0;
        let written = unsafe {
            oracle::pack(
                user.as_ptr(),
                user.len(),
                password.as_ptr(),
                password.len(),
                target.as_ptr(),
                target.len(),
                port,
                host.as_ptr(),
                host.len(),
                output.as_mut_ptr(),
                capacity,
                &raw mut error,
            )
        };
        (written, error)
    }

    /// One pack configuration over every short prefix, exact capacity, spare
    /// capacity and size query; returns the cases run.
    fn pack(target: &[u8], host: &[u8], user: &[u8], password: &[u8], port: i32) -> usize {
        let after = Proxy::with(user, password, target, port);
        let texts = [user, password, target, host];
        let (required, _) = upstream_pack(texts, port, &mut [], 0);
        let mut capacities: Vec<usize> = (0..=(required + 2).min(64)).collect();
        capacities.extend([required, required + 1, required + 2]);
        for &capacity in &capacities {
            let mut a = vec![0xcc_u8; capacity + 2];
            let mut b = a.clone();
            let before = upstream_pack(texts, port, &mut a, capacity);
            assert_eq!(
                before,
                after.pack(host, &mut b, capacity),
                "proxy request differential"
            );
            assert_eq!(a, b, "proxy request bytes");
        }
        capacities.len()
    }

    #[test]
    fn ported_proxy_gate_pack() {
        let long = [b'z'; 512];
        let text: [&[u8]; 6] = [b"", b"localhost", b"[::1]", b"ab\0cd", &long, b"x\xffy"];
        let mut packed = 0;
        for target in text {
            for host in text {
                for port in [0, 80, -1, 65535, i32::MIN, i32::MAX] {
                    for credential in text {
                        packed += pack(target, host, credential, credential, port);
                    }
                }
            }
        }
        packed += pack(b"target", b"host", b"", b"secret", 443);
        packed += pack(b"target", b"host", b"user", b"", 443);
        assert!(packed > 6 * 6 * 6 * 6 * 3, "pack cases {packed}");
    }

    #[test]
    fn ported_proxy_gate_credentials() {
        let mut credentials = 0;
        let sizes = [0, 1, 253, 254, 255, 256, 512, 1_048_576];
        for u in sizes {
            for p in sizes {
                let (mut user, mut password) = (vec![b'u'; u], vec![b'p'; p]);
                for variant in 0..4 {
                    if variant == 1 && !user.is_empty() {
                        let middle = user.len() / 2;
                        user[middle] = 0;
                    }
                    if variant == 2 && !password.is_empty() {
                        let middle = password.len() / 2;
                        password[middle] = 0;
                    }
                    if variant == 3 && !user.is_empty() {
                        user[0] = 0;
                    }
                    let after = Proxy::with(&user, &password, b"target", 443);
                    assert_eq!(
                        upstream_auth(&user, &password),
                        after.auth(),
                        "fixed credential buffer/auth/errno differential"
                    );
                    credentials += 1;
                }
            }
        }
        assert_eq!(credentials, 256, "credential cases");
    }

    fn handshake(input: &[u8]) {
        let mut last_error = 0;
        let accepted =
            unsafe { oracle::handshake(input.as_ptr(), input.len(), &raw mut last_error) };
        assert_eq!(errno(), libc::EINTR, "upstream handshake keeps errno");
        log::set_last_error(0);
        let candidate = unsafe { swr_v1_http_proxy_accept(input.as_ptr(), input.len()) };
        assert_eq!(errno(), libc::EINTR, "candidate handshake keeps errno");
        assert_eq!(
            (accepted, last_error),
            (candidate, log::last_error()),
            "proxy handshake/last-error differential"
        );
    }

    #[test]
    fn ported_proxy_gate_handshake() {
        let mut responses: Vec<Vec<u8>> = [
            &b"HTTP/1.1 200 Connection established\r\n"[..],
            b"http/1.0 200 a different message with trailing bytes",
            b"HTTP/1.1X200 Connection established\r\n",
            b"HTTP/1.1 407 Proxy authorization needed\r\n",
            b"HTTP/1.1 \t\x0b\x0c\r\n200 Connection established\r\n",
        ]
        .map(<[u8]>::to_vec)
        .to_vec();
        responses.push([&b"HTTP/1.1 "[..], &[b' '; 60]].concat());
        responses.push([&b"HTTP/1.1 \0"[..], &[b'X'; 40]].concat());
        for input in &responses {
            for n in 0..=input.len() {
                handshake(&input[..n]);
            }
        }
        let mut seed: u32 = 90271;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for i in 0..8192_u32 {
            let mut input = match i % 3 {
                0 => b"HTTP/1.1 ".to_vec(),
                1 => b"HTTP/1.0X".to_vec(),
                _ => b"invalid".to_vec(),
            };
            let spaces = next() % 15;
            input.extend((0..spaces).map(|_| b' '));
            if i % 2 == 1 {
                input.extend_from_slice(b"200");
            }
            while input.len() < 36 + usize::try_from(next() % 50).unwrap() {
                input.push(u8::try_from(next() % 128).unwrap());
            }
            handshake(&input);
        }
    }

    fn upstream_create(host: &[u8], user: &[u8], password: &[u8]) -> (i32, [Vec<u8>; 3]) {
        let mut output = vec![0u8; host.len() + user.len() + password.len()];
        let mut lengths = [0usize; 3];
        let port = unsafe {
            oracle::create(
                host.as_ptr(),
                host.len(),
                -123,
                user.as_ptr(),
                user.len(),
                password.as_ptr(),
                password.len(),
                output.as_mut_ptr(),
                &raw mut lengths,
            )
        };
        let (host, rest) = output.split_at(lengths[0]);
        let (user, rest) = rest.split_at(lengths[1]);
        (port, [host, user, &rest[..lengths[2]]].map(<[u8]>::to_vec))
    }

    /// The factory and authorization differential, and a later host assignment
    /// that leaves the other fields alone.
    #[test]
    fn ported_dispatch_gate() {
        let long_name = [b'x'; 200];
        let mut proxies = 0;
        let hosts: [&[u8]; 3] = [b"", b"host", &long_name];
        let users: [&[u8]; 3] = [b"", b"u\0tail", &long_name];
        let passwords: [&[u8]; 3] = [b"", b"p\0tail", &long_name];
        for host in hosts {
            for user in users {
                for password in passwords {
                    let authenticated = !user.is_empty() && !password.is_empty();
                    let (expected_user, expected_password): (&[u8], &[u8]) = if authenticated {
                        (user, password)
                    } else {
                        (b"", b"")
                    };
                    let expected = [host, expected_user, expected_password].map(<[u8]>::to_vec);
                    assert_eq!(
                        upstream_create(host, user, password),
                        (-123, expected.clone()),
                        "native factory fields"
                    );

                    let current = Proxy::create(host, -123, user, password);
                    assert_eq!(
                        (current.port(), current.fields()),
                        (-123, expected.clone()),
                        "real candidate factory fields"
                    );
                    assert_eq!(
                        current.auth(),
                        upstream_auth(expected_user, expected_password),
                        "actual owner authorization/errno matches pinned control"
                    );
                    assert_eq!(
                        current.fields(),
                        expected,
                        "authorization preserves owner fields"
                    );

                    let replacement = [b'z'; 600];
                    assert_eq!(current.set(0, &replacement), 0, "host assignment");
                    assert_eq!(current.get(0), replacement, "host assignment value");
                    assert_eq!(
                        current.fields()[1..],
                        expected[1..],
                        "publication preserves other fields"
                    );
                    proxies += 1;
                }
            }
        }
        assert_eq!(proxies, 27, "proxy cases");
    }
}
