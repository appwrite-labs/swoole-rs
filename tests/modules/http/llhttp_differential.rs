//! Differential check: crates/http's llhttp against the oracle's C llhttp 9.3 on
//! generated requests and responses, cut into pieces, with callbacks that pause or
//! fail on a script. Every call's result, error position, reason and parser fields,
//! and every callback, must match. `CASES` and `SEED` widen a run.
#![expect(
    clippy::indexing_slicing,
    reason = "generated inputs index within their own lengths"
)]
use std::ffi::{CStr, c_char, c_int, c_void};
use swoole_rs_http::llhttp::{self, Callbacks, Parser as RParser};
use swoole_rs_oracle as _;

#[repr(C)]
struct CParser {
    index: i32,
    span_pos0: *mut c_void,
    span_cb0: *mut c_void,
    error: i32,
    reason: *const c_char,
    error_pos: *const c_char,
    data: *mut c_void,
    current: *mut c_void,
    content_length: u64,
    kind: u8,
    method: u8,
    http_major: u8,
    http_minor: u8,
    header_state: u8,
    lenient_flags: u16,
    upgrade: u8,
    finish: u8,
    flags: u16,
    status_code: u16,
    initial_message_completed: u8,
    settings: *const c_void,
}

type D = Option<unsafe extern "C" fn(*mut CParser, *const c_char, usize) -> c_int>;
type N = Option<unsafe extern "C" fn(*mut CParser) -> c_int>;

#[repr(C)]
struct Settings {
    on_message_begin: N,
    on_protocol: D,
    on_url: D,
    on_status: D,
    on_method: D,
    on_version: D,
    on_header_field: D,
    on_header_value: D,
    on_chunk_extension_name: D,
    on_chunk_extension_value: D,
    on_headers_complete: N,
    on_body: D,
    on_message_complete: N,
    rest: [N; 12],
}

unsafe extern "C" {
    fn llhttp_init(parser: *mut CParser, kind: c_int, settings: *const Settings);
    fn llhttp_execute(parser: *mut CParser, data: *const c_char, length: usize) -> c_int;
    fn llhttp_finish(parser: *mut CParser) -> c_int;
    fn llhttp_resume(parser: *mut CParser);
    fn llhttp_resume_after_upgrade(parser: *mut CParser);
    fn llhttp_should_keep_alive(parser: *const CParser) -> c_int;
    fn llhttp_message_needs_eof(parser: *const CParser) -> c_int;
}

/// Callback log and scripted returns.
#[derive(Default)]
struct Log {
    events: Vec<String>,
    script: Vec<i32>,
}

impl Log {
    fn ev(&mut self, name: &str, data: Option<&[u8]>) -> i32 {
        let r = self.script.get(self.events.len()).copied().unwrap_or(0);
        match data {
            Some(d) => self
                .events
                .push(format!("{name}({:?})={r}", String::from_utf8_lossy(d))),
            None => self.events.push(format!("{name}={r}")),
        }
        r
    }
}

unsafe fn log<'a>(p: *mut CParser) -> &'a mut Log {
    unsafe { &mut *(*p).data.cast::<Log>() }
}
unsafe fn sl<'a>(at: *const c_char, n: usize) -> &'a [u8] {
    if n == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(at.cast(), n) }
    }
}
unsafe extern "C" fn c_url(p: *mut CParser, at: *const c_char, n: usize) -> c_int {
    unsafe { log(p).ev("url", Some(sl(at, n))) }
}
unsafe extern "C" fn c_field(p: *mut CParser, at: *const c_char, n: usize) -> c_int {
    unsafe { log(p).ev("field", Some(sl(at, n))) }
}
unsafe extern "C" fn c_value(p: *mut CParser, at: *const c_char, n: usize) -> c_int {
    unsafe { log(p).ev("value", Some(sl(at, n))) }
}
unsafe extern "C" fn c_body(p: *mut CParser, at: *const c_char, n: usize) -> c_int {
    unsafe { log(p).ev("body", Some(sl(at, n))) }
}
unsafe extern "C" fn c_headers(p: *mut CParser) -> c_int {
    unsafe { log(p).ev("headers", None) }
}
unsafe extern "C" fn c_complete(p: *mut CParser) -> c_int {
    unsafe { log(p).ev("complete", None) }
}

static SETTINGS: Settings = Settings {
    on_message_begin: None,
    on_protocol: None,
    on_url: Some(c_url),
    on_status: None,
    on_method: None,
    on_version: None,
    on_header_field: Some(c_field),
    on_header_value: Some(c_value),
    on_chunk_extension_name: None,
    on_chunk_extension_value: None,
    on_headers_complete: Some(c_headers),
    on_body: Some(c_body),
    on_message_complete: Some(c_complete),
    rest: [None; 12],
};

struct R {
    parser: RParser,
    log: Log,
}

impl Callbacks for R {
    fn parser(&mut self) -> &mut RParser {
        &mut self.parser
    }
    fn on_url(&mut self, d: &[u8]) -> i32 {
        self.log.ev("url", Some(d))
    }
    fn on_header_field(&mut self, d: &[u8]) -> i32 {
        self.log.ev("field", Some(d))
    }
    fn on_header_value(&mut self, d: &[u8]) -> i32 {
        self.log.ev("value", Some(d))
    }
    fn on_body(&mut self, d: &[u8]) -> i32 {
        self.log.ev("body", Some(d))
    }
    fn on_headers_complete(&mut self) -> i32 {
        self.log.ev("headers", None)
    }
    fn on_message_complete(&mut self) -> i32 {
        self.log.ev("complete", None)
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the remainder is below n, a usize"
    )]
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, v: &[&'a str]) -> &'a str {
        v[self.below(v.len())]
    }
}

const METHODS: &[&str] = &[
    "GET",
    "POST",
    "HEAD",
    "PUT",
    "CONNECT",
    "OPTIONS",
    "PRI",
    "GET_PARAMETER",
    "SOURCE",
    "SETUP",
    "M-SEARCH",
    "QUERY",
    "PROPPATCH",
    "DESCRIBE",
    "GE",
    "GETX",
    "get",
    "PATCH",
    "UNLOCK",
];
const URLS: &[&str] = &[
    "/",
    "/a?b=c#d",
    "*",
    "http://h:80/p?q",
    "h:80",
    "/a b",
    "/\t",
    "http://u@h/",
    "http://u@@h",
    "/%20?x#y#z",
    "/a\x7f",
    "/\u{e9}",
    "",
    "x:y",
    "http:/x",
    "/q?a\"b",
];
const PROTOS: &[&str] = &[
    "HTTP/1.1",
    "HTTP/1.0",
    "HTTP/2.0",
    "HTTP/0.9",
    "HTTP/1.2",
    "RTSP/1.0",
    "ICE/1.0",
    "HTTP/1.",
    "HTTPS/1.1",
    "HTTP/x.1",
    "HTTP/11",
    "HTP/1.1",
];
const STATUS: &[&str] = &[
    "200 OK",
    "101 Switching",
    "100 Continue",
    "204",
    "304 NM",
    "404 Not Found",
    "1000 X",
    "20 X",
    "200",
    "199 X",
    "102 P",
    "abc",
];
const HEADERS: &[&str] = &[
    "Host: x",
    "Connection: close",
    "Connection: keep-alive",
    "Connection: Upgrade",
    "connection: keep-alive, upgrade",
    "Connection: closed, x",
    "Connection: CLOSE",
    "Connection: Keep-Alive ,close",
    "Connection: close\t",
    "Connection:close",
    "Connection: clos",
    "Connectionx: close",
    "Connectio: close",
    "CONTENT-LENGTH: 3",
    "Content-length : 3",
    "Proxy-Connection: close",
    "Upgrade: websocket",
    "Content-Length: 5",
    "Content-Length: 0",
    "content-length: 3",
    "Content-Length: 1 2",
    "Content-Length: 99999999999999999999999",
    "Content-Length:",
    "Content-Length: ",
    "Transfer-Encoding: chunked",
    "Transfer-Encoding: gzip, chunked",
    "Transfer-Encoding: chunked, gzip",
    "Transfer-Encoding: identity",
    "transfer-encoding: CHUNKED",
    "Transfer-Encoding: chunkedx",
    "X: a\tb",
    "X : y",
    "X:y",
    "X-Long: \u{e9}\u{e9}",
    ": y",
    "X\x01: y",
    "Expect: 100-continue",
    "X: y ",
    "X:\r\n y",
    "Upgrade: h2c",
];

fn generate(rng: &mut Rng) -> (u8, Vec<u8>) {
    if rng.below(2) == 0 {
        return body_case(rng);
    }
    let kind = if rng.below(3) == 0 { 2 } else { 1 };
    let mut out = Vec::new();
    for _ in 0..=rng.below(3) {
        if rng.below(8) == 0 {
            out.extend_from_slice(b"\r\n");
        }
        if kind == 1 {
            out.extend_from_slice(rng.pick(METHODS).as_bytes());
            out.extend_from_slice(rng.pick(&[" ", " ", "  ", "\t", ""]).as_bytes());
            out.extend_from_slice(rng.pick(URLS).as_bytes());
            out.extend_from_slice(rng.pick(&[" ", " ", "  ", "\r\n", "\n"]).as_bytes());
            out.extend_from_slice(rng.pick(PROTOS).as_bytes());
        } else {
            out.extend_from_slice(rng.pick(PROTOS).as_bytes());
            out.extend_from_slice(rng.pick(&[" ", " ", ""]).as_bytes());
            out.extend_from_slice(rng.pick(STATUS).as_bytes());
        }
        out.extend_from_slice(
            rng.pick(&["\r\n", "\r\n", "\r\n", "\n", "\r", " \r\n"])
                .as_bytes(),
        );
        for _ in 0..rng.below(5) {
            out.extend_from_slice(rng.pick(HEADERS).as_bytes());
            out.extend_from_slice(rng.pick(&["\r\n", "\r\n", "\r\n", "\n", "\r"]).as_bytes());
        }
        out.extend_from_slice(rng.pick(&["\r\n", "\r\n", "\n", " \r\n"]).as_bytes());
        match rng.below(5) {
            0 => out.extend_from_slice(b"hello"),
            1 => {
                for _ in 0..rng.below(3) {
                    out.extend_from_slice(
                        rng.pick(&[
                            "5\r\nhello\r\n",
                            "a;x=y\r\n0123456789\r\n",
                            "3;q=\"a\\\"b\"\r\nabc\r\n",
                            "1 \r\nx\r\n",
                            "G\r\n",
                            "3\nabc\r\n",
                            "2;\r\nab\r\n",
                            "1;a=\r\nx\r\n",
                            "ffffffffffffffffff\r\n",
                            "2\r\nabX\r\n",
                        ])
                        .as_bytes(),
                    );
                }
                out.extend_from_slice(
                    rng.pick(&["0\r\n\r\n", "0\r\nTrailer: x\r\n\r\n", "0\r\n", ""])
                        .as_bytes(),
                );
            }
            _ => {}
        }
    }
    // mutations
    for _ in 0..rng.below(3) {
        if out.is_empty() {
            break;
        }
        let i = rng.below(out.len());
        match rng.below(3) {
            0 => out[i] = (rng.next() & 0xff) as u8,
            1 => {
                out.remove(i);
            }
            _ => out.insert(i, (rng.next() & 0xff) as u8),
        }
    }
    (kind, out)
}

/// A well-formed head, then a body from an alphabet that hits the body and chunk states.
fn body_case(rng: &mut Rng) -> (u8, Vec<u8>) {
    let heads: &[(u8, &str)] = &[
        (1, "POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"),
        (2, "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"),
        (1, "POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\n"),
        (2, "HTTP/1.1 200 OK\r\n\r\n"),
        (
            2,
            "HTTP/1.0 200 OK\r\nConnection: keep-alive\r\nContent-Length: 2\r\n\r\n",
        ),
        (1, "GET / HTTP/1.1\r\nConnection: close\r\n\r\n"),
        (1, "PRI * HTTP/2.0"),
        (1, "CONNECT h:1 HTTP/1.1\r\n\r\n"),
        (
            1,
            "GET / HTTP/1.1\r\nConnection: upgrade\r\nUpgrade: ws\r\n\r\n",
        ),
        (
            2,
            "HTTP/1.1 101 S\r\nConnection: upgrade\r\nUpgrade: ws\r\n\r\n",
        ),
        (2, "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n"),
    ];
    let (kind, head) = heads[rng.below(heads.len())];
    let mut out = head.as_bytes().to_vec();
    let alphabet = b"0123456789aAfFgG;=\"\\ \t\r\n\r\n\r\nxyz,\x7f\x80\x00SMPRI";
    let pieces: &[&str] = &[
        "\r\n\r\nSM\r\n\r\n",
        "5\r\nhello\r\n",
        "0\r\n\r\n",
        "3;a=\"b\"\r\nabc\r\n",
        "GET / HTTP/1.1\r\n\r\n",
        "1;a=\"",
        "1;a=b",
        "\\\x01",
        "1;a=\"\\",
    ];
    for _ in 0..rng.below(30) {
        if rng.below(4) == 0 {
            out.extend_from_slice(pieces[rng.below(pieces.len())].as_bytes());
        } else {
            out.push(alphabet[rng.below(alphabet.len())]);
        }
    }
    (kind, out)
}

/// One side's whole run: per call, the result, position, reason and fields.
fn run_c(kind: u8, input: &[u8], cuts: &[usize], script: &[i32]) -> (Vec<String>, Vec<String>) {
    let mut log = Log {
        script: script.to_vec(),
        ..Log::default()
    };
    let mut p: CParser = unsafe { std::mem::zeroed() };
    unsafe { llhttp_init(&raw mut p, c_int::from(kind), &raw const SETTINGS) };
    p.data = (&raw mut log).cast();
    let mut out = Vec::new();
    let mut start = 0;
    let mut chunks: Vec<&[u8]> = Vec::new();
    for &cut in cuts.iter().chain([&input.len()]) {
        chunks.push(&input[start..cut]);
        start = cut;
    }
    'outer: for chunk in chunks {
        let mut data = chunk;
        for _ in 0..8 {
            let r = unsafe { llhttp_execute(&raw mut p, data.as_ptr().cast(), data.len()) };
            let pos = if r == 0 {
                data.len()
            } else {
                unsafe {
                    p.error_pos
                        .offset_from(data.as_ptr().cast())
                        .cast_unsigned()
                }
            };
            let reason = if r != 0 && !p.reason.is_null() {
                unsafe { CStr::from_ptr(p.reason) }
                    .to_string_lossy()
                    .into_owned()
            } else {
                String::new()
            };
            out.push(format!(
                "r={r} pos={pos} reason={reason:?} m={} v={}.{} f={:#x} s={} u={} cl={} fin={} ka={} eof={}",
                p.method, p.http_major, p.http_minor, p.flags, p.status_code, p.upgrade, p.content_length, p.finish,
                unsafe { llhttp_should_keep_alive(&raw const p) }, unsafe { llhttp_message_needs_eof(&raw const p) }
            ));
            match r {
                0 => break,
                21 => unsafe { llhttp_resume(&raw mut p) },
                22 => unsafe { llhttp_resume_after_upgrade(&raw mut p) },
                _ => break 'outer,
            }
            data = &data[pos..];
        }
    }
    let f = unsafe { llhttp_finish(&raw mut p) };
    out.push(format!("finish={f}"));
    (out, log.events)
}

fn run_r(kind: u8, input: &[u8], cuts: &[usize], script: &[i32]) -> (Vec<String>, Vec<String>) {
    let mut r = R {
        parser: RParser::new(if kind == 1 {
            llhttp::Kind::Request
        } else {
            llhttp::Kind::Response
        }),
        log: Log {
            script: script.to_vec(),
            ..Log::default()
        },
    };
    let mut out = Vec::new();
    let mut start = 0;
    let mut chunks: Vec<&[u8]> = Vec::new();
    for &cut in cuts.iter().chain([&input.len()]) {
        chunks.push(&input[start..cut]);
        start = cut;
    }
    'outer: for chunk in chunks {
        let mut data = chunk;
        for _ in 0..8 {
            let res = llhttp::execute(&mut r, data);
            let p = &r.parser;
            let pos = if res == 0 { data.len() } else { p.error_pos() };
            let reason = if res != 0 {
                p.reason().to_owned()
            } else {
                String::new()
            };
            out.push(format!(
                "r={res} pos={pos} reason={reason:?} m={} v={}.{} f={:#x} s={} u={} cl={} fin={} ka={} eof={}",
                p.method, p.http_major, p.http_minor, p.flags, p.status_code, u8::from(p.upgrade), p.content_length,
                match p.finish {
                    llhttp::Finish::Safe => 0,
                    llhttp::Finish::SafeWithCb => 1,
                    llhttp::Finish::Unsafe => 2,
                },
                i32::from(p.should_keep_alive()), i32::from(p.needs_eof())
            ));
            match res {
                0 => break,
                21 => r.parser.resume(),
                22 => r.parser.resume_after_upgrade(),
                _ => break 'outer,
            }
            data = &data[pos..];
        }
    }
    let f = llhttp::finish(&mut r);
    out.push(format!("finish={f}"));
    (out, r.log.events)
}

#[test]
fn differential() {
    let seed: u64 = std::env::var("SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let cases: usize = std::env::var("CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20_000);
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    for case in 0..cases {
        let (kind, input) = generate(&mut rng);
        let mut cuts: Vec<usize> = (0..rng.below(4))
            .map(|_| rng.below(input.len() + 1))
            .collect();
        cuts.sort_unstable();
        let script: Vec<i32> = (0..40)
            .map(|_| match rng.below(40) {
                0 => 21,
                1 => -1,
                2 => 1,
                3 => 2,
                4 => 7,
                _ => 0,
            })
            .collect();
        let c = run_c(kind, &input, &cuts, &script);
        let r = run_r(kind, &input, &cuts, &script);
        assert!(
            c == r,
            "case {case} kind {kind} cuts {cuts:?} input {:?}\nC: {c:#?}\nR: {r:#?}",
            String::from_utf8_lossy(&input)
        );
    }
}
