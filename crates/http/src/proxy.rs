//! CONNECT request formatting and the pinned proxy response state machine.
use crate::{OVERFLOW, c_text, tail};
use std::io::Write;

/// A CONNECT request's texts; each is read as `%.*s`, up to its first NUL.
pub struct HttpProxyRequest<'a> {
    pub target: &'a [u8],
    pub host: &'a [u8],
    pub version: &'a [u8],
    pub authorization: &'a [u8],
    pub port: i32,
    pub authenticated: bool,
}
/// Format the pinned fixed 256-byte `username:password` staging buffer: 0, or 1
/// when native code publishes EOVERFLOW (the formatted count overflows `int`), with
/// the length written. The useful prefix never exceeds 255 bytes.
#[must_use]
pub fn proxy_credentials(user: &[u8], password: &[u8], output: &mut [u8; 256]) -> (i32, usize) {
    // Only a potentially int-overflowing formatted count needs full C-string
    // lengths. Native snprintf would reject that count even with a short buffer.
    if user
        .len()
        .checked_add(password.len())
        .and_then(|length| length.checked_add(1))
        .is_none_or(|length| length > i32::MAX as usize)
        && c_text(user)
            .len()
            .checked_add(c_text(password).len())
            .and_then(|length| length.checked_add(1))
            .is_none_or(|length| length > i32::MAX as usize)
    {
        output[0] = 0;
        return (1, 0);
    }
    let (buffer, _) = output.split_at_mut(255);
    let mut length = 0;
    for (index, value) in [user, password].into_iter().enumerate() {
        if index == 1 && length < 255 {
            if let Some(colon) = buffer.get_mut(length) {
                *colon = b':';
            }
            length += 1;
        }
        let limit = value.len().min(255 - length);
        let input = c_text(value.get(..limit).unwrap_or_default());
        if let Some(to) = buffer.get_mut(length..length + input.len()) {
            to.copy_from_slice(input);
        }
        length += input.len();
    }
    if let Some(nul) = output.get_mut(length) {
        *nul = 0;
    }
    (0, length)
}

/// The credentials, formatted and Base64-encoded into `output`, NUL-terminated:
/// [`proxy_credentials`]'s status and the encoded length.
#[must_use]
pub fn proxy_authorization(user: &[u8], password: &[u8], output: &mut [u8; 512]) -> (i32, usize) {
    let mut plain = [0u8; 256];
    let (status, count) = proxy_credentials(user, password, &mut plain);
    // At most 255 bytes encode to 340 characters and a NUL.
    let mut text = [0u8; 341];
    let encoded = swoole_rs_base64::encode(plain.get(..count).unwrap_or_default(), &mut text);
    if let (Some(to), Some(from)) = (output.get_mut(..=encoded), text.get(..=encoded)) {
        to.copy_from_slice(from);
    }
    (status, encoded)
}

/// The CONNECT request, as upstream's `snprintf` formats it before cutting it to
/// the caller's buffer.
///
/// # Errors
/// `OVERFLOW` for a request beyond libc's `int`.
pub fn proxy_pack(request: &HttpProxyRequest<'_>) -> Result<Vec<u8>, i32> {
    let [target, host, version, authorization] = [
        request.target,
        request.host,
        request.version,
        request.authorization,
    ]
    .map(c_text);
    // libc's formatted-result type is int. Reject larger results before writing,
    // rather than reproducing signed narrowing.
    let digits = request.port.unsigned_abs().checked_ilog10().unwrap_or(0) as usize
        + 1
        + usize::from(request.port < 0);
    let fixed = b"CONNECT : HTTP/1.1\r\nHost: :\r\nUser-Agent: Swoole/\r\nProxy-Connection: Keep-Alive\r\n\r\n".len() + 2 * digits;
    let lengths = [
        target.len(),
        host.len(),
        version.len(),
        if request.authenticated {
            b"Proxy-Authorization: Basic ".len() + authorization.len() + 2
        } else {
            0
        },
    ];
    let Some(total) = lengths
        .into_iter()
        .try_fold(fixed, usize::checked_add)
        .filter(|&n| i32::try_from(n).is_ok())
    else {
        return Err(OVERFLOW);
    };
    let mut out = Vec::with_capacity(total);
    let written = (|| {
        out.write_all(b"CONNECT ")?;
        out.write_all(target)?;
        write!(out, ":{} HTTP/1.1\r\nHost: ", request.port)?;
        out.write_all(host)?;
        write!(out, ":{}\r\nUser-Agent: Swoole/", request.port)?;
        out.write_all(version)?;
        out.write_all(b"\r\nProxy-Connection: Keep-Alive\r\n")?;
        if request.authenticated {
            out.write_all(b"Proxy-Authorization: Basic ")?;
            out.write_all(authorization)?;
            out.write_all(b"\r\n")?;
        }
        out.write_all(b"\r\n")
    })();
    written.map_err(|_| OVERFLOW)?;
    Ok(out)
}
/// A proxy's response to CONNECT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handshake {
    /// Status 200 with more than four bytes after it.
    Accepted,
    /// Another status (native sets last-error).
    Rejected,
    /// Not HTTP/1.x, or incomplete.
    Incomplete,
}

/// Pinned handshake; minimum length and cursor skips are intentional.
#[must_use]
pub fn proxy_handshake(input: &[u8]) -> Handshake {
    if input.len() < b"HTTP/1.1 200 Connection established\r\n".len() {
        return Handshake::Incomplete;
    }
    if !crate::case_prefix(input, b"HTTP/1.1") && !crate::case_prefix(input, b"HTTP/1.0") {
        return Handshake::Incomplete;
    }
    // The original loop skips index 8 after recognizing HTTP/1.x.
    let Some(cursor) = tail(input, 9)
        .iter()
        .position(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c))
    else {
        return Handshake::Incomplete;
    };
    let cursor = cursor + 9;
    if !tail(input, cursor).starts_with(b"200") {
        return Handshake::Rejected;
    }
    if input.len() - cursor > 4 {
        Handshake::Accepted
    } else {
        Handshake::Incomplete
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_preserve_fixed_prefix_and_native_nul_rules() {
        for size in [0, 1, 253, 254, 255, 256, 1024] {
            let username = vec![b'u'; size];
            let mut output = [0xcc; 256];
            let (status, written) = proxy_credentials(&username, b"password\0ignored", &mut output);
            assert_eq!(status, 0);
            let mut expected = username.clone();
            expected.push(b':');
            expected.extend_from_slice(b"password");
            expected.truncate(255);
            assert_eq!(written, expected.len());
            assert_eq!(&output[..written], expected);
            assert_eq!(output[written], 0);
        }
    }
    #[test]
    fn authorization_reuses_codec_and_keeps_output_bounds() {
        let mut output = [0xcc; 512];
        let (status, count) = proxy_authorization(b"user", b"password", &mut output);
        assert_eq!(status, 0);
        assert_eq!(&output[..count], b"dXNlcjpwYXNzd29yZA==");
        assert_eq!(output[count], 0);
        assert_eq!(output[count + 1], 0xcc);
        let (_, count) = proxy_authorization(b"", b"", &mut output);
        assert_eq!(&output[..count], b"Og==");
    }
    #[test]
    fn pack_prefix_and_query() {
        let request = HttpProxyRequest {
            target: b"t\0ignored",
            host: b"h",
            version: b"6.2.2",
            authorization: b"YWJj",
            port: i32::MIN,
            authenticated: true,
        };
        let expected=b"CONNECT t:-2147483648 HTTP/1.1\r\nHost: h:-2147483648\r\nUser-Agent: Swoole/6.2.2\r\nProxy-Connection: Keep-Alive\r\nProxy-Authorization: Basic YWJj\r\n\r\n";
        assert_eq!(proxy_pack(&request).unwrap(), expected);
        let plain = HttpProxyRequest {
            authenticated: false,
            port: 80,
            ..request
        };
        assert_eq!(
            proxy_pack(&plain).unwrap(),
            b"CONNECT t:80 HTTP/1.1\r\nHost: h:80\r\nUser-Agent: Swoole/6.2.2\r\nProxy-Connection: Keep-Alive\r\n\r\n"
        );
    }
    #[test]
    fn handshake_cursor_and_error_policy() {
        let check = proxy_handshake;
        assert_eq!(check(b""), Handshake::Incomplete);
        assert_eq!(
            check(b"HTTP/1.1 200 Connection established\r\n"),
            Handshake::Accepted
        );
        assert_eq!(
            check(b"http/1.0X200 Connection established\r\n"),
            Handshake::Accepted
        );
        assert_eq!(
            check(b"HTTP/1.1 407 Proxy authorization needed\r\n"),
            Handshake::Rejected
        );
        assert_eq!(check(b"HTTP/1.1 200"), Handshake::Incomplete);
        assert_eq!(
            check(b"NOT HTTP 200 Connection established\r\n"),
            Handshake::Incomplete
        );
        let mut input = b"HTTP/1.1 ".to_vec();
        input.resize(36, b' ');
        assert_eq!(check(&input), Handshake::Incomplete);
        input.extend_from_slice(b"200");
        assert_eq!(check(&input), Handshake::Incomplete);
        input.push(b'!');
        assert_eq!(check(&input), Handshake::Incomplete);
        input.push(b'!');
        assert_eq!(check(&input), Handshake::Accepted);
    }
}
