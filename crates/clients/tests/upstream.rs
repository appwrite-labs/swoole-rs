//! Differential tests against the pinned upstream C++ (`swoole-rs-oracle`).

use swoole_rs_clients::swr_v1_clients_cookie;
use swoole_rs_oracle::clients as upstream;

/// `php_swoole_http_parse_set_cookies`: the key, value offset and value length it
/// splits a `Set-Cookie` value into, which fix every Zend effect it makes (the
/// value it decodes, the key it stores under and the raw header it appends).
#[test]
fn set_cookie_split_matches_upstream() {
    let mut cookies: Vec<Vec<u8>> = [
        "",
        "a=b",
        "=b",
        "abc",
        "a=",
        "a=b;c=d",
        "a;b=c",
        "a=b=c",
        "a=%20+; path=/",
    ]
    .iter()
    .map(|cookie| cookie.as_bytes().to_vec())
    .collect();
    cookies.push(b"a\0x=b\0c;d".to_vec());
    for byte in 0..=255u8 {
        for prefix in [&b""[..], b"a=", b"a;", b"="] {
            cookies.push([prefix, &[byte], b"=x;y"].concat());
        }
    }
    for cookie in &cookies {
        let (mut key, mut offset, mut value) = (0, 0, 0);
        // SAFETY: the oracle reads `cookie.len()` bytes and writes three counts.
        unsafe {
            upstream::cookie(
                cookie.as_ptr().cast(),
                cookie.len(),
                &raw mut key,
                &raw mut offset,
                &raw mut value,
            );
        }
        // SAFETY: as above.
        let plan = unsafe { swr_v1_clients_cookie(cookie.as_ptr(), cookie.len()) };
        assert_eq!(
            (plan.key_length, plan.value_offset, plan.value_length),
            (key, offset, value),
            "{cookie:?}"
        );
    }
    assert_eq!(cookies.len(), 1034);
}
