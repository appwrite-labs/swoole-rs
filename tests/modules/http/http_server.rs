// Upstream's `core-tests/src/server/http_server.cpp` TESTs that ran a request through
// the `http_server::listen` SDK (`src/wrapper/http.cc`, deleted): `parser1`, `parser2`,
// `upload` and `max_request_size`. The same requests go through the server's HTTP
// reader (`readable_callback_http`, crates/ports/src/read.rs, which includes this file
// in its tests) under the same port settings, and the request the reader dispatches is
// checked for what the SDK's handler asserted.
use super::*;
use swoole_rs_corebase::constants::SW_HTTP_REQUEST_ENTITY_TOO_LARGE_PACKET;
use swoole_rs_oracle::cases::{REQ1, REQ2};

const MB: u64 = 1024 * 1024;
/// `SW_INPUT_BUFFER_SIZE`: a port's default `package_max_length`.
const INPUT_BUFFER_SIZE: u64 = 2 * MB;
/// `SW_BUFFER_SIZE_BIG`: a connection's receive buffer, and what one `recv` reads.
const BUFFER_SIZE_BIG: usize = 65536;

/// A port with `package_max_length` `max` and `upload_max_filesize` `uploads`, whose
/// client sends `request`.
fn server(request: &[u8], max: u64, uploads: u64) -> Fake {
    let mut f = fake(&request.chunks(BUFFER_SIZE_BIG).collect::<Vec<_>>());
    (f.bytes, f.max, f.uploads) = (vec![0; BUFFER_SIZE_BIG], max, uploads);
    f
}

/// The reader runs on each readable event, until the client has sent everything.
fn serve(f: &mut Fake) {
    loop {
        let left = f.incoming.len();
        run(ON_READ_HTTP, f);
        if f.incoming.is_empty() || f.incoming.len() == left {
            return;
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// `examples/test.jpg` (`test::get_jpg_file()`), the file `req2.bin` uploads.
fn jpg() -> &'static [u8] {
    let start = find(REQ2, b"filename=").unwrap();
    let start = start + find(&REQ2[start..], b"\r\n\r\n").unwrap() + 4;
    &REQ2[start..REQ2.len() - b"\r\n--------------------------cc0882704733751a--\r\n".len()]
}

/// What `curl [-H 'Transfer-Encoding: chunked'] -F "file=@<jpg>"
/// http://127.0.0.1:<port>/upload?test=curl` sends.
fn curl(chunked: bool) -> Vec<u8> {
    let boundary = "------------------------d74496d66958873e";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.jpg\"\r\n\
         Content-Type: image/jpeg\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(jpg());
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let framing = if chunked {
        "Transfer-Encoding: chunked".to_owned()
    } else {
        format!("Content-Length: {}", body.len())
    };
    let mut request = format!(
        "POST /upload?test=curl HTTP/1.1\r\nHost: 127.0.0.1:9501\r\nUser-Agent: curl/8.5.0\r\n\
         Accept: */*\r\n{framing}\r\nContent-Type: multipart/form-data; boundary={boundary}\r\n\r\n"
    )
    .into_bytes();
    if chunked {
        for chunk in body.chunks(BUFFER_SIZE_BIG) {
            request.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
            request.extend_from_slice(chunk);
            request.extend_from_slice(b"\r\n");
        }
        request.extend_from_slice(b"0\r\n\r\n");
    } else {
        request.extend_from_slice(&body);
    }
    request
}

/// A chunked body's data.
fn dechunk(mut body: &[u8]) -> Vec<u8> {
    let mut data = Vec::new();
    loop {
        let line = find(body, b"\r\n").unwrap();
        let size = usize::from_str_radix(core::str::from_utf8(&body[..line]).unwrap(), 16).unwrap();
        if size == 0 {
            return data;
        }
        data.extend_from_slice(&body[line + 2..line + 2 + size]);
        body = &body[line + 2 + size + 2..];
    }
}

/// A dispatched multipart request's parts: the form fields and the files, whose
/// `Swoole-Upload-File` paths (a part the server streamed to disk) come too.
fn parts(request: &[u8]) -> (usize, usize, Vec<String>) {
    let head = find(request, b"\r\n\r\n").unwrap() + 4;
    let boundary = find(&request[..head], b"boundary=").unwrap() + 9;
    let end = boundary + find(&request[boundary..], b"\r\n").unwrap();
    let delimiter = [b"--", &request[boundary..end]].concat();
    let body = if find(&request[..head], b"Transfer-Encoding: chunked").is_some() {
        dechunk(&request[head..])
    } else {
        request[head..].to_vec()
    };
    let (mut forms, mut files, mut uploads) = (0, 0, Vec::new());
    let mut rest = &body[..];
    while let Some(at) = find(rest, &delimiter) {
        rest = &rest[at + delimiter.len()..];
        let Some(headers) = find(rest, b"\r\n\r\n").map(|end| &rest[..end]) else {
            break;
        };
        let text = String::from_utf8_lossy(headers);
        for line in text.split("\r\n") {
            if let Some(path) = line.strip_prefix("Swoole-Upload-File: ") {
                uploads.push(path.to_owned());
            }
            if line.starts_with("Content-Disposition:") {
                if line.contains("filename=") {
                    files += 1;
                } else {
                    forms += 1;
                }
            }
        }
    }
    (forms, files, uploads)
}

// http_server.parser1
#[test]
fn parser1() {
    let mut f = server(REQ1, INPUT_BUFFER_SIZE, 0);
    serve(&mut f);
    assert!(f.sent.is_empty(), "200 OK: no error response");
    assert_eq!(f.dispatched, [REQ1.to_vec()], "the request reaches the handler");
    assert_eq!(parts(&f.dispatched[0]).0, 3, "ctx.form_data.size()");
}

// http_server.parser2
#[test]
fn parser2() {
    let mut f = server(REQ2, 64 * 1024, MB);
    serve(&mut f);
    assert!(f.sent.is_empty(), "200 OK: no error response");
    assert_eq!(f.dispatched.len(), 1, "the request reaches the handler");
    let (forms, files, uploads) = parts(&f.dispatched[0]);
    assert_eq!(forms, 3, "ctx.form_data.size()");
    assert_eq!((files, uploads.len()), (1, 1), "the file streamed to disk");
    assert_eq!(std::fs::read(&uploads[0]).unwrap(), jpg(), "the upload");
    std::fs::remove_file(&uploads[0]).unwrap();
}

// http_server.upload
#[test]
fn upload() {
    let request = curl(true);
    let mut f = server(&request, 2 * MB, 0);
    serve(&mut f);
    assert!(f.sent.is_empty(), "the handler answers, not the server");
    assert_eq!(f.dispatched, [request], "the request reaches the handler");
    assert_eq!(parts(&f.dispatched[0]).1, 1, "ctx.files.size()");
    let url = String::from_utf8(f.urls[0].clone()).unwrap();
    assert_eq!(url.split_once('?'), Some(("/upload", "test=curl")), "request_path, query_string");
}

// http_server.max_request_size
#[test]
fn max_request_size() {
    let mut f = server(&curl(false), 128 * 1024, 0);
    serve(&mut f);
    assert_eq!(f.sent, [Reply::TooLarge], "refused");
    assert!(f.dispatched.is_empty(), "never reaches the handler");
    assert!(
        SW_HTTP_REQUEST_ENTITY_TOO_LARGE_PACKET.contains("413 Request Entity Too Large"),
        "the response"
    );
    logged();
}

// http_server.ignore_http1_request_trailers (core-tests/src/server/http_request_trailer.cpp)
#[test]
fn ignore_http1_request_trailers() {
    let request: &[u8] = b"POST / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\
        Content-Type: text/plain\r\nCookie: initial=yes\r\n\
        Transfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\
        Content-Type: multipart/form-data\r\nCookie: extra=no\r\n\
        X-Trailer: ignored\r\n\r\n";
    let mut f = server(request, INPUT_BUFFER_SIZE, 0);
    serve(&mut f);
    assert!(f.sent.is_empty(), "200 OK: no error response");
    assert_eq!(f.dispatched, [request.to_vec()], "the request, trailers too, reaches the handler");
    // The handler's headers are the head's; the trailer section changes none.
    let head = &f.dispatched[0][..find(&f.dispatched[0], b"\r\n\r\n").unwrap()];
    let value = |name: &[u8]| -> Vec<&[u8]> {
        head.split(|&b| b == b'\n')
            .filter_map(|line| line.trim_ascii().strip_prefix(name)?.strip_prefix(b": "))
            .collect()
    };
    assert_eq!(value(b"Content-Type"), [b"text/plain"], "ctx.headers[\"Content-Type\"]");
    assert_eq!(value(b"Cookie"), [b"initial=yes"], "ctx.headers[\"Cookie\"]");
    assert!(value(b"X-Trailer").is_empty(), "ctx.headers.count(\"X-Trailer\") == 0");
}
