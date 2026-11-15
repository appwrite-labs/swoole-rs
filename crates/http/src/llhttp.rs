//! The HTTP/1 parser Swoole bundled as `thirdparty/llhttp` (llhttp 9.3), in Rust.
//!
//! It parses requests and responses the way Swoole configures llhttp, with no lenient
//! flags. Every state, error code, reason and error position follows the generated
//! `llhttp.c`, so callers and PHP see the same results. A span (URL, header field,
//! header value or body) still open at the end of an input buffer is flushed to its
//! callback, as llhttp does, and the next call continues it. Only the callbacks Swoole
//! uses are hooks here; llhttp's others did nothing that a caller could observe.

/// `HPE_*` codes a caller checks.
pub const HPE_OK: i32 = 0;
pub const HPE_INVALID_HEADER_TOKEN: i32 = 10;
pub const HPE_INVALID_EOF_STATE: i32 = 14;
pub const HPE_PAUSED: i32 = 21;
pub const HPE_PAUSED_UPGRADE: i32 = 22;
pub const HPE_PAUSED_H2_UPGRADE: i32 = 23;
pub const HPE_USER: i32 = 24;

/// `llhttp_type_t`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    /// `HTTP_BOTH`, a zeroed parser's type. This port doesn't detect the type, so
    /// nothing parses with it.
    #[default]
    Both,
    Request,
    Response,
}

/// `llhttp_finish_t`: what the end of input means.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Finish {
    /// Nothing is pending.
    #[default]
    Safe,
    /// The message completes at EOF.
    SafeWithCb,
    /// EOF is an error.
    Unsafe,
}

/// `llhttp_flags_t`.
pub const F_CONNECTION_KEEP_ALIVE: u16 = 0x1;
pub const F_CONNECTION_CLOSE: u16 = 0x2;
pub const F_CONNECTION_UPGRADE: u16 = 0x4;
pub const F_CHUNKED: u16 = 0x8;
pub const F_UPGRADE: u16 = 0x10;
pub const F_CONTENT_LENGTH: u16 = 0x20;
pub const F_SKIPBODY: u16 = 0x40;
pub const F_TRAILING: u16 = 0x80;
pub const F_TRANSFER_ENCODING: u16 = 0x200;

/// `HTTP_HEAD`, `HTTP_CONNECT` and `HTTP_PRI`.
pub const HEAD: u8 = 2;
pub const CONNECT: u8 = 5;
pub const PRI: u8 = 34;

/// `llhttp_method_t`'s names, by number.
const METHODS: [&str; 47] = [
    "DELETE",
    "GET",
    "HEAD",
    "POST",
    "PUT",
    "CONNECT",
    "OPTIONS",
    "TRACE",
    "COPY",
    "LOCK",
    "MKCOL",
    "MOVE",
    "PROPFIND",
    "PROPPATCH",
    "SEARCH",
    "UNLOCK",
    "BIND",
    "REBIND",
    "UNBIND",
    "ACL",
    "REPORT",
    "MKACTIVITY",
    "CHECKOUT",
    "MERGE",
    "M-SEARCH",
    "NOTIFY",
    "SUBSCRIBE",
    "UNSUBSCRIBE",
    "PATCH",
    "PURGE",
    "MKCALENDAR",
    "LINK",
    "UNLINK",
    "SOURCE",
    "PRI",
    "DESCRIBE",
    "ANNOUNCE",
    "SETUP",
    "PLAY",
    "PAUSE",
    "TEARDOWN",
    "GET_PARAMETER",
    "SET_PARAMETER",
    "REDIRECT",
    "RECORD",
    "FLUSH",
    "QUERY",
];

/// `llhttp_method_name`: empty for a number llhttp has no method for.
#[must_use]
pub fn method_name(method: u8) -> &'static str {
    METHODS
        .get(usize::from(method))
        .copied()
        .unwrap_or_default()
}

/// The protocols a request or status line may name.
const PROTOCOLS: [&str; 3] = ["HTTP", "ICE", "RTSP"];

/// The header names llhttp tracks (lower case), and the header state each sets.
const HEADERS: [&str; 5] = [
    "connection",
    "content-length",
    "proxy-connection",
    "transfer-encoding",
    "upgrade",
];
const HEADER_STATES: [u8; 5] = [1, 2, 1, 3, 4];

/// `Connection` tokens, and the header state each sets.
const CONNECTION: [&str; 3] = ["close", "keep-alive", "upgrade"];
const CONNECTION_STATES: [u8; 3] = [6, 5, 7];

/// The rest of the HTTP/2 preface after `PRI * HTTP/2.0`.
const PREFACE: [&str; 1] = ["\r\n\r\nSM\r\n\r\n"];
const CHUNKED: [&str; 1] = ["chunked"];

/// By byte, the words (as bits) that start with it.
#[expect(
    clippy::indexing_slicing,
    reason = "a byte indexes 256 entries; slice::get_mut isn't const yet"
)]
const fn starts(words: &[&str]) -> [u64; 256] {
    let mut starts = [0; 256];
    let (mut rest, mut bit) = (words, 1);
    while let [word, tail @ ..] = rest {
        if let Some(&first) = word.as_bytes().first() {
            starts[first as usize] |= bit;
        }
        (rest, bit) = (tail, bit << 1);
    }
    starts
}

const METHOD_STARTS: [u64; 256] = starts(&METHODS);
const PROTOCOL_STARTS: [u64; 256] = starts(&PROTOCOLS);
const HEADER_STARTS: [u64; 256] = starts(&HEADERS);
const CONNECTION_STARTS: [u64; 256] = starts(&CONNECTION);
const CHUNKED_STARTS: [u64; 256] = starts(&CHUNKED);
const PREFACE_STARTS: [u64; 256] = starts(&PREFACE);

/// The callbacks: each returns 0 to go on, `HPE_PAUSED` to pause, or an error.
pub trait Callbacks {
    /// The parser these callbacks drive, which they may read and change.
    fn parser(&mut self) -> &mut Parser;
    fn on_url(&mut self, _data: &[u8]) -> i32 {
        0
    }
    fn on_header_field(&mut self, _data: &[u8]) -> i32 {
        0
    }
    fn on_header_value(&mut self, _data: &[u8]) -> i32 {
        0
    }
    /// 1 skips the body, 2 also treats the message as an upgrade.
    fn on_headers_complete(&mut self) -> i32 {
        0
    }
    fn on_body(&mut self, _data: &[u8]) -> i32 {
        0
    }
    fn on_message_complete(&mut self) -> i32 {
        0
    }
}

/// Where the parser is. The states that read no byte run between the others.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
enum State {
    #[default]
    Start,
    Method,
    ReqFirstSpace,
    ReqSpaces,
    UrlEntryConnect,
    UrlStart,
    UrlSchema,
    UrlSchemaDelim,
    UrlSchemaDelim1,
    UrlServer,
    UrlServerWithAt,
    UrlPath,
    UrlQueryOrFragment,
    UrlQuery,
    UrlFragment,
    UrlSkipToHttp09,
    UrlSkipLfToHttp09,
    UrlSkipLfToHttp09Lf,
    UrlToHttp09,
    UrlSkipToHttp,
    UrlToHttp,
    ReqHttpStart,
    Protocol,
    AfterProtocol,
    HttpMajor,
    HttpDot,
    HttpMinor,
    ReqHttpComplete,
    ReqHttpCompleteCrlf,
    PriUpgrade,
    ResAfterVersion,
    ResStatusCode,
    ResStatusOtherwise,
    ResStatus,
    ResLineAlmostDone,
    HeadersStart,
    HeaderFieldStart,
    HeaderField,
    HeaderFieldColon,
    HeaderFieldGeneral,
    FieldComplete,
    DiscardWs,
    DiscardWsAlmostDone,
    DiscardLws,
    HeaderValue,
    ValueOtherwise,
    ValueAlmostDone,
    ValueLws,
    Connection,
    ConnectionMatch,
    ConnectionToken,
    ConnectionWs,
    ContentLength,
    ContentLengthWs,
    TeChunked,
    TeChunkedLast,
    TeToken,
    TeTokenOws,
    HeadersAlmostDone,
    AfterHeadersComplete,
    Body,
    Eof,
    MessageComplete,
    UpgradeCheck,
    PauseUpgrade,
    AfterMessageComplete,
    Closed,
    ChunkSizeDigit,
    ChunkSize,
    ChunkSizeOtherwise,
    ChunkSizeAlmostDone,
    ChunkExtensions,
    ChunkExtensionName,
    ChunkExtensionValue,
    ChunkQuotedValue,
    ChunkQuotedPair,
    ChunkQuotedDone,
    ChunkData,
    ChunkDataAlmostDone,
    ChunkDataAlmostDoneLf,
    /// An error a paused span callback deferred.
    Fail,
    Dead,
}

impl State {
    /// Whether the state goes on without input: the steps between bytes.
    fn takes_no_byte(self) -> bool {
        matches!(
            self,
            Self::FieldComplete
                | Self::AfterHeadersComplete
                | Self::MessageComplete
                | Self::UpgradeCheck
                | Self::PauseUpgrade
                | Self::AfterMessageComplete
                | Self::Fail
                | Self::Dead
        )
    }
}

/// The span being collected for a callback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
enum Span {
    #[default]
    None,
    Url,
    Field,
    Value,
    Body,
}

/// `llhttp_t`. All zeroes is a valid parser of no type, as `llhttp_t parser = {}`.
#[derive(Debug, Default)]
pub struct Parser {
    pub kind: Kind,
    pub method: u8,
    pub http_major: u8,
    pub http_minor: u8,
    pub upgrade: bool,
    pub finish: Finish,
    pub flags: u16,
    pub status_code: u16,
    pub content_length: u64,
    /// The `HPE_*` code, 0 while parsing goes on.
    pub error: i32,
    reason: Option<&'static &'static str>,
    error_pos: usize,
    header_state: u8,
    state: State,
    /// Bytes of the word being matched, and which word (the one that ends here,
    /// if any, else the first that goes on).
    index: u8,
    word: u8,
    /// The words that still match, by bit.
    candidates: u64,
    span: Span,
    span_start: usize,
    pending: (i32, Option<&'static &'static str>),
}

impl Parser {
    /// `llhttp_init`: a fresh parser of `kind`.
    #[must_use]
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            ..Self::default()
        }
    }

    /// `llhttp_reset`: a fresh parser of the same kind.
    pub fn reset(&mut self) {
        *self = Self::new(self.kind);
    }

    /// `llhttp_get_error_reason`.
    #[must_use]
    pub fn reason(&self) -> &'static str {
        self.reason.copied().unwrap_or_default()
    }

    /// `llhttp_set_error_reason`.
    pub fn set_reason(&mut self, reason: &'static &'static str) {
        self.reason = Some(reason);
    }

    /// `llhttp_get_error_pos`: the offset in the last input where parsing stopped.
    #[must_use]
    pub fn error_pos(&self) -> usize {
        self.error_pos
    }

    /// `llhttp_resume`: go on after `HPE_PAUSED`.
    pub fn resume(&mut self) {
        if self.error == HPE_PAUSED {
            self.error = 0;
        }
    }

    /// `llhttp_resume_after_upgrade`: go on after `HPE_PAUSED_UPGRADE`.
    pub fn resume_after_upgrade(&mut self) {
        if self.error == HPE_PAUSED_UPGRADE {
            self.error = 0;
        }
    }

    /// `llhttp_message_needs_eof`.
    #[must_use]
    pub fn needs_eof(&self) -> bool {
        if self.kind == Kind::Request {
            return false;
        }
        if self.status_code / 100 == 1
            || self.status_code == 204
            || self.status_code == 304
            || self.flags & F_SKIPBODY != 0
        {
            return false;
        }
        if self.flags & F_TRANSFER_ENCODING != 0 && self.flags & F_CHUNKED == 0 {
            return true;
        }
        self.flags & (F_CHUNKED | F_CONTENT_LENGTH) == 0
    }

    /// `llhttp_should_keep_alive`.
    #[must_use]
    pub fn should_keep_alive(&self) -> bool {
        if self.http_major > 0 && self.http_minor > 0 {
            if self.flags & F_CONNECTION_CLOSE != 0 {
                return false;
            }
        } else if self.flags & F_CONNECTION_KEEP_ALIVE == 0 {
            return false;
        }
        !self.needs_eof()
    }

    /// `llhttp__before_headers_complete`.
    fn before_headers_complete(&mut self) {
        self.upgrade = if self.flags & F_UPGRADE != 0 && self.flags & F_CONNECTION_UPGRADE != 0 {
            self.kind == Kind::Request || self.status_code == 101
        } else {
            self.method == CONNECT
        };
    }

    /// `llhttp__after_headers_complete`: what follows the head.
    fn after_headers_complete(&self) -> u8 {
        let has_body = self.flags & F_CHUNKED != 0 || self.content_length > 0;
        let response = self.kind == Kind::Response;
        if (self.upgrade && (self.method == CONNECT || self.flags & F_SKIPBODY != 0 || !has_body))
            || (response && self.status_code == 101)
        {
            return 1;
        }
        if response && self.status_code == 100 {
            return 0;
        }
        if self.flags & F_SKIPBODY != 0
            || (response && matches!(self.status_code, 102 | 103 | 204 | 304))
        {
            0
        } else if self.flags & F_CHUNKED != 0 {
            2
        } else if self.flags & F_TRANSFER_ENCODING != 0 {
            if self.kind == Kind::Request { 5 } else { 4 }
        } else if self.flags & F_CONTENT_LENGTH == 0 {
            if self.needs_eof() { 4 } else { 0 }
        } else if self.content_length == 0 {
            0
        } else {
            3
        }
    }

    /// Apply a finished header's `Connection` or `Transfer-Encoding` state.
    fn header_flags(&mut self) {
        match self.header_state {
            5 => self.flags |= F_CONNECTION_KEEP_ALIVE,
            6 => self.flags |= F_CONNECTION_CLOSE,
            7 => self.flags |= F_CONNECTION_UPGRADE,
            8 => {
                self.flags |= F_CHUNKED;
                return;
            }
            _ => return,
        }
        self.header_state = 1;
    }
}

/// `llhttp_execute`: parse `data`, returning `HPE_OK` or the error that stopped it.
pub fn execute(cb: &mut (impl Callbacks + ?Sized), data: &[u8]) -> i32 {
    let parser = cb.parser();
    if parser.error != 0 {
        return parser.error;
    }
    if parser.span != Span::None {
        parser.span_start = 0;
    }
    let mut run = Run { cb, data, p: 0 };
    if let Err(error) = run.run() {
        return error;
    }
    let parser = run.cb.parser();
    let span = parser.span;
    let start = parser.span_start;
    if span != Span::None {
        let error = run.call(span, start, data.len());
        if error != 0 {
            let parser = run.cb.parser();
            parser.error = error;
            parser.error_pos = data.len();
            return error;
        }
    }
    0
}

/// `llhttp_finish`: the end of input.
pub fn finish(cb: &mut (impl Callbacks + ?Sized)) -> i32 {
    let parser = cb.parser();
    if parser.error != 0 {
        return 0;
    }
    match parser.finish {
        Finish::Safe => 0,
        Finish::SafeWithCb => cb.on_message_complete(),
        Finish::Unsafe => {
            parser.reason = Some(&"Invalid EOF state");
            HPE_INVALID_EOF_STATE
        }
    }
}

/// One call's input and position.
struct Run<'a, C: ?Sized> {
    cb: &'a mut C,
    data: &'a [u8],
    p: usize,
}

type Next = Result<State, i32>;

/// A byte matched against a set of words.
enum Word {
    /// No word goes on with this byte.
    Mismatch,
    /// A longer word goes on with it.
    Partial,
    /// It ends this word, which no other extends.
    Whole(u8),
}

/// Match `c` against `words` (at most 64, with `starts` their first bytes), after
/// the parser's `index` bytes: only the words still in `candidates` can go on, so no
/// prefix is compared again.
fn next_word(words: &[&str], starts: &[u64; 256], parser: &mut Parser, c: u8) -> Word {
    let index = usize::from(parser.index);
    let mut candidates = if index == 0 {
        starts.get(usize::from(c)).copied().unwrap_or_default()
    } else {
        parser.candidates
    };
    let (mut whole, mut longer, mut next) = (None, None, 0);
    while candidates != 0 {
        let i = candidates.trailing_zeros() as usize;
        candidates &= candidates - 1;
        let Some(word) = words.get(i).map(|word| word.as_bytes()) else {
            break;
        };
        if word.get(index) == Some(&c) {
            if word.len() == index + 1 {
                whole = Some(i);
            } else {
                longer.get_or_insert(i);
                next |= 1 << i;
            }
        }
    }
    let byte = |i: usize| u8::try_from(i).unwrap_or_default();
    match (whole, longer) {
        (Some(word), None) => {
            parser.index = 0;
            Word::Whole(byte(word))
        }
        (whole, Some(longer)) => {
            parser.word = byte(whole.unwrap_or(longer));
            parser.candidates = next;
            parser.index += 1;
            Word::Partial
        }
        (None, None) => Word::Mismatch,
    }
}

/// Which of `words` `name` is, if any (ignoring case if `fold`, for words of
/// lower-case letters and `-` and a name without CR): what the byte-by-byte match
/// finds for a word that a byte no word takes follows.
fn whole_word(words: &[&str], starts: &[u64; 256], name: &[u8], fold: bool) -> Option<u8> {
    let first = if fold {
        lower(*name.first()?)
    } else {
        *name.first()?
    };
    let mut candidates = starts.get(usize::from(first)).copied().unwrap_or_default();
    while candidates != 0 {
        let i = candidates.trailing_zeros() as usize;
        candidates &= candidates - 1;
        let word = words.get(i)?.as_bytes();
        // `n | 0x20` is a lower-case letter only from that letter or its capital, and `-` only from `-` or CR.
        let same = if fold {
            word.len() == name.len() && word.iter().zip(name).all(|(&w, &n)| w == n | 0x20)
        } else {
            word == name
        };
        if same {
            return u8::try_from(i).ok();
        }
    }
    None
}

fn url_ws(c: u8) -> bool {
    matches!(c, 9 | 12)
}

fn path_char(c: u8) -> bool {
    is(PATH, c)
}

fn graphic(c: u8) -> bool {
    matches!(c, 0x21..=0x7e)
}

/// A query goes on to a fragment at `#`.
fn query_char(c: u8) -> bool {
    graphic(c) && c != b'#'
}

fn token(c: u8) -> bool {
    is(TOKEN, c)
}

const TOKEN: u8 = 1;
const VALUE: u8 = 2;
const PATH: u8 = 4;

/// Whether `c` is in `class`: one lookup where the per-byte loops would test ranges.
fn is(class: u8, c: u8) -> bool {
    CLASSES.get(usize::from(c)).copied().unwrap_or_default() & class != 0
}

/// By byte, its classes.
#[expect(
    clippy::indexing_slicing,
    reason = "256 entries by byte; slice::get_mut isn't const yet"
)]
const CLASSES: [u8; 256] = {
    let mut classes = [0; 256];
    let mut c = 0;
    while c < 256 {
        if matches!(c, 0x21 | 0x23..=0x27 | 0x2a | 0x2b | 0x2d | 0x2e | 0x30..=0x39 | 0x41..=0x5a | 0x5e..=0x7a | 0x7c | 0x7e)
        {
            classes[c] |= TOKEN;
        }
        if c == 9 || c >= 0x20 && c != 0x7f {
            classes[c] |= VALUE;
        }
        if matches!(c, 0x21 | 0x22 | 0x24..=0x3e | 0x40..=0x7e) {
            classes[c] |= PATH;
        }
        c += 1;
    }
    classes
};

fn value_char(c: u8) -> bool {
    is(VALUE, c)
}

fn list_char(c: u8) -> bool {
    value_char(c) && c != b','
}

fn server_char(c: u8) -> bool {
    matches!(c, 0x21 | 0x24..=0x2e | 0x30..=0x3b | 0x3d | 0x41..=0x5b | 0x5d | 0x5f | 0x61..=0x7a | 0x7e)
}

fn lower(c: u8) -> u8 {
    c.to_ascii_lowercase()
}

fn hex(c: u8) -> Option<u8> {
    char::from(c)
        .to_digit(16)
        .and_then(|digit| u8::try_from(digit).ok())
}

fn digit(c: u8) -> Option<u8> {
    c.is_ascii_digit().then(|| c - b'0')
}

impl<C: Callbacks + ?Sized> Run<'_, C> {
    fn parser(&mut self) -> &mut Parser {
        self.cb.parser()
    }

    /// A span's callback, with `SPAN_CALLBACK_MAYBE`'s -1 as `HPE_USER`.
    fn call(&mut self, span: Span, start: usize, end: usize) -> i32 {
        let data = self.data.get(start..end).unwrap_or_default();
        let (error, reason) = match span {
            Span::None => return 0,
            Span::Url => (self.cb.on_url(data), &"Span callback error in on_url"),
            Span::Field => (
                self.cb.on_header_field(data),
                &"Span callback error in on_header_field",
            ),
            Span::Value => (
                self.cb.on_header_value(data),
                &"Span callback error in on_header_value",
            ),
            Span::Body => (self.cb.on_body(data), &"Span callback error in on_body"),
        };
        if error == -1 {
            self.parser().reason = Some(reason);
            return HPE_USER;
        }
        error
    }

    /// Consume the byte and go to `next`.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "a state's step, which other steps' errors share"
    )]
    fn eat(&mut self, next: State) -> Next {
        self.p += 1;
        Ok(next)
    }

    /// Consume this byte and the run after it that `accept` takes, staying in
    /// `state`: llhttp's per-byte loop for a state that only eats, in one scan.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "a state's step, which other steps' errors share"
    )]
    fn skip(&mut self, state: State, accept: impl Fn(u8) -> bool) -> Next {
        let rest = self.data.get(self.p + 1..).unwrap_or_default();
        self.p += 1 + rest.iter().position(|&c| !accept(c)).unwrap_or(rest.len());
        Ok(state)
    }

    /// Go on to `next`, a state that reads a byte, with that byte here when the
    /// input has it: the run loop's step without its dispatch, which costs more
    /// than most states' work on the usual request.
    fn go(&mut self, next: State, step: impl FnOnce(&mut Self, u8) -> Next) -> Next {
        match self.data.get(self.p) {
            Some(&c) => step(self, c),
            None => Ok(next),
        }
    }

    /// Stop with `error`, resuming at `resume` if it is a pause.
    fn stop(&mut self, error: i32, reason: &'static &'static str, resume: State) -> Next {
        let p = self.p;
        let parser = self.parser();
        parser.error = error;
        parser.reason = Some(reason);
        parser.error_pos = p;
        parser.state = resume;
        Err(error)
    }

    fn fail(&mut self, error: i32, reason: &'static &'static str) -> Next {
        self.stop(error, reason, State::Dead)
    }

    /// Consume the byte, then fail.
    fn eat_fail(&mut self, error: i32, reason: &'static &'static str) -> Next {
        self.p += 1;
        self.fail(error, reason)
    }

    fn open(&mut self, span: Span) {
        let p = self.p;
        let parser = self.parser();
        parser.span = span;
        parser.span_start = p;
    }

    /// A body span starts at its first byte, so an empty one is never flushed.
    fn open_body(&mut self) {
        if self.parser().span == Span::None {
            self.open(Span::Body);
        }
    }

    /// End the span at this byte, consuming it if `eat`, and go to `next`. A
    /// callback error stops there, to resume at `next`.
    fn close(&mut self, eat: bool, next: State) -> Next {
        let parser = self.parser();
        let span = parser.span;
        let start = parser.span_start;
        parser.span = Span::None;
        let error = self.call(span, start, self.p);
        if eat {
            self.p += 1;
        }
        if error != 0 {
            let p = self.p;
            let parser = self.parser();
            parser.error = error;
            parser.error_pos = p;
            parser.state = next;
            return Err(error);
        }
        Ok(next)
    }

    /// End the span, then fail with `error` (after a paused callback, on resume).
    fn close_fail(&mut self, eat: bool, error: i32, reason: &'static &'static str) -> Next {
        self.parser().pending = (error, Some(reason));
        self.close(eat, State::Fail)
    }

    /// The state lives here until the input runs out; every step that stops
    /// (`stop`, `close`) stores the state to resume at itself.
    fn run(&mut self) -> Result<(), i32> {
        let mut state = self.parser().state;
        loop {
            let c = match self.data.get(self.p) {
                Some(&c) => c,
                None if state.takes_no_byte() => 0,
                None => {
                    self.parser().state = state;
                    return Ok(());
                }
            };
            state = self.step(state, c)?;
        }
    }

    /// Consume up to `content_length` body bytes.
    fn consume(&mut self, state: State) -> Next {
        let avail = (self.data.len() - self.p) as u64;
        let parser = self.parser();
        let need = parser.content_length;
        if avail < need {
            parser.content_length = need - avail;
            self.p = self.data.len();
            return Ok(state);
        }
        parser.content_length = 0;
        self.p += usize::try_from(need).unwrap_or(usize::MAX);
        let next = if state == State::Body {
            State::MessageComplete
        } else {
            State::ChunkDataAlmostDone
        };
        self.close(false, next)
    }

    fn field_complete(&mut self) -> Next {
        let parser = self.parser();
        match parser.header_state {
            2 if parser.flags & F_TRANSFER_ENCODING != 0 => self.fail(
                11,
                &"Content-Length can't be present with Transfer-Encoding",
            ),
            3 if parser.flags & F_CONTENT_LENGTH != 0 => self.fail(
                15,
                &"Transfer-Encoding can't be present with Content-Length",
            ),
            _ => self.go(State::DiscardWs, Self::discard_ws),
        }
    }

    fn discard_ws(&mut self, c: u8) -> Next {
        match c {
            9 | b' ' => {
                self.p += 1;
                match self.data.get(self.p) {
                    Some(&c) if !matches!(c, 9 | 10 | 13 | b' ') => self.value_start(c),
                    _ => Ok(State::DiscardWs),
                }
            }
            10 => self.eat_fail(10, &"Invalid header value char"),
            13 => self.eat(State::DiscardWsAlmostDone),
            _ => self.value_start(c),
        }
    }

    /// A header line's first byte, or the head's end.
    fn field_start(&mut self, c: u8) -> Next {
        match c {
            10 => self.eat_fail(10, &"Invalid header field char"),
            13 if self.data.get(self.p + 1) == Some(&10) => {
                self.p += 1;
                self.head_end()
            }
            13 => self.eat(State::HeadersAlmostDone),
            b':' => self.fail(10, &"Invalid header token"),
            _ => {
                self.open(Span::Field);
                self.parser().index = 0;
                self.header_field(c)
            }
        }
    }

    /// The LF that ends the head.
    fn head_end(&mut self) -> Next {
        self.p += 1;
        if self.parser().flags & F_TRAILING != 0 {
            Ok(State::MessageComplete)
        } else {
            self.headers_complete()
        }
    }

    /// The head's first header line, after the start line.
    fn headers_start(&mut self, c: u8) -> Next {
        if c == b' ' {
            return self.eat_fail(30, &"Unexpected space after start line");
        }
        self.field_start(c)
    }

    /// Past the space after the method, the URL's first byte if it is here.
    fn req_space(&mut self) -> Next {
        self.p += 1;
        match self.data.get(self.p) {
            Some(&c) if c != b' ' => self.url_entry(c),
            _ => Ok(State::ReqSpaces),
        }
    }

    /// The URL's first byte: an origin-form `/` or `*` goes straight to its path.
    fn url_entry(&mut self, c: u8) -> Next {
        if self.parser().method == CONNECT {
            return Ok(State::UrlEntryConnect);
        }
        if url_ws(c) {
            return self.eat_fail(7, &"Invalid characters in url");
        }
        self.open(Span::Url);
        if matches!(c, b'*' | b'/') {
            return self.eat(State::UrlPath);
        }
        Ok(State::UrlStart)
    }

    /// The space after the URL: its span's end, then the protocol.
    fn url_end(&mut self) -> Next {
        self.close(false, State::UrlSkipToHttp)?;
        self.p += 1;
        self.go(State::UrlToHttp, Self::url_to_http)
    }

    fn url_to_http(&mut self, c: u8) -> Next {
        if url_ws(c) {
            return self.eat_fail(7, &"Invalid characters in url");
        }
        if c == b' ' {
            return self.eat(State::ReqHttpStart);
        }
        self.protocol(c)
    }

    /// The `/` after the protocol, and the version with it when `/D.D` is here.
    fn after_protocol(&mut self, c: u8) -> Next {
        if c != b'/' {
            return self.fail(8, &"Expected HTTP/, RTSP/ or ICE/");
        }
        if let Some(&[_, major, b'.', minor]) = self.data.get(self.p..self.p + 4)
            && let Some(major) = digit(major)
        {
            self.parser().http_major = major;
            self.p += 3;
            return self.http_minor(minor);
        }
        self.eat(State::HttpMajor)
    }

    /// The CR after a request's version, and the LF with it.
    fn req_http_complete(&mut self, c: u8) -> Next {
        match c {
            10 => self.eat_fail(9, &"Expected CRLF after version"),
            13 if self.data.get(self.p + 1) == Some(&10) => {
                self.p += 2;
                self.go(State::HeadersStart, Self::headers_start)
            }
            13 => self.eat(State::ReqHttpCompleteCrlf),
            _ => self.fail(9, &"Expected CRLF after version"),
        }
    }

    /// A header value's bytes, up to and past its CRLF when that is here. It
    /// doesn't go on to the next line, whose value would call it again.
    fn header_value(&mut self, c: u8) -> Next {
        if value_char(c) {
            self.skip(State::HeaderValue, value_char)?;
        }
        // The usual end: CR LF and the next line's first byte, all here, as
        // `ValueOtherwise`, `ValueAlmostDone` and `ValueLws` take them.
        match self.data.get(self.p..).unwrap_or_default() {
            [] => Ok(State::HeaderValue),
            [b'\r', b'\n', next, ..] if !matches!(next, 9 | b' ') => {
                self.close(true, State::ValueAlmostDone)?;
                self.p += 1;
                self.parser().header_flags();
                Ok(State::HeaderFieldStart)
            }
            _ => Ok(State::ValueOtherwise),
        }
    }

    fn headers_complete(&mut self) -> Next {
        self.parser().before_headers_complete();
        match self.cb.on_headers_complete() {
            0 => {}
            1 => self.parser().flags |= F_SKIPBODY,
            2 => {
                let parser = self.parser();
                parser.upgrade = true;
                parser.flags |= F_SKIPBODY;
            }
            HPE_PAUSED => {
                return self.stop(
                    HPE_PAUSED,
                    &"Paused by on_headers_complete",
                    State::AfterHeadersComplete,
                );
            }
            _ => return self.fail(17, &"User callback error"),
        }
        self.after_headers_complete()
    }

    fn after_headers_complete(&mut self) -> Next {
        match self.parser().after_headers_complete() {
            1 => match self.cb.on_message_complete() {
                0 => Ok(State::PauseUpgrade),
                HPE_PAUSED => self.stop(
                    HPE_PAUSED,
                    &"on_message_complete pause",
                    State::PauseUpgrade,
                ),
                _ => self.fail(18, &"`on_message_complete` callback error"),
            },
            2 => {
                self.parser().content_length = 0;
                Ok(State::ChunkSizeDigit)
            }
            3 => Ok(State::Body),
            4 => {
                self.parser().finish = Finish::SafeWithCb;
                Ok(State::Eof)
            }
            5 => self.fail(15, &"Request has invalid `Transfer-Encoding`"),
            _ => match self.cb.on_message_complete() {
                0 => Ok(self.after_message_complete()),
                HPE_PAUSED => self.stop(
                    HPE_PAUSED,
                    &"on_message_complete pause",
                    State::AfterMessageComplete,
                ),
                _ => self.fail(18, &"`on_message_complete` callback error"),
            },
        }
    }

    fn message_complete(&mut self) -> Next {
        match self.cb.on_message_complete() {
            0 => Ok(State::UpgradeCheck),
            HPE_PAUSED => self.stop(
                HPE_PAUSED,
                &"on_message_complete pause",
                State::UpgradeCheck,
            ),
            _ => self.fail(18, &"`on_message_complete` callback error"),
        }
    }

    fn after_message_complete(&mut self) -> State {
        let parser = self.parser();
        let keep_alive = parser.should_keep_alive();
        parser.finish = Finish::Safe;
        parser.flags = 0;
        if keep_alive {
            parser.content_length = 0;
            State::Start
        } else {
            State::Closed
        }
    }

    /// The header value's first byte, by the header it belongs to.
    fn value_start(&mut self, c: u8) -> Next {
        self.open(Span::Value);
        let parser = self.parser();
        match parser.header_state {
            1 => Ok(State::Connection),
            2 if parser.flags & F_CONTENT_LENGTH != 0 => self.fail(4, &"Duplicate Content-Length"),
            2 => Ok(State::ContentLength),
            3 => {
                if parser.flags & F_CHUNKED != 0 && parser.kind == Kind::Request {
                    return self.close_fail(true, 15, &"Invalid `Transfer-Encoding` header value");
                }
                parser.flags = (parser.flags | F_TRANSFER_ENCODING) & !F_CHUNKED;
                Ok(State::TeChunked)
            }
            4 => {
                parser.flags |= F_UPGRADE;
                parser.header_state = 0;
                self.header_value(c)
            }
            _ => self.header_value(c),
        }
    }

    /// The method's next byte, or the whole method when it is here.
    fn method(&mut self, c: u8) -> Next {
        if self.parser().index == 0 {
            let rest = self.data.get(self.p..).unwrap_or_default();
            let head = rest.get(..16).unwrap_or(rest);
            if let Some(end) = head.iter().position(|&c| c == b' ') {
                let name = head.get(..end).unwrap_or_default();
                if let Some(method) = whole_word(&METHODS, &METHOD_STARTS, name, false) {
                    self.parser().method = method;
                    self.p += end;
                    return self.req_space();
                }
            }
        }
        let parser = self.parser();
        match next_word(&METHODS, &METHOD_STARTS, parser, c) {
            Word::Whole(method) => {
                parser.method = method;
                self.eat(State::ReqFirstSpace)
            }
            Word::Partial => self.eat(State::Method),
            Word::Mismatch => {
                // `GET` ends where `GET_PARAMETER` would go on.
                let index = usize::from(parser.index);
                parser.index = 0;
                if METHODS.get(usize::from(parser.word)).map(|name| name.len()) == Some(index) {
                    parser.method = parser.word;
                    return Ok(State::ReqFirstSpace);
                }
                self.fail(6, &"Invalid method encountered")
            }
        }
    }

    /// The protocol's next byte, and on a request its method's check.
    fn protocol(&mut self, c: u8) -> Next {
        let rest = self.data.get(self.p..).unwrap_or_default();
        let head = rest.get(..5).unwrap_or(rest);
        let whole = head
            .iter()
            .position(|&c| c == b'/')
            .filter(|_| self.parser().index == 0)
            .and_then(|end| {
                let name = head.get(..end)?;
                Some((end, whole_word(&PROTOCOLS, &PROTOCOL_STARTS, name, false)?))
            });
        let protocol = if let Some((end, protocol)) = whole {
            self.p += end;
            protocol
        } else {
            let parser = self.parser();
            match next_word(&PROTOCOLS, &PROTOCOL_STARTS, parser, c) {
                Word::Whole(protocol) => {
                    self.p += 1;
                    protocol
                }
                Word::Partial => return self.eat(State::Protocol),
                Word::Mismatch => {
                    parser.index = 0;
                    return self.fail(8, &"Expected HTTP/, RTSP/ or ICE/");
                }
            }
        };
        let parser = self.parser();
        let (kind, method) = (parser.kind, parser.method);
        let valid = kind != Kind::Request
            || match protocol {
                0 => method <= PRI || method == 46,
                1 => method == 33,
                _ => matches!(method, 1 | 3 | 6 | 35..=45),
            };
        if !valid {
            return self.fail(
                8,
                match protocol {
                    0 => &"Invalid method for HTTP/x.x request",
                    1 => &"Expected SOURCE method for ICE/x.x request",
                    _ => &"Invalid method for RTSP/x.x request",
                },
            );
        }
        self.go(State::AfterProtocol, Self::after_protocol)
    }

    /// A tracked header name's next byte, or the general token rules; the whole
    /// name at once when it is here up to its colon.
    fn header_field(&mut self, c: u8) -> Next {
        if self.parser().index == 0 {
            let rest = self.data.get(self.p..).unwrap_or_default();
            let end = rest.iter().position(|&c| !token(c)).unwrap_or(rest.len());
            if rest.get(end) == Some(&b':') {
                let name = rest.get(..end).unwrap_or_default();
                let parser = self.parser();
                parser.header_state = whole_word(&HEADERS, &HEADER_STARTS, name, true)
                    .and_then(|header| HEADER_STATES.get(usize::from(header)).copied())
                    .unwrap_or_default();
                self.p += end;
                self.close(true, State::FieldComplete)?;
                return self.field_complete();
            }
        }
        let parser = self.parser();
        match next_word(&HEADERS, &HEADER_STARTS, parser, lower(c)) {
            Word::Whole(header) => {
                parser.header_state = HEADER_STATES
                    .get(usize::from(header))
                    .copied()
                    .unwrap_or_default();
                self.eat(State::HeaderFieldColon)
            }
            Word::Partial => self.eat(State::HeaderField),
            Word::Mismatch => {
                parser.index = 0;
                parser.header_state = 0;
                Ok(State::HeaderFieldGeneral)
            }
        }
    }

    /// A `Connection` token's next byte, or the whole token when its end is here.
    fn connection(&mut self, c: u8) -> Next {
        if self.parser().index == 0 {
            let rest = self.data.get(self.p..).unwrap_or_default();
            let end = rest
                .iter()
                .position(|&c| matches!(c, 10 | 13 | b' ' | b','));
            let name = end.and_then(|end| rest.get(..end)).unwrap_or_default();
            if let Some(token) = whole_word(&CONNECTION, &CONNECTION_STARTS, name, true) {
                self.parser().header_state = CONNECTION_STATES
                    .get(usize::from(token))
                    .copied()
                    .unwrap_or_default();
                self.p += name.len();
                return Ok(State::ConnectionWs);
            }
        }
        let parser = self.parser();
        match next_word(&CONNECTION, &CONNECTION_STARTS, parser, lower(c)) {
            Word::Whole(token) => {
                parser.header_state = CONNECTION_STATES
                    .get(usize::from(token))
                    .copied()
                    .unwrap_or_default();
                self.eat(State::ConnectionWs)
            }
            Word::Partial => self.eat(State::ConnectionMatch),
            Word::Mismatch => {
                parser.index = 0;
                Ok(State::ConnectionToken)
            }
        }
    }

    fn te_chunked(&mut self, c: u8) -> Next {
        let parser = self.parser();
        match next_word(&CHUNKED, &CHUNKED_STARTS, parser, lower(c)) {
            Word::Whole(_) => self.eat(State::TeChunkedLast),
            Word::Partial => self.eat(State::TeChunked),
            Word::Mismatch => {
                parser.index = 0;
                Ok(State::TeToken)
            }
        }
    }

    fn pri_upgrade(&mut self, c: u8) -> Next {
        let parser = self.parser();
        match next_word(&PREFACE, &PREFACE_STARTS, parser, c) {
            Word::Whole(_) => self.eat_fail(HPE_PAUSED_H2_UPGRADE, &"Pause on PRI/Upgrade"),
            Word::Partial => self.eat(State::PriUpgrade),
            Word::Mismatch => {
                parser.index = 0;
                self.fail(9, &"Expected HTTP/2 Connection Preface")
            }
        }
    }

    /// The minor version's digit, then the version's check.
    fn http_minor(&mut self, c: u8) -> Next {
        let Some(minor) = digit(c) else {
            return self.fail(9, &"Invalid minor version");
        };
        self.p += 1;
        let parser = self.parser();
        parser.http_minor = minor;
        let valid = matches!((parser.http_major, minor), (0, 9) | (1, 0 | 1) | (2, 0));
        if !valid {
            return self.fail(9, &"Invalid HTTP version");
        }
        if parser.kind != Kind::Request {
            Ok(State::ResAfterVersion)
        } else if parser.method == PRI {
            Ok(State::PriUpgrade)
        } else {
            self.go(State::ReqHttpComplete, Self::req_http_complete)
        }
    }

    fn mul_add(&mut self, base: u64, add: u8) -> bool {
        let parser = self.parser();
        match parser
            .content_length
            .checked_mul(base)
            .and_then(|length| length.checked_add(u64::from(add)))
        {
            Some(length) => {
                parser.content_length = length;
                true
            }
            None => false,
        }
    }

    #[expect(
        clippy::too_many_lines,
        clippy::cognitive_complexity,
        reason = "one state machine, as llhttp.c's"
    )]
    fn step(&mut self, state: State, c: u8) -> Next {
        use State as S;
        match state {
            S::Start => {
                if matches!(c, b'\r' | b'\n') {
                    return self.eat(S::Start);
                }
                let parser = self.parser();
                parser.finish = Finish::Unsafe;
                parser.index = 0;
                parser.word = 0;
                if parser.kind == Kind::Response {
                    self.protocol(c)
                } else {
                    self.method(c)
                }
            }
            S::Method => self.method(c),
            S::ReqFirstSpace => {
                if c == b' ' {
                    return self.req_space();
                }
                self.fail(6, &"Expected space after method")
            }
            S::ReqSpaces => {
                if c == b' ' {
                    return self.eat(S::ReqSpaces);
                }
                self.url_entry(c)
            }
            S::UrlEntryConnect => {
                if url_ws(c) {
                    return self.eat_fail(7, &"Invalid characters in url");
                }
                self.open(Span::Url);
                Ok(S::UrlServer)
            }
            S::UrlStart | S::UrlSchema => match c {
                9 | 10 | 12 | 13 | b' ' => self.eat_fail(7, &"Invalid characters in url"),
                b'*' | b'/' if state == S::UrlStart => self.eat(S::UrlPath),
                b':' if state == S::UrlSchema => self.eat(S::UrlSchemaDelim),
                b'A'..=b'Z' | b'a'..=b'z' if state == S::UrlStart => Ok(S::UrlSchema),
                b'A'..=b'Z' | b'a'..=b'z' => self.eat(S::UrlSchema),
                _ if state == S::UrlStart => self.fail(7, &"Unexpected start char in url"),
                _ => self.fail(7, &"Unexpected char in url schema"),
            },
            S::UrlSchemaDelim | S::UrlSchemaDelim1 => match c {
                9 | 10 | 12 | 13 | b' ' if state == S::UrlSchemaDelim => {
                    self.eat_fail(7, &"Invalid characters in url")
                }
                b'/' if state == S::UrlSchemaDelim => self.eat(S::UrlSchemaDelim1),
                b'/' => self.eat(S::UrlServer),
                _ => self.fail(7, &"Unexpected char in url schema"),
            },
            S::UrlServer | S::UrlServerWithAt => match c {
                9 | 12 => self.eat_fail(7, &"Invalid characters in url"),
                10 => self.close(false, S::UrlSkipToHttp09),
                13 => self.close(false, S::UrlSkipLfToHttp09),
                b' ' => self.close(false, S::UrlSkipToHttp),
                b'/' => self.eat(S::UrlPath),
                b'?' => self.eat(S::UrlQuery),
                b'@' if state == S::UrlServer => self.eat(S::UrlServerWithAt),
                b'@' => self.eat_fail(7, &"Double @ in url"),
                _ if server_char(c) => self.eat(S::UrlServer),
                _ => self.fail(7, &"Unexpected char in url server"),
            },
            S::UrlPath => match c {
                9 | 12 => self.eat_fail(7, &"Invalid characters in url"),
                _ if path_char(c) => {
                    self.skip(S::UrlPath, path_char)?;
                    if self.data.get(self.p) == Some(&b' ') {
                        return self.url_end();
                    }
                    Ok(S::UrlPath)
                }
                b' ' => self.url_end(),
                _ => Ok(S::UrlQueryOrFragment),
            },
            S::UrlQueryOrFragment | S::UrlQuery | S::UrlFragment => match c {
                9 | 12 => self.eat_fail(7, &"Invalid characters in url"),
                10 => self.close(false, S::UrlSkipToHttp09),
                13 => self.close(false, S::UrlSkipLfToHttp09),
                b' ' => self.url_end(),
                b'#' if state != S::UrlFragment => self.eat(S::UrlFragment),
                b'?' if state == S::UrlQueryOrFragment => self.eat(S::UrlQuery),
                0x21..=0x7e if state == S::UrlQuery => self.skip(S::UrlQuery, query_char),
                0x21..=0x7e if state == S::UrlFragment => self.skip(S::UrlFragment, graphic),
                _ if state == S::UrlQuery => self.fail(7, &"Invalid char in url query"),
                _ if state == S::UrlFragment => self.fail(7, &"Invalid char in url fragment start"),
                _ => self.fail(7, &"Invalid char in url path"),
            },
            S::UrlSkipToHttp09 | S::UrlSkipToHttp => {
                if url_ws(c) {
                    return self.eat_fail(7, &"Invalid characters in url");
                }
                self.eat(if state == S::UrlSkipToHttp {
                    S::UrlToHttp
                } else {
                    S::UrlToHttp09
                })
            }
            S::UrlSkipLfToHttp09 => match c {
                9 | 12 => self.eat_fail(7, &"Invalid characters in url"),
                13 => self.eat(S::UrlSkipLfToHttp09Lf),
                _ => self.fail(7, &"Expected CRLF"),
            },
            S::UrlSkipLfToHttp09Lf => {
                if c == 10 {
                    return self.eat(S::UrlToHttp09);
                }
                self.fail(7, &"Expected CRLF")
            }
            S::UrlToHttp09 => {
                if url_ws(c) {
                    return self.eat_fail(7, &"Invalid characters in url");
                }
                let parser = self.parser();
                parser.http_major = 0;
                parser.http_minor = 9;
                Ok(S::HeadersStart)
            }
            S::UrlToHttp => self.url_to_http(c),
            S::ReqHttpStart => {
                if c == b' ' {
                    return self.eat(S::ReqHttpStart);
                }
                Ok(S::Protocol)
            }
            S::Protocol => self.protocol(c),
            S::AfterProtocol => self.after_protocol(c),
            S::HttpMajor => {
                let Some(major) = digit(c) else {
                    return self.fail(9, &"Invalid major version");
                };
                self.parser().http_major = major;
                self.eat(S::HttpDot)
            }
            S::HttpDot => {
                if c == b'.' {
                    return self.eat(S::HttpMinor);
                }
                self.fail(9, &"Expected dot")
            }
            S::HttpMinor => self.http_minor(c),
            S::ReqHttpComplete => self.req_http_complete(c),
            S::ReqHttpCompleteCrlf => {
                if c == 10 {
                    self.p += 1;
                    return self.go(S::HeadersStart, Self::headers_start);
                }
                self.fail(2, &"Expected CRLF after version")
            }
            S::PriUpgrade => self.pri_upgrade(c),
            S::ResAfterVersion => {
                if c == b' ' {
                    let parser = self.parser();
                    parser.status_code = 0;
                    parser.index = 0;
                    return self.eat(S::ResStatusCode);
                }
                self.fail(9, &"Expected space after version")
            }
            S::ResStatusCode => {
                let Some(d) = digit(c) else {
                    self.parser().index = 0;
                    return self.fail(13, &"Invalid status code");
                };
                let parser = self.parser();
                parser.status_code = parser.status_code * 10 + u16::from(d);
                parser.index += 1;
                if parser.index < 3 {
                    return self.eat(S::ResStatusCode);
                }
                parser.index = 0;
                self.eat(S::ResStatusOtherwise)
            }
            S::ResStatusOtherwise => match c {
                10 => self.eat_fail(13, &"Invalid response status"),
                13 => self.eat(S::ResLineAlmostDone),
                b' ' => self.eat(S::ResStatus),
                _ => self.fail(13, &"Invalid response status"),
            },
            S::ResStatus => match c {
                10 => self.eat_fail(25, &"Missing expected CR after response line"),
                13 => self.eat(S::ResLineAlmostDone),
                _ => self.eat(S::ResStatus),
            },
            S::ResLineAlmostDone => {
                if matches!(c, 10 | 13) {
                    return self.eat(S::HeadersStart);
                }
                self.fail(2, &"Expected LF after CR")
            }
            S::HeadersStart => self.headers_start(c),
            S::HeaderFieldStart => self.field_start(c),
            S::HeaderField => self.header_field(c),
            S::HeaderFieldColon => match c {
                b' ' => self.close_fail(true, 10, &"Invalid header field char"),
                b':' => self.close(true, S::FieldComplete),
                _ => {
                    self.parser().header_state = 0;
                    Ok(S::HeaderFieldGeneral)
                }
            },
            S::HeaderFieldGeneral => match c {
                _ if token(c) => self.skip(S::HeaderFieldGeneral, token),
                b':' => self.close(true, S::FieldComplete),
                _ => self.fail(10, &"Invalid header token"),
            },
            S::DiscardWs => self.discard_ws(c),
            S::DiscardWsAlmostDone => {
                if c == 10 {
                    return self.eat(S::DiscardLws);
                }
                self.fail(2, &"Expected LF after CR")
            }
            S::DiscardLws => {
                if matches!(c, 9 | b' ') {
                    return self.eat_fail(10, &"Invalid header value char");
                }
                if self.parser().header_state == 2 {
                    return self.fail(11, &"Empty Content-Length");
                }
                self.parser().header_flags();
                self.open(Span::Value);
                self.close(false, S::HeaderFieldStart)
            }
            S::HeaderValue => self.header_value(c),
            S::ValueOtherwise => match c {
                10 => self.close_fail(false, 25, &"Missing expected CR after header value"),
                13 => self.close(true, S::ValueAlmostDone),
                _ => self.close_fail(false, 10, &"Invalid header value char"),
            },
            S::ValueAlmostDone => {
                if c == 10 {
                    return self.eat(S::ValueLws);
                }
                self.fail(3, &"Missing expected LF after header value")
            }
            S::ValueLws => {
                if matches!(c, 9 | b' ') {
                    return self.fail(10, &"Unexpected whitespace after header value");
                }
                self.parser().header_flags();
                self.field_start(c)
            }
            S::Connection => match c {
                9 | b' ' => self.eat(S::Connection),
                _ => self.connection(c),
            },
            S::ConnectionMatch => self.connection(c),
            S::ConnectionToken => match c {
                b',' => self.eat(S::Connection),
                _ if list_char(c) => self.eat(S::ConnectionToken),
                _ => Ok(S::ValueOtherwise),
            },
            S::ConnectionWs => match c {
                10 | 13 => Ok(S::ValueOtherwise),
                b' ' => self.eat(S::ConnectionWs),
                b',' => {
                    self.parser().header_flags();
                    self.eat(S::Connection)
                }
                _ => {
                    self.parser().header_state = 0;
                    Ok(S::ConnectionToken)
                }
            },
            S::ContentLength => {
                let Some(d) = digit(c) else {
                    return Ok(S::ContentLengthWs);
                };
                self.p += 1;
                if self.mul_add(10, d) {
                    return Ok(S::ContentLength);
                }
                self.close_fail(false, 11, &"Content-Length overflow")
            }
            S::ContentLengthWs => match c {
                10 | 13 => {
                    self.parser().flags |= F_CONTENT_LENGTH;
                    Ok(S::ValueOtherwise)
                }
                b' ' => self.eat(S::ContentLengthWs),
                _ => self.close_fail(false, 11, &"Invalid character in Content-Length"),
            },
            S::TeChunked => self.te_chunked(c),
            S::TeChunkedLast => match c {
                10 | 13 => {
                    self.parser().header_state = 8;
                    Ok(S::ValueOtherwise)
                }
                b' ' => self.eat(S::TeChunkedLast),
                b',' if self.parser().kind == Kind::Request => {
                    self.close_fail(true, 15, &"Invalid `Transfer-Encoding` header value")
                }
                b',' => Ok(S::TeChunked),
                _ => Ok(S::TeToken),
            },
            S::TeToken => match c {
                b',' => self.eat(S::TeTokenOws),
                _ if list_char(c) => self.eat(S::TeToken),
                _ => {
                    self.parser().header_state = 0;
                    Ok(S::HeaderValue)
                }
            },
            S::TeTokenOws => match c {
                9 | b' ' => self.eat(S::TeTokenOws),
                _ => Ok(S::TeChunked),
            },
            S::HeadersAlmostDone => {
                if c != 10 {
                    return self.fail(2, &"Expected LF after headers");
                }
                self.head_end()
            }
            S::Eof => {
                self.open_body();
                self.p = self.data.len();
                Ok(S::Eof)
            }
            S::Closed => {
                if matches!(c, 10 | 13) {
                    return self.eat(S::Closed);
                }
                self.eat_fail(5, &"Data after `Connection: close`")
            }
            S::ChunkSizeDigit | S::ChunkSize => {
                let Some(d) = hex(c) else {
                    if state == S::ChunkSize {
                        return Ok(S::ChunkSizeOtherwise);
                    }
                    return self.fail(12, &"Invalid character in chunk size");
                };
                self.p += 1;
                if self.mul_add(16, d) {
                    return Ok(S::ChunkSize);
                }
                self.fail(12, &"Chunk size overflow")
            }
            S::ChunkSizeOtherwise => match c {
                9 | b' ' => self.eat_fail(12, &"Invalid character in chunk size"),
                10 => self.eat_fail(25, &"Missing expected CR after chunk size"),
                13 => self.eat(S::ChunkSizeAlmostDone),
                b';' => self.eat(S::ChunkExtensions),
                _ => self.fail(12, &"Invalid character in chunk size"),
            },
            S::ChunkSizeAlmostDone => {
                if c != 10 {
                    return self.fail(2, &"Expected LF after chunk size");
                }
                self.p += 1;
                if self.parser().content_length == 0 {
                    self.parser().flags |= F_TRAILING;
                    return Ok(S::HeaderFieldStart);
                }
                Ok(S::ChunkData)
            }
            S::ChunkExtensions => match c {
                13 | b' ' => self.eat_fail(2, &"Invalid character in chunk extensions"),
                _ => Ok(S::ChunkExtensionName),
            },
            S::ChunkExtensionName => match c {
                10 => self.fail(25, &"Missing expected CR after chunk extension name"),
                13 => self.eat(S::ChunkSizeAlmostDone),
                b';' => self.eat(S::ChunkExtensions),
                b'=' => self.eat(S::ChunkExtensionValue),
                _ if token(c) => self.eat(S::ChunkExtensionName),
                _ => self.eat_fail(2, &"Invalid character in chunk extensions name"),
            },
            S::ChunkExtensionValue => match c {
                10 => self.fail(25, &"Missing expected CR after chunk extension value"),
                13 => self.eat(S::ChunkSizeAlmostDone),
                b'"' => self.eat(S::ChunkQuotedValue),
                b';' => self.eat(S::ChunkExtensions),
                _ if token(c) => self.eat(S::ChunkExtensionValue),
                _ => self.eat_fail(2, &"Invalid character in chunk extensions value"),
            },
            S::ChunkQuotedValue => match c {
                b'"' => self.eat(S::ChunkQuotedDone),
                b'\\' => self.eat(S::ChunkQuotedPair),
                9 | 0x20..=0xff => self.eat(S::ChunkQuotedValue),
                _ => self.eat_fail(2, &"Invalid character in chunk extensions quoted value"),
            },
            S::ChunkQuotedPair => {
                if value_char(c) {
                    return self.eat(S::ChunkQuotedValue);
                }
                self.eat_fail(2, &"Invalid quoted-pair in chunk extensions quoted value")
            }
            S::ChunkQuotedDone => match c {
                10 => self.fail(25, &"Missing expected CR after chunk extension value"),
                13 => self.eat(S::ChunkSizeAlmostDone),
                b';' => self.eat(S::ChunkExtensions),
                _ => self.fail(2, &"Invalid character in chunk extensions quote value"),
            },
            S::ChunkDataAlmostDone => match c {
                10 => self.eat_fail(25, &"Missing expected CR after chunk data"),
                13 => self.eat(S::ChunkDataAlmostDoneLf),
                _ => self.fail(2, &"Expected LF after chunk data"),
            },
            S::ChunkDataAlmostDoneLf => {
                if c != 10 {
                    return self.fail(2, &"Expected LF after chunk data");
                }
                self.parser().content_length = 0;
                self.eat(S::ChunkSizeDigit)
            }
            S::FieldComplete => self.field_complete(),
            S::AfterHeadersComplete => self.after_headers_complete(),
            S::MessageComplete => self.message_complete(),
            S::UpgradeCheck => Ok(if self.parser().upgrade {
                S::PauseUpgrade
            } else {
                S::AfterMessageComplete
            }),
            S::PauseUpgrade => self.stop(
                HPE_PAUSED_UPGRADE,
                &"Pause on CONNECT/Upgrade",
                S::AfterMessageComplete,
            ),
            S::AfterMessageComplete => Ok(self.after_message_complete()),
            S::Fail => {
                let (error, reason) = self.parser().pending;
                self.fail(error, reason.unwrap_or(&""))
            }
            S::Dead => {
                let parser = self.parser();
                parser.state = S::Dead;
                Err(parser.error)
            }
            S::Body | S::ChunkData => {
                self.open_body();
                self.consume(state)
            }
        }
    }
}
