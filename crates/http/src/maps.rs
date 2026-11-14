use std::ffi::CStr;
pub(super) static METHODS: [&CStr; 27] = [
    c"DELETE",
    c"GET",
    c"HEAD",
    c"POST",
    c"PUT",
    c"PATCH",
    c"CONNECT",
    c"OPTIONS",
    c"TRACE",
    c"COPY",
    c"LOCK",
    c"MKCOL",
    c"MOVE",
    c"PROPFIND",
    c"PROPPATCH",
    c"UNLOCK",
    c"REPORT",
    c"MKACTIVITY",
    c"CHECKOUT",
    c"MERGE",
    c"M-SEARCH",
    c"NOTIFY",
    c"SUBSCRIBE",
    c"UNSUBSCRIBE",
    c"PURGE",
    c"QUERY",
    c"PRI",
];
/// The legacy status line for `code`, or None for an unknown code.
pub(crate) fn status(code: i32) -> Option<&'static CStr> {
    Some(match code {
        100 => c"100 Continue",
        101 => c"101 Switching Protocols",
        102 => c"102 Processing",
        103 => c"103 Early Hints",
        201 => c"201 Created",
        202 => c"202 Accepted",
        203 => c"203 Non-Authoritative Information",
        204 => c"204 No Content",
        205 => c"205 Reset Content",
        206 => c"206 Partial Content",
        207 => c"207 Multi-Status",
        208 => c"208 Already Reported",
        226 => c"226 IM Used",
        300 => c"300 Multiple Choices",
        301 => c"301 Moved Permanently",
        302 => c"302 Found",
        303 => c"303 See Other",
        304 => c"304 Not Modified",
        305 => c"305 Use Proxy",
        306 => c"306 (Unused)",
        307 => c"307 Temporary Redirect",
        308 => c"308 Permanent Redirect",
        400 => c"400 Bad Request",
        401 => c"401 Unauthorized",
        402 => c"402 Payment Required",
        403 => c"403 Forbidden",
        404 => c"404 Not Found",
        405 => c"405 Method Not Allowed",
        406 => c"406 Not Acceptable",
        407 => c"407 Proxy Authentication Required",
        408 => c"408 Request Timeout",
        409 => c"409 Conflict",
        410 => c"410 Gone",
        411 => c"411 Length Required",
        412 => c"412 Precondition Failed",
        413 => c"413 Payload Too Large",
        414 => c"414 URI Too Long",
        415 => c"415 Unsupported Media Type",
        416 => c"416 Range Not Satisfiable",
        417 => c"417 Expectation Failed",
        418 => c"418 I'm a teapot",
        421 => c"421 Misdirected Request",
        422 => c"422 Unprocessable Entity",
        423 => c"423 Locked",
        424 => c"424 Failed Dependency",
        425 => c"425 Too Early",
        426 => c"426 Upgrade Required",
        428 => c"428 Precondition Required",
        429 => c"429 Too Many Requests",
        431 => c"431 Request Header Fields Too Large",
        451 => c"451 Unavailable For Legal Reasons",
        500 => c"500 Internal Server Error",
        501 => c"501 Not Implemented",
        502 => c"502 Bad Gateway",
        503 => c"503 Service Unavailable",
        504 => c"504 Gateway Timeout",
        505 => c"505 HTTP Version Not Supported",
        506 => c"506 Variant Also Negotiates",
        507 => c"507 Insufficient Storage",
        508 => c"508 Loop Detected",
        510 => c"510 Not Extended",
        511 => c"511 Network Authentication Required",
        0 | 200 => c"200 OK",
        _ => return None,
    })
}
/// `get_method`: the method's number (1-based), or -1.
#[must_use]
pub fn method(input: &[u8]) -> i32 {
    METHODS
        .iter()
        .position(|name| super::case_equal(name.to_bytes(), input))
        .map_or(-1, |index| i32::try_from(index + 1).unwrap_or(-1))
}
/// `get_method_string`: the method's name, or None.
#[must_use]
pub fn method_string(method: i32) -> Option<&'static CStr> {
    usize::try_from(method)
        .ok()
        .and_then(|id| id.checked_sub(1))
        .and_then(|index| METHODS.get(index))
        .copied()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn method_ids_roundtrip_and_reject_unknowns() {
        for (index, name) in METHODS.iter().enumerate() {
            let lower = name.to_bytes().to_ascii_lowercase();
            let id = i32::try_from(index + 1).unwrap();
            assert_eq!(method(&lower), id);
            assert_eq!(method_string(id), Some(*name));
        }
        assert_eq!(method(b"UNKNOWN"), -1);
        for id in [i32::MIN, 0, 28, i32::MAX] {
            assert_eq!(method_string(id), None);
        }
    }
}
