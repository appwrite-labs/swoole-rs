//! The hybrid's `redis::parse`, `redis::recv_packet` driver and `Socks5Proxy` over
//! Rust's codecs, as the oracle's shims drive the pinned ones (`crates/oracle/shim`'s
//! protocols.cc, resp_stream.cc and socks5.cc): each logs what its shim logs.
use std::ffi::{CStr, c_char, c_int, c_void};
use swoole_rs_oracle::socks5::Case;
use swoole_rs_protocols::{
    RespRequest, resp_spans, socks_auth, socks_config, socks_connect, socks_negotiate,
    socks_strerror, socks_transition,
};
use swoole_rs_protocols_ffi::{RESP_RECV_DONE, RespConnection, SW_ERR, recv_packet};

/// `redis::parse` over [`resp_spans`]: each string after its u32 length (an integer
/// span in decimal; nothing for a malformed wire), and whether a number overflowed a
/// signed long, where native code sets `ERANGE`.
pub fn resp_parse(data: &[u8]) -> (Vec<u8>, bool) {
    let (mut items, mut range) = (Vec::new(), false);
    let count = resp_spans(
        data,
        |span| {
            let decimal = span.integer.to_string();
            let item = if span.kind == 1 {
                decimal.as_bytes()
            } else {
                &data[span.offset..span.offset + span.length]
            };
            items.extend_from_slice(&(item.len() as u32).to_ne_bytes());
            items.extend_from_slice(item);
        },
        &mut range,
    );
    if count.is_none() {
        items.clear();
    }
    (items, range)
}

/// The shim's socket, String and connection: each recv hands out at most the next
/// chunk, then would block (or reads the peer's close).
struct Stream {
    chunks: Vec<Vec<u8>>,
    close_at_end: bool,
    content: Vec<u8>,
    size: usize,
    offset: i64,
    request: RespRequest,
    log: Vec<u8>,
}

impl RespConnection for Stream {
    fn recv(&mut self) -> isize {
        let Some(chunk) = self.chunks.first_mut() else {
            return if self.close_at_end { 0 } else { RESP_RECV_DONE };
        };
        let count = chunk.len().min(self.size - self.content.len());
        self.content.extend(chunk.drain(..count));
        if chunk.is_empty() {
            self.chunks.remove(0);
        }
        count as isize
    }

    fn view(&mut self) -> (&[u8], usize, &mut RespRequest) {
        (&self.content, self.size, &mut self.request)
    }

    fn set_offset(&mut self, offset: i64) {
        self.offset = offset;
    }

    fn extend(&mut self, size: usize) {
        self.size = size;
    }

    fn dispatch(&mut self) -> c_int {
        self.log
            .extend_from_slice(format!("P{}:", self.content.len()).as_bytes());
        self.log.extend_from_slice(&self.content);
        0
    }

    fn clear(&mut self) {
        self.content.clear();
        self.offset = 0;
    }
}

/// resp_stream.cc's driver over Rust's loop: `recv_packet` until it fails or the
/// chunks are drained, logged as `swoole_rs_oracle::resp_stream::UPSTREAM` logs the
/// pinned one.
pub fn resp_stream(
    data: &[u8],
    chunks: &[u16],
    buffer_size: usize,
    maximum: u32,
    close_at_end: bool,
) -> Vec<u8> {
    let mut rest = data;
    let mut stream = Stream {
        chunks: Vec::new(),
        close_at_end,
        content: Vec::new(),
        size: buffer_size,
        offset: 0,
        request: RespRequest::default(),
        log: Vec::new(),
    };
    for &chunk in chunks {
        if rest.is_empty() {
            break;
        }
        let (chunk, after) = rest.split_at(usize::from(chunk).min(rest.len()));
        stream.chunks.push(chunk.to_vec());
        rest = after;
    }
    for _ in 0..4096 {
        let drained = stream.chunks.is_empty();
        let result = recv_packet(&mut stream, maximum, 4096);
        stream.log.extend_from_slice(format!("R{result}").as_bytes());
        if result == SW_ERR {
            break;
        }
        let r = stream.request;
        let state = format!(
            ",{},{}/{},{}/{};",
            r.state, r.lines_received, r.lines_total, r.bytes_received, r.bytes_total
        );
        stream.log.extend_from_slice(state.as_bytes());
        if drained {
            let buffer = format!("L{}O{}", stream.content.len(), stream.offset);
            stream.log.extend_from_slice(buffer.as_bytes());
            break;
        }
    }
    stream.log
}

unsafe extern "C" {
    /// `inet_pton(3)`, which the shim's `Address::assign` calls.
    fn inet_pton(family: c_int, source: *const c_char, address: *mut c_void) -> c_int;
}

/// The hybrid's `Socks5Proxy` as socks5.cc had it: upstream's members, each method's
/// wire work done by Rust's codec.
struct Socks5Proxy<'a> {
    state: u8,
    method: u8,
    dns_tunnel: bool,
    username: &'a [u8],
    password: &'a [u8],
    target_host: &'a [u8],
    target_port: u16,
    socket_type: u8,
    buf: [u8; 512],
}

impl Socks5Proxy<'_> {
    fn pack_negotiate_request(&mut self) -> i64 {
        match socks_negotiate(!self.username.is_empty(), &mut self.buf) {
            Some(method) => {
                self.method = method;
                3
            }
            None => -1,
        }
    }

    fn pack_auth_request(&mut self) -> i64 {
        socks_auth(self.username, self.password, &mut self.buf)
    }

    /// The shim's `Address::assign`: `inet_pton` of the host up to its first NUL, for
    /// an inet4 (1, 2) or inet6 (3, 4) socket type.
    fn literal(&self) -> Option<(u8, Vec<u8>)> {
        let (family, kind, size) = match self.socket_type {
            1 | 2 => (libc::AF_INET, 1, 4),
            3 | 4 => (libc::AF_INET6, 4, 16),
            _ => return None,
        };
        let host = [self.target_host, &[0]].concat();
        let host = CStr::from_bytes_until_nul(&host).ok()?;
        let mut address = [0u8; 16];
        // SAFETY: a NUL-terminated host and room for either family's address.
        let parsed = unsafe { inet_pton(family, host.as_ptr(), address.as_mut_ptr().cast()) };
        (parsed == 1).then(|| (kind, address[..size].to_vec()))
    }

    fn pack_connect_request(&mut self) -> i64 {
        // The prefix stays even when address validation fails.
        self.buf[..3].copy_from_slice(&[5, 1, 0]);
        let (kind, address) = if self.dns_tunnel {
            (3, self.target_host.to_vec())
        } else {
            match self.literal() {
                Some(literal) => literal,
                None => return -1,
            }
        };
        match socks_connect(kind, &address, self.target_port, &mut self.buf) {
            -2 => -1,
            size => size,
        }
    }

    fn handshake(&mut self, data: &[u8], send: &mut dyn FnMut(&[u8]) -> i64) -> bool {
        let decision = socks_transition(self.state, self.method, data);
        if decision.error != 0 {
            return false;
        }
        self.state = decision.state;
        if decision.action == 1 || decision.action == 2 {
            let size = if decision.action == 1 {
                self.pack_auth_request()
            } else {
                self.pack_connect_request()
            };
            return size >= 0 && send(&self.buf[..size as usize]) == size;
        }
        true
    }
}

/// socks5.cc's driver over [`Socks5Proxy`], logged as
/// `swoole_rs_oracle::socks5::UPSTREAM` logs the pinned one. The slices are the
/// case's own fields.
pub fn socks5(
    case: &Case,
    user: &[u8],
    password: &[u8],
    target_host: &[u8],
    mut replies: &[u8],
    reply_sizes: &[u16],
) -> Vec<u8> {
    let mut log = Vec::new();
    // `create`: 0 refuses the credentials, 2 keeps both, 1 keeps neither.
    let config = socks_config(user.len(), password.len());
    if config == 0 {
        log.push(b'N');
        return log;
    }
    let (username, password) = if config == 2 {
        (user, password)
    } else {
        (&[][..], &[][..])
    };
    let mut proxy = Socks5Proxy {
        state: 0,
        method: 0,
        dns_tunnel: case.dns_tunnel != 0,
        username,
        password,
        target_host,
        target_port: case.target_port,
        socket_type: case.socket_type,
        buf: [0; 512],
    };
    // The shim's send: logged, and one byte short when the case says so.
    let short = i64::from(case.send_short != 0);
    let send = |log: &mut Vec<u8>, data: &[u8]| {
        log.extend_from_slice(format!("S{}:", data.len()).as_bytes());
        log.extend_from_slice(data);
        data.len() as i64 - short
    };
    let length = proxy.pack_negotiate_request();
    send(&mut log, &proxy.buf[..length as usize]);
    // SW_SOCKS5_STATE_HANDSHAKE, until SW_SOCKS5_STATE_READY.
    proxy.state = 1;
    for &size in reply_sizes {
        if proxy.state == 4 {
            break;
        }
        let (reply, rest) = replies.split_at(usize::from(size));
        replies = rest;
        let ok = proxy.handshake(reply, &mut |data| send(&mut log, data));
        let line = format!("H{},{},{};", u8::from(ok), proxy.state, proxy.method);
        log.extend_from_slice(line.as_bytes());
        if let [_, code, ..] = *reply {
            log.extend_from_slice(socks_strerror(i32::from(code)).to_bytes());
        }
        if !ok {
            break;
        }
    }
    log
}
