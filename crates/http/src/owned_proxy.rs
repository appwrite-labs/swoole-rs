//! HTTP CONNECT construction reads the canonical Rust proxy owner directly.
use crate::{
    HttpProxyRequest, HttpProxyText, swr_v1_http_proxy_pack, swr_v2_http_proxy_authorization,
};
use swoole_rs_corebase::ProxyState;

/// # Safety
/// owner is live/stable; output lends512 bytes and written is writable/disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_proxy_owned_authorization(
    owner: *const ProxyState,
    output: *mut u8,
    written: *mut usize,
) -> i32 {
    // SAFETY: caller lends the live owner's unchanged string slots.
    let owner = unsafe { &*owner };
    // SAFETY: the username is stable through the bounded codec.
    let user = unsafe { owner.string(1) };
    // SAFETY: the password is stable through the bounded codec.
    let password = unsafe { owner.string(2) };
    let credentials = [
        HttpProxyText {
            data: user.data.cast(),
            length: user.length,
        },
        HttpProxyText {
            data: password.data.cast(),
            length: password.length,
        },
    ];
    // SAFETY: stable credentials and exclusive512-byte output/length match the codec contract.
    unsafe { swr_v2_http_proxy_authorization(credentials.as_ptr(), output, written) }
}
/// # Safety
/// owner/input spans remain stable; output and written are exclusive/disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_proxy_owned_pack(
    owner: *const ProxyState,
    host: HttpProxyText,
    version: HttpProxyText,
    authorization: HttpProxyText,
    output: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> i32 {
    // SAFETY: caller lends a live stable owner for request construction.
    let owner = unsafe { &*owner };
    // SAFETY: leaf scalar snapshot before formatting.
    let fields = unsafe { *owner.fields.get() };
    // SAFETY: the target bytes remain stable throughout formatting.
    let target = unsafe { owner.string(3) };
    // SAFETY: the password slot remains stable throughout formatting.
    let password = unsafe { owner.string(2) };
    let request = HttpProxyRequest {
        target: HttpProxyText {
            data: target.data.cast(),
            length: target.length,
        },
        host,
        version,
        authorization,
        port: fields.target_port,
        authenticated: u32::from(password.length != 0),
    };
    // SAFETY: unchanged owner/spans and disjoint exclusive output follow the codec contract.
    unsafe { swr_v1_http_proxy_pack(&raw const request, output, capacity, written) }
}

/// The cases of the former `tests/modules/http/proxy_gate.cc` and `dispatch_gate.cc`,
/// against the pinned `HttpProxy` in swoole-rs-oracle. The candidate is the owner
/// and the Rust calls the hybrid's `HttpProxy` makes, with its errno, empty-string
/// and last-error mapping (`src/protocol/http.cc`).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{swr_v1_http_proxy_auth_enabled, swr_v1_http_proxy_handshake};
    use std::ffi::{c_char, c_void};
    use swoole_rs_corebase::{
        CoreStringView, PreparedProxy, swr_v1_core_proxy_configure, swr_v1_core_proxy_free,
        swr_v1_core_proxy_new, swr_v1_core_string_cache, swr_v1_core_string_set,
        swr_v1_core_string_view,
    };
    use swoole_rs_oracle::http_proxy as oracle;

    const VERSION: &[u8] = b"6.2.4";
    const HANDSHAKE_FAILED: i32 = 7103;

    fn errno() -> i32 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }
    fn span(value: &[u8]) -> HttpProxyText {
        HttpProxyText {
            data: value.as_ptr(),
            length: value.len(),
        }
    }
    unsafe extern "C" fn refuse(_: *mut c_void, _: *const c_char, _: usize) -> bool {
        false
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
        /// `HttpProxy::create`, refusing the native cache of field `refused`.
        fn create(
            host: &[u8],
            port: i32,
            user: &[u8],
            password: &[u8],
            refused: Option<usize>,
        ) -> (Self, i32, usize) {
            let proxy = Self::new();
            if let Some(index) = refused {
                proxy.refuse(index, true);
            }
            let enabled = swr_v1_http_proxy_auth_enabled(user.len(), password.len()) == 1;
            let texts = views([host, user, password]);
            let result = unsafe {
                swr_v1_core_proxy_configure(proxy.0.owner, texts.as_ptr(), port, enabled, -1)
            };
            if let Some(index) = refused {
                proxy.refuse(index, false);
            }
            (proxy, result.status, usize::try_from(result.index).unwrap())
        }
        /// Registers (or detaches) a native cache on field `index` that refuses
        /// every publication, as a failing C++ allocation does.
        fn refuse(&self, index: usize, refusing: bool) {
            let update = refusing.then_some(refuse as unsafe extern "C" fn(_, _, _) -> _);
            unsafe {
                swr_v1_core_string_cache(self.0.strings[index], std::ptr::null_mut(), update);
            };
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
            let (mut output, mut length) = ([0u8; 512], 0);
            let result = unsafe {
                swr_v1_http_proxy_owned_authorization(
                    self.0.owner,
                    output.as_mut_ptr(),
                    &raw mut length,
                )
            };
            if result < 0 {
                return (Vec::new(), libc::EINTR);
            }
            let end = output
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(output.len());
            (
                output[..end].to_vec(),
                if result == 1 {
                    libc::EOVERFLOW
                } else {
                    libc::EINTR
                },
            )
        }
        /// `pack` into `output[..capacity]`, and errno after it from EINTR.
        fn pack(&self, host: &[u8], output: &mut [u8], capacity: usize) -> (usize, i32) {
            let (authorization, error) = if self.get(2).is_empty() {
                (Vec::new(), libc::EINTR)
            } else {
                self.auth()
            };
            let mut written = 0;
            unsafe {
                swr_v1_http_proxy_owned_pack(
                    self.0.owner,
                    span(host),
                    span(VERSION),
                    span(&authorization),
                    output.as_mut_ptr(),
                    capacity,
                    &raw mut written,
                )
            };
            (written, error)
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
        let status = unsafe { swr_v1_http_proxy_handshake(input.as_ptr(), input.len()) };
        assert_eq!(errno(), libc::EINTR, "candidate handshake keeps errno");
        let candidate_error = if status == -2 { HANDSHAKE_FAILED } else { 0 };
        assert_eq!(
            (accepted, last_error),
            (status == 1, candidate_error),
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

    /// The factory and authorization differential, and what a refused assignment
    /// leaves: create assigns the host, then the port, then the username, then the
    /// password, and stops at a refusal; a refused cache publication keeps both values.
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

                    let (current, status, _) = Proxy::create(host, -123, user, password, None);
                    assert_eq!(
                        (status, current.port(), current.fields()),
                        (0, -123, expected.clone()),
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

                    for refused in 0..if authenticated { 3 } else { 1 } {
                        let (proxy, status, index) =
                            Proxy::create(host, -123, user, password, Some(refused));
                        assert_eq!((status, index), (-3, refused), "candidate factory refusal");
                        assert_eq!(
                            proxy.port(),
                            if refused == 0 { 0 } else { -123 },
                            "port follows the host"
                        );
                        for (index, value) in proxy.fields().iter().enumerate() {
                            let assigned = if index < refused {
                                expected[index].as_slice()
                            } else {
                                b""
                            };
                            assert_eq!(
                                value.as_slice(),
                                assigned,
                                "fields before the refusal only"
                            );
                        }
                    }

                    let replacement = [b'z'; 600];
                    current.refuse(0, true);
                    assert_eq!(current.set(0, &replacement), -3, "candidate cache refusal");
                    assert_eq!(
                        current.fields(),
                        expected,
                        "candidate cache refusal preserves Rust values"
                    );
                    current.refuse(0, false);
                    assert_eq!(
                        current.set(0, &replacement),
                        0,
                        "candidate cache publication"
                    );
                    assert_eq!(
                        current.get(0),
                        replacement,
                        "candidate cache publication value"
                    );
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
