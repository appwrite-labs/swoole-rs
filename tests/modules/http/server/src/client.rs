//! core-tests' `httplib::Client` (cpp-httplib 0.7, core-tests/src/_lib/http.cpp), as
//! far as the cases use it: `Get`, `Post` with form parameters, `set_keep_alive` and
//! `set_read_timeout`. A request that fails returns `None`, where httplib returns
//! `nullptr`; the reads follow httplib's: byte-wise lines, a status line that never
//! fails, headers up to a bare CRLF, and a body by `Transfer-Encoding: chunked`,
//! `Content-Length`, or else to the peer's close. A read that times out fails.

use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::os::fd::AsRawFd;
use std::time::Duration;

/// `httplib::USER_AGENT`.
pub const USER_AGENT: &str = "cpp-httplib/0.7";

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

const POLLOUT: i16 = 4;

unsafe extern "C" {
    fn poll(fds: *mut PollFd, count: u64, timeout: i32) -> i32;
}

pub struct Response {
    pub status: i32,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }
}

pub struct Client {
    host: String,
    port: u16,
    keep_alive: bool,
    read_timeout: Duration,
    socket: Option<TcpStream>,
}

impl Client {
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            host: host.to_owned(),
            port,
            keep_alive: false,
            // CPPHTTPLIB_READ_TIMEOUT_SECOND.
            read_timeout: Duration::from_secs(5),
            socket: None,
        }
    }

    pub fn set_keep_alive(&mut self, on: bool) {
        self.keep_alive = on;
    }

    pub fn set_read_timeout(&mut self, sec: u64, usec: u64) {
        self.read_timeout = Duration::from_secs(sec) + Duration::from_micros(usec);
    }

    pub fn get(&mut self, path: &str) -> Option<Response> {
        self.get_with(path, &[])
    }

    pub fn get_with(&mut self, path: &str, headers: &[(&str, &str)]) -> Option<Response> {
        self.send("GET", path, headers, None)
    }

    /// `Post(path, params)`: the parameters in key order, as `httplib::Params` (a
    /// multimap) holds them.
    pub fn post(&mut self, path: &str, params: &[(&str, &str)]) -> Option<Response> {
        let mut params = params.to_vec();
        params.sort_by_key(|(key, _)| *key);
        let body = params
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&");
        self.send(
            "POST",
            path,
            &[],
            Some(("application/x-www-form-urlencoded", body.into_bytes())),
        )
    }

    /// `Client::send`: reuse the socket while it is writable, else connect.
    fn send(
        &mut self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<(&str, Vec<u8>)>,
    ) -> Option<Response> {
        if let Some(socket) = &self.socket {
            let mut fd = PollFd {
                fd: socket.as_raw_fd(),
                events: POLLOUT,
                revents: 0,
            };
            // SAFETY: one readable and writable pollfd.
            if unsafe { poll(&mut fd, 1, 0) } <= 0 {
                self.socket = None;
            }
        }
        if self.socket.is_none() {
            let socket = TcpStream::connect((self.host.as_str(), self.port)).ok()?;
            // CPPHTTPLIB_TCP_NODELAY.
            socket.set_nodelay(true).ok()?;
            self.socket = Some(socket);
        }
        let close = !self.keep_alive;
        let socket = self.socket.as_mut()?;
        socket.set_read_timeout(Some(self.read_timeout)).ok()?;
        let response = request(
            socket, method, path, headers, body, close, &self.host, self.port,
        );
        let stop = response.as_ref().is_some_and(|(r, version)| {
            r.header("Connection") == Some("close") || version == "HTTP/1.0"
        });
        if close || stop {
            self.socket = None;
        }
        response.map(|(r, _)| r)
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "httplib's request, one argument per field"
)]
fn request(
    socket: &mut TcpStream,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<(&str, Vec<u8>)>,
    close: bool,
    host: &str,
    port: u16,
) -> Option<(Response, String)> {
    // write_request: the caller's headers, then httplib's defaults, in key order.
    let mut all: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    if close {
        all.push(("Connection".into(), "close".into()));
    }
    let host = if port == 80 {
        host.to_owned()
    } else {
        format!("{host}:{port}")
    };
    all.push(("Host".into(), host));
    all.push(("Accept".into(), "*/*".into()));
    all.push(("User-Agent".into(), USER_AGENT.into()));
    let payload = match body {
        Some((content_type, payload)) => {
            all.push(("Content-Type".into(), content_type.into()));
            all.push(("Content-Length".into(), payload.len().to_string()));
            payload
        }
        None => {
            all.push(("Content-Length".into(), "0".into()));
            Vec::new()
        }
    };
    all.sort_by_key(|(key, _)| key.to_ascii_lowercase());
    let mut out = format!("{method} {path} HTTP/1.1\r\n");
    for (key, value) in &all {
        out.push_str(&format!("{key}: {value}\r\n"));
    }
    out.push_str("\r\n");
    let mut out = out.into_bytes();
    out.extend_from_slice(&payload);
    socket.write_all(&out).ok()?;

    // read_response_line: a line that doesn't match leaves the status at -1.
    let line = getline(socket)?;
    let (version, status) = status_line(&line).unwrap_or_default();
    let mut response = Response {
        status: status.unwrap_or(-1),
        headers: Vec::new(),
        body: Vec::new(),
    };
    // read_headers.
    loop {
        let line = getline(socket)?;
        if !line.ends_with(b"\r\n") {
            continue;
        }
        if line.len() == 2 {
            break;
        }
        let mut end = line.len() - 2;
        while end > 0 && matches!(line[end - 1], b' ' | b'\t') {
            end -= 1;
        }
        let line = &line[..end];
        if let Some(colon) = line.iter().position(|&b| b == b':') {
            let mut value = colon + 1;
            while value < line.len() && matches!(line[value], b' ' | b'\t') {
                value += 1;
            }
            if value < line.len() {
                response.headers.push((
                    String::from_utf8_lossy(&line[..colon]).into_owned(),
                    String::from_utf8_lossy(&line[value..]).into_owned(),
                ));
            }
        }
    }
    // read_content.
    if method != "HEAD" {
        let chunked = response
            .header("Transfer-Encoding")
            .is_some_and(|v| v.eq_ignore_ascii_case("chunked"));
        let length = response.header("Content-Length").map(|v| {
            let digits: String = v.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u64>().unwrap_or(0)
        });
        let body = if chunked {
            read_chunked(socket)?
        } else if let Some(length) = length {
            read_exact(socket, length)?
        } else {
            let mut body = Vec::new();
            loop {
                let mut buf = [0; 4096];
                match read(socket, &mut buf)? {
                    0 => break body,
                    n => body.extend_from_slice(&buf[..n]),
                }
            }
        };
        response.body = body;
    }
    Some((response, version))
}

/// `(HTTP/1\.[01]) (\d+?) .*\r\n`, matched whole; `.` matches neither CR nor LF.
fn status_line(line: &[u8]) -> Option<(String, Option<i32>)> {
    let version = line.get(..8)?;
    if version != b"HTTP/1.0" && version != b"HTTP/1.1" {
        return None;
    }
    let rest = line.get(8..)?.strip_prefix(b" ")?.strip_suffix(b"\r\n")?;
    let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0
        || rest.get(digits) != Some(&b' ')
        || rest.contains(&b'\r')
        || rest.contains(&b'\n')
    {
        return None;
    }
    let status = std::str::from_utf8(&rest[..digits]).ok()?.parse().ok();
    Some((String::from_utf8_lossy(version).into_owned(), status))
}

/// One `recv`; `None` where httplib's read returns -1 (an error or the read timeout).
fn read(socket: &mut TcpStream, buf: &mut [u8]) -> Option<usize> {
    loop {
        match socket.read(buf) {
            Ok(n) => return Some(n),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
}

/// `stream_line_reader::getline`: bytes up to and including LF, or to the close; `None`
/// on an error or a close before the first byte.
fn getline(socket: &mut TcpStream) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    loop {
        let mut byte = [0];
        match read(socket, &mut byte)? {
            0 if line.is_empty() => return None,
            0 => return Some(line),
            _ => {
                line.push(byte[0]);
                if byte[0] == b'\n' {
                    return Some(line);
                }
            }
        }
    }
}

fn read_exact(socket: &mut TcpStream, length: u64) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    while (body.len() as u64) < length {
        let mut buf = [0; 4096];
        let want = usize::try_from(length - body.len() as u64)
            .unwrap_or(4096)
            .min(4096);
        match read(socket, &mut buf[..want])? {
            0 => return None,
            n => body.extend_from_slice(&buf[..n]),
        }
    }
    Some(body)
}

/// `read_content_chunked`.
fn read_chunked(socket: &mut TcpStream) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    let mut line = getline(socket)?;
    loop {
        let digits = line.iter().take_while(|b| b.is_ascii_hexdigit()).count();
        if digits == 0 {
            return None;
        }
        let length = u64::from_str_radix(std::str::from_utf8(&line[..digits]).ok()?, 16).ok()?;
        if length == 0 {
            break;
        }
        body.extend(read_exact(socket, length)?);
        if getline(socket)? != b"\r\n" {
            return Some(body);
        }
        line = getline(socket)?;
    }
    (getline(socket)? == b"\r\n").then_some(body)
}
