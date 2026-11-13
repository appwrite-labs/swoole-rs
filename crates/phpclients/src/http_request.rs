//! The HTTP client's byte-level rules, free of Zend: request formatting through
//! upstream's fixed `sw_snprintf` buffers, multipart sizes, header token lists,
//! random tokens and response header semantics.
use swoole_rs_clients::http_format::{Format, http_format};

/// `SW_HTTP_CLIENT_BOUNDARY_PREKEY` and `SW_HTTP_CLIENT_BOUNDARY_TOTAL_SIZE`.
const BOUNDARY_PREFIX: &[u8] = b"----SwooleBoundary";
pub(crate) const BOUNDARY_SIZE: usize = 39;
/// `SW_HTTP_FORM_RAW_DATA_FMT` and `SW_HTTP_FORM_FILE_DATA_FMT` without their
/// `%.*s` directives (`sizeof(fmt) - FMT_LEN - 1`).
const FIELD_OVERHEAD: usize =
    b"--%.*s\r\nContent-Disposition: form-data; name=\"%.*s\"\r\n\r\n".len() - 8;
const FILE_OVERHEAD: usize = b"--%.*s\r\nContent-Disposition: form-data; name=\"%.*s\"; filename=\"%.*s\"\r\nContent-Type: %.*s\r\n\r\n".len() - 16;
/// `header_buf[2048]` and `content_length_str[64]`.
const PART_BUFFER: usize = 2048;
const LENGTH_BUFFER: usize = 64;
/// `SW_STACK_BUFFER_SIZE`, the bound of `swoole_http_token_list_contains_value`.
const TOKEN_LIST_LIMIT: usize = 8192;

/// `sw_snprintf` into a `capacity`-byte buffer, appended to `out`.
fn format(out: &mut Vec<u8>, kind: Format, spans: [&[u8]; 4], number: usize, capacity: usize) {
    let mut buffer = vec![0_u8; capacity];
    let result = http_format(kind, spans.map(Some), number, buffer.as_mut_slice());
    out.extend_from_slice(buffer.get(..result.written).unwrap_or(&buffer));
}

/// `Key: value\r\n` (`add_headers`).
pub(crate) fn header(out: &mut Vec<u8>, key: &[u8], value: &[u8]) {
    out.extend_from_slice(key);
    out.extend_from_slice(b": ");
    out.extend_from_slice(value);
    out.extend_from_slice(b"\r\n");
}

/// `Content-Length: n\r\n\r\n` (`add_content_length`).
pub(crate) fn content_length(out: &mut Vec<u8>, length: usize) {
    format(out, Format::ContentLength, [b""; 4], length, LENGTH_BUFFER);
}

/// The absolute URI of a request through a plain HTTP proxy.
pub(crate) fn proxy_uri(out: &mut Vec<u8>, host: &[u8], path: &[u8], port: u16) {
    let capacity = path.len() + host.len() + b"http://".len() + 10;
    format(
        out,
        Format::ProxyUri,
        [host, path, b"", b""],
        usize::from(port),
        capacity,
    );
}

/// The `Host` header's value when none was given: the port only when it is
/// not the scheme's default.
pub(crate) fn host_value(host: &[u8], port: u16, ssl: bool) -> Vec<u8> {
    let default = if ssl { 443 } else { 80 };
    if port == default {
        return host.to_vec();
    }
    // std_string::format("%s:%u"): the host stops at a NUL.
    let mut value = host.split(|&b| b == 0).next().unwrap_or_default().to_vec();
    value.push(b':');
    value.extend_from_slice(port.to_string().as_bytes());
    value
}

/// `network::Socket::convert_to_type(host)`: the socket type a host implies
/// (`SW_SOCK_TCP`, `SW_SOCK_TCP6`, `SW_SOCK_UNIX_STREAM`); a `unix:/` host
/// loses its scheme and all but one leading slash.
pub(crate) fn socket_type(host: &mut Vec<u8>) -> i64 {
    if host.starts_with(b"unix:/") {
        host.drain(..b"unix:".len());
        match host.iter().position(|&b| b != b'/') {
            Some(first) => drop(host.drain(..first - 1)),
            None => host.clear(),
        }
        return 5;
    }
    if host.contains(&b':') { 3 } else { 1 }
}

/// A multipart boundary: the prefix and 21 random alphanumerics.
pub(crate) fn boundary() -> [u8; BOUNDARY_SIZE] {
    const CHARACTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut random = [0_u8; BOUNDARY_SIZE];
    fill_random(&mut random);
    let mut characters = random.iter().map(|&byte| pick(CHARACTERS, byte));
    let mut out = [0_u8; BOUNDARY_SIZE];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = match BOUNDARY_PREFIX.get(index) {
            Some(&prefix) => prefix,
            None => characters.next().unwrap_or(b'0'),
        };
    }
    out
}

/// `create_token`: `length` bytes from upstream's table (whose `§` is two bytes).
pub(crate) fn token<const N: usize>() -> [u8; N] {
    const CHARACTERS: &[u8] =
        b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ!\"\xc2\xa7$%&/()=[]{}";
    let mut random = [0_u8; N];
    fill_random(&mut random);
    random.map(|byte| pick(CHARACTERS, byte))
}

/// `characters[byte % characters.len()]`.
fn pick(characters: &[u8], byte: u8) -> u8 {
    let index = usize::from(byte) % characters.len();
    characters.get(index).copied().unwrap_or_default()
}

/// `out` from the system CSPRNG; any part it leaves unfilled comes from the
/// clock rather than stay a predictable zero token.
fn fill_random(out: &mut [u8]) {
    let filled = swoole_rs_corebase::random_bytes(out);
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos())
        .to_le_bytes();
    let rest = out.iter_mut().enumerate().skip(filled);
    for ((index, byte), seed) in rest.zip(seed.iter().cycle()) {
        *byte = seed ^ u8::try_from(index & 0xff).unwrap_or(0);
    }
}

/// `Content-Type: multipart/form-data; boundary=...\r\n`.
pub(crate) fn multipart_type(out: &mut Vec<u8>, boundary: &[u8]) {
    format(
        out,
        Format::MultipartType,
        [boundary, b"", b"", b""],
        0,
        PART_BUFFER,
    );
}

/// A form field's part header.
pub(crate) fn field_header(out: &mut Vec<u8>, boundary: &[u8], name: &[u8]) {
    format(
        out,
        Format::Field,
        [boundary, name, b"", b""],
        0,
        PART_BUFFER,
    );
}

/// A file's part header.
pub(crate) fn file_header(boundary: &[u8], name: &[u8], filename: &[u8], kind: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    format(
        &mut out,
        Format::File,
        [boundary, name, filename, kind],
        0,
        PART_BUFFER,
    );
    out
}

/// The closing boundary.
pub(crate) fn closing(boundary: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    format(
        &mut out,
        Format::Closing,
        [boundary, b"", b"", b""],
        0,
        PART_BUFFER,
    );
    out
}

/// The bytes a form field adds to a multipart body, as upstream counts them.
pub(crate) fn field_length(name: usize, value: usize) -> usize {
    FIELD_OVERHEAD + BOUNDARY_SIZE + name + value + 2
}

/// The bytes a file adds to a multipart body, as upstream counts them.
pub(crate) fn file_length(name: usize, filename: usize, kind: usize, size: i64) -> usize {
    (FILE_OVERHEAD + BOUNDARY_SIZE + name + filename + kind)
        .wrapping_add_signed(isize::try_from(size).unwrap_or(isize::MAX))
        + 2
}

/// The multipart body's total: the parts and the closing boundary.
pub(crate) fn multipart_length(parts: usize) -> usize {
    parts + BOUNDARY_SIZE + 6
}

/// `swoole_strnpos(data, "\r\n\r\n")`.
pub(crate) fn header_end(data: &[u8]) -> Option<usize> {
    data.windows(4).position(|window| window == b"\r\n\r\n")
}

/// `swoole_http_token_list_contains_value`: -1 when the list is too long to
/// check (upstream callers treat that as found), 1 when found, else 0.
pub(crate) fn token_list_contains(list: &[u8], value: &[u8]) -> i32 {
    if list.is_empty() {
        return 0;
    }
    if list.len() >= TOKEN_LIST_LIMIT {
        return -1;
    }
    if list.eq_ignore_ascii_case(value) {
        return 1;
    }
    // strtok over a NUL-terminated copy: the list stops at its first NUL.
    let found = list
        .split(|&b| b == 0)
        .next()
        .unwrap_or_default()
        .split(|&b| b == b',')
        .filter(|token| !token.is_empty())
        .any(|token| trim(token).eq_ignore_ascii_case(value));
    i32::from(found)
}

/// `swoole::ltrim` then `swoole::rtrim` (C `isspace`).
fn trim(mut token: &[u8]) -> &[u8] {
    let space = |b: &u8| matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r');
    while let Some((first, rest)) = token.split_first()
        && space(first)
    {
        token = rest;
    }
    while let Some((last, rest)) = token.split_last()
        && space(last)
    {
        token = rest;
    }
    token
}

/// `SW_STR_ISTARTS_WITH`.
pub(crate) fn starts_with_ignore_case(value: &[u8], prefix: &[u8]) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// `swoole_strncasestr(value, needle)`.
pub(crate) fn contains_ignore_case(value: &[u8], needle: &[u8]) -> bool {
    needle.is_empty()
        || value
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

/// The `Accept-Encoding` the client offers, per the compression Swoole was
/// built with (`features`: 1 zlib, 2 brotli, 4 zstd); None without any.
pub(crate) fn accept_encoding(features: u32) -> Option<&'static [u8]> {
    match (features & 1 != 0, features & 2 != 0, features & 4 != 0) {
        (true, true, _) => Some(b"gzip, deflate, br"),
        (true, false, _) => Some(b"gzip, deflate"),
        (false, true, _) => Some(b"br"),
        (false, false, true) => Some(b"zstd"),
        (false, false, false) => None,
    }
}

/// `swHttpCompressMethod`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Compress {
    None = 0,
    Gzip = 1,
    Deflate = 2,
    Br = 3,
    Zstd = 4,
}

/// A `Content-Encoding` value's method, per the compression Swoole was built
/// with; None leaves the current method.
pub(crate) fn content_encoding(value: &[u8], features: u32) -> Option<Compress> {
    if features & 2 != 0 && starts_with_ignore_case(value, b"br") {
        return Some(Compress::Br);
    }
    if features & 1 != 0 {
        if starts_with_ignore_case(value, b"gzip") {
            return Some(Compress::Gzip);
        }
        if starts_with_ignore_case(value, b"deflate") {
            return Some(Compress::Deflate);
        }
    }
    if features & 4 != 0 && starts_with_ignore_case(value, b"zstd") {
        return Some(Compress::Zstd);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_format_through_the_fixed_buffers() {
        let mut out = Vec::new();
        content_length(&mut out, 42);
        assert_eq!(out, b"Content-Length: 42\r\n\r\n", "content length");
        let mut out = Vec::new();
        proxy_uri(&mut out, b"example.com", b"/a?b", 8080);
        assert_eq!(out, b"http://example.com:8080/a?b", "proxy URI with port");
        let mut out = Vec::new();
        proxy_uri(&mut out, b"example.com:81", b"/", 80);
        assert_eq!(
            out, b"http://example.com:81/",
            "a host with a port keeps it"
        );
        let mut out = Vec::new();
        field_header(&mut out, b"B", b"name");
        assert_eq!(
            out, b"--B\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\n",
            "field part"
        );
        assert_eq!(
            file_header(b"B", b"f", b"a.txt", b"text/plain"),
            b"--B\r\nContent-Disposition: form-data; name=\"f\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\n",
            "file part"
        );
        assert_eq!(closing(b"B"), b"--B--\r\n", "closing boundary");
        let long = vec![b'x'; 3000];
        assert_eq!(
            file_header(b"B", &long, b"", b"").len(),
            PART_BUFFER - 1,
            "snprintf truncates at the buffer"
        );
    }

    #[test]
    fn multipart_lengths_count_as_upstream() {
        let boundary = [b'b'; BOUNDARY_SIZE];
        let mut field = Vec::new();
        field_header(&mut field, &boundary, b"key");
        assert_eq!(
            field_length(3, 5),
            field.len() + 5 + 2,
            "field: header, value, CRLF"
        );
        let file = file_header(&boundary, b"n", b"f.txt", b"text/plain");
        assert_eq!(
            file_length(1, 5, 10, 7),
            file.len() + 7 + 2,
            "file: header, content, CRLF"
        );
        assert_eq!(
            multipart_length(0),
            closing(&boundary).len(),
            "closing boundary"
        );
    }

    #[test]
    fn hosts_imply_socket_types() {
        let mut host = b"unix:///tmp/a.sock".to_vec();
        assert_eq!(socket_type(&mut host), 5, "unix socket");
        assert_eq!(host, b"/tmp/a.sock", "one leading slash kept");
        let mut host = b"unix:/b".to_vec();
        assert_eq!(socket_type(&mut host), 5, "one slash");
        assert_eq!(host, b"/b", "path");
        assert_eq!(socket_type(&mut b"::1".to_vec()), 3, "IPv6");
        assert_eq!(socket_type(&mut b"example.com".to_vec()), 1, "TCP");
    }

    #[test]
    fn host_header_names_a_non_default_port() {
        assert_eq!(host_value(b"h", 80, false), b"h", "default http port");
        assert_eq!(host_value(b"h", 443, true), b"h", "default https port");
        assert_eq!(
            host_value(b"h", 443, false),
            b"h:443",
            "https port over http"
        );
        assert_eq!(host_value(b"h", 8080, true), b"h:8080", "custom port");
    }

    #[test]
    fn random_tokens_use_their_tables() {
        let boundary = boundary();
        assert!(boundary.starts_with(BOUNDARY_PREFIX), "boundary prefix");
        assert!(
            boundary[BOUNDARY_PREFIX.len()..]
                .iter()
                .all(u8::is_ascii_alphanumeric),
            "boundary is alphanumeric"
        );
        assert_eq!(token::<16>().len(), 16, "token length");
    }

    #[test]
    fn token_lists_match_case_insensitively() {
        assert_eq!(token_list_contains(b"", b"websocket"), 0, "empty");
        assert_eq!(
            token_list_contains(b"WebSocket", b"websocket"),
            1,
            "whole value"
        );
        assert_eq!(
            token_list_contains(b"keep-alive, Upgrade", b"Upgrade"),
            1,
            "a listed token"
        );
        assert_eq!(
            token_list_contains(b"a,,  b ", b"b"),
            1,
            "trimmed, empty skipped"
        );
        assert_eq!(token_list_contains(b"a\0, b", b"b"), 0, "stops at a NUL");
        assert_eq!(token_list_contains(b"close", b"Upgrade"), 0, "absent");
        assert_eq!(
            token_list_contains(&[b'a'; TOKEN_LIST_LIMIT], b"b"),
            -1,
            "too long to check"
        );
    }

    #[test]
    fn content_encodings_follow_the_build() {
        assert_eq!(content_encoding(b"GZIP", 1), Some(Compress::Gzip), "gzip");
        assert_eq!(
            content_encoding(b"deflate", 1),
            Some(Compress::Deflate),
            "deflate"
        );
        assert_eq!(content_encoding(b"br", 1), None, "no brotli in the build");
        assert_eq!(content_encoding(b"br", 3), Some(Compress::Br), "brotli");
        assert_eq!(content_encoding(b"zstd", 4), Some(Compress::Zstd), "zstd");
        assert_eq!(accept_encoding(1), Some(&b"gzip, deflate"[..]), "zlib");
        assert_eq!(
            accept_encoding(3),
            Some(&b"gzip, deflate, br"[..]),
            "zlib and brotli"
        );
        assert_eq!(accept_encoding(4), Some(&b"zstd"[..]), "zstd only");
        assert_eq!(accept_encoding(0), None, "none");
        assert!(
            contains_ignore_case(b"x, Permessage-Deflate", b"permessage-deflate"),
            "strncasestr"
        );
        assert_eq!(header_end(b"a\r\n\r\nb"), Some(1), "header end");
    }
}
