//! `HttpProxy`'s create, pack and handshake over corebase's proxy owner, which the
//! clients and hooks call, over crates/http's CONNECT formatting.
use swoole_rs_corebase::{ProxyState, VERSION, log, set_errno};
use swoole_rs_http::{HttpProxyRequest, proxy_authorization, proxy_handshake, proxy_pack};

const HANDSHAKE_FAILED: i32 = 7103;

/// `HttpProxy::create`'s assignments on a fresh owner: the host and port, then the
/// credentials when both are set. Only an allocation can fail, and that aborts as
/// Rust's allocation failures do.
#[must_use]
pub fn http_proxy(host: &[u8], port: i32, user: &[u8], password: &[u8]) -> Box<ProxyState> {
    let proxy = ProxyState::new(false);
    let authenticated = !user.is_empty() && !password.is_empty();
    let result = proxy.configure([host, user, password], port, authenticated, -1);
    if result.status != 0 {
        std::process::abort();
    }
    proxy
}

/// `HttpProxy::get_auth_str()`: the Base64 credentials, errno EOVERFLOW when they
/// were cut to fit.
#[must_use]
pub fn authorization(owner: &ProxyState) -> Vec<u8> {
    let mut encoded = [0u8; 512];
    let (status, length) = proxy_authorization(&owner.string(1), &owner.string(2), &mut encoded);
    if status == 1 {
        set_errno(libc::EOVERFLOW);
    }
    let encoded = encoded.get(..length).unwrap_or_default();
    encoded
        .split(|&b| b == 0)
        .next()
        .unwrap_or_default()
        .to_vec()
}

/// `HttpProxy::pack`: the CONNECT request for `host` into `output`, NUL-terminated
/// and cut to fit, with the authorization when the proxy has a password. Returns
/// the bytes written (the whole length for an empty output).
pub fn proxy_connect(state: &ProxyState, host: &[u8], output: &mut [u8]) -> usize {
    let authenticated = !state.string(2).is_empty();
    let auth = if authenticated {
        authorization(state)
    } else {
        Vec::new()
    };
    let request = HttpProxyRequest {
        target: &state.string(3),
        host,
        version: VERSION.to_bytes(),
        authorization: &auth,
        port: state.fields().target_port,
        authenticated,
    };
    let Ok(text) = proxy_pack(&request) else {
        return 0;
    };
    if output.is_empty() {
        return text.len();
    }
    if output.len() > i32::MAX as usize {
        return 0;
    }
    let length = text.len().min(output.len() - 1);
    let (Some(to), Some(from)) = (output.get_mut(..length), text.get(..length)) else {
        return 0;
    };
    to.copy_from_slice(from);
    if let Some(end) = output.get_mut(length) {
        *end = 0;
    }
    length
}

/// `HttpProxy::handshake`: whether `input` accepts the CONNECT, setting the last
/// error when the proxy refused it.
#[must_use]
pub fn proxy_accept(input: &[u8]) -> bool {
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
    use swoole_rs_oracle::http_proxy as oracle;

    fn errno() -> i32 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }

    /// An `HttpProxy`: its Rust owner.
    struct Proxy(Box<ProxyState>);
    impl Proxy {
        /// As the gates built theirs: every field assigned, whatever is empty.
        fn with(user: &[u8], password: &[u8], target: &[u8], target_port: i32) -> Self {
            let proxy = Self(ProxyState::new(false));
            for (index, value) in [(1, user), (2, password), (3, target)] {
                assert_eq!(proxy.set(index, value), 0, "field {index}");
            }
            proxy.0.update(|fields| fields.target_port = target_port);
            proxy
        }
        /// `HttpProxy::create`.
        fn create(host: &[u8], port: i32, user: &[u8], password: &[u8]) -> Self {
            Self(http_proxy(host, port, user, password))
        }
        fn set(&self, index: usize, value: &[u8]) -> i32 {
            self.0.set_string(index, value)
        }
        fn get(&self, index: usize) -> Vec<u8> {
            self.0.string(index).to_vec()
        }
        fn fields(&self) -> [Vec<u8>; 3] {
            [0, 1, 2].map(|index| self.get(index))
        }
        fn port(&self) -> i32 {
            self.0.fields().port
        }
        /// `get_auth_str`, and errno after it from EINTR.
        fn auth(&self) -> (Vec<u8>, i32) {
            set_errno(libc::EINTR);
            (authorization(&self.0), errno())
        }
        /// `pack` into `output[..capacity]`, and errno after it from EINTR.
        fn pack(&self, host: &[u8], output: &mut [u8], capacity: usize) -> (usize, i32) {
            set_errno(libc::EINTR);
            let written = proxy_connect(&self.0, host, &mut output[..capacity]);
            (written, errno())
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
        let candidate = proxy_accept(input);
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
