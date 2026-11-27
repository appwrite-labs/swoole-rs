//! The streaming `multipart/form-data` parser upstream bundled as
//! `thirdparty/multipart_parser.c` (node-formidable's, by way of libcat), state for
//! state: the same callbacks, results, quirks and error messages.

use swoole_rs_corebase::{buffer::BoundedBytes, constants::SW_HTTP_HEADER_MAX_SIZE};

/// A part header assembled across calls, with one shared limit for its name and value.
#[derive(Debug)]
pub struct MultipartHeader {
    bytes: BoundedBytes,
    name_len: usize,
}

impl MultipartHeader {
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            bytes: BoundedBytes::new(limit.min(SW_HTTP_HEADER_MAX_SIZE as usize), 0),
            name_len: 0,
        }
    }

    pub fn append_name(&mut self, data: &[u8]) -> Flow {
        let flow = self.append_value(data);
        self.name_len = self.bytes.as_ref().len();
        flow
    }

    pub fn append_value(&mut self, data: &[u8]) -> Flow {
        if self.bytes.append(data).is_ok() {
            Flow::Go
        } else {
            Flow::Fail
        }
    }

    #[must_use]
    pub fn parts(&self) -> (&[u8], &[u8]) {
        self.bytes.as_ref().split_at(self.name_len)
    }

    pub fn clear(&mut self) {
        self.bytes.clear();
        self.name_len = 0;
    }
}

/// A callback's result: upstream's 0 goes on, `MPPE_PAUSED` stops
/// [`MultipartParser::execute`] where the parser is, any other value fails the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Go,
    Pause,
    Fail,
}

/// `enum multipart_error` past `MPPE_OK`: why the last call failed. The parser
/// never sets `Paused` or `Unknown`, which only have messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultipartError {
    Paused,
    Unknown,
    BoundaryEndNoCrlf,
    BadStartBoundary,
    InvalidHeaderFieldChar,
    InvalidHeaderValueChar,
    BadPartEnd,
    EndBoundaryNoDash,
    HeaderValueIncomplete,
}

/// Why execution stopped with an error. Callback failures leave the parser's
/// stored diagnostic unchanged, while syntax errors also update it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    Parser(MultipartError),
    Callback,
}

const CR: u8 = b'\r';
const LF: u8 = b'\n';

/// The parser's callbacks, each answering a [`Flow`]. A header field or value
/// is a slice of the input `execute` was given, lent for that whole call (`'i`).
pub trait Parts<'i> {
    fn on_header_field(&mut self, _data: &'i [u8]) -> Flow {
        Flow::Go
    }
    fn on_header_value(&mut self, _data: &'i [u8]) -> Flow {
        Flow::Go
    }
    fn on_header_value_complete(&mut self) -> Flow {
        Flow::Go
    }
    fn on_part_data(&mut self, _data: &[u8]) -> Flow {
        Flow::Go
    }
    /// Before a part's headers; `boundary` is `--` and the boundary.
    fn on_part_data_begin(&mut self, _boundary: &[u8]) -> Flow {
        Flow::Go
    }
    fn on_headers_complete(&mut self) -> Flow {
        Flow::Go
    }
    fn on_part_data_end(&mut self) -> Flow {
        Flow::Go
    }
    /// After the final boundary; `boundary` is `--` and the boundary.
    fn on_body_end(&mut self, _boundary: &[u8]) -> Flow {
        Flow::Go
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Start,
    StartBoundary,
    HeaderFieldStart,
    HeaderField,
    HeadersAlmostDone,
    HeaderValueStart,
    HeaderValue,
    HeaderValueAlmostDone,
    PartDataStart,
    PartData,
    PartDataAlmostBoundary,
    PartDataBoundary,
    PartDataAlmostEnd,
    PartDataEnd,
    PartDataFinalHyphen,
    End,
}

/// `struct multipart_parser`.
#[derive(Debug)]
pub struct MultipartParser {
    /// `--` and the boundary.
    boundary: [u8; u8::MAX as usize],
    /// Upstream's `unsigned char` boundary length, two more than the boundary's.
    boundary_length: u8,
    index: usize,
    state: State,
    /// Where the last error was, its reason (`None` for `MPPE_OK`), the byte expected
    /// (0 for none) and the byte found.
    pub error_i: usize,
    pub error_reason: Option<MultipartError>,
    pub error_expected: u8,
    pub error_unexpected: u8,
}

/// What one step of [`MultipartParser::execute`] did with its byte.
enum Step {
    Next,
    Return(Result<usize, ParseError>),
}

impl MultipartParser {
    /// Build the parser when the boundary and its two leading dashes fit the
    /// upstream parser's one-byte boundary length.
    #[must_use]
    pub fn new(boundary: &[u8]) -> Option<Self> {
        let mut stored = [0; u8::MAX as usize];
        let (prefix, rest) = stored.split_first_chunk_mut::<2>()?;
        *prefix = *b"--";
        rest.get_mut(..boundary.len())?.copy_from_slice(boundary);
        let boundary_length = u8::try_from(boundary.len() + 2).ok()?;
        Some(Self {
            boundary: stored,
            boundary_length,
            index: 0,
            state: State::Start,
            error_i: 0,
            error_reason: None,
            error_expected: 0,
            error_unexpected: 0,
        })
    }

    /// Whether the parser has consumed the closing multipart boundary.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.state == State::End
    }

    /// `--` and the boundary, as long as upstream's `boundary_length` says.
    #[must_use]
    pub fn boundary(&self) -> &[u8] {
        self.prefix(usize::from(self.boundary_length))
    }

    fn prefix(&self, length: usize) -> &[u8] {
        self.boundary.get(..length).unwrap_or(&self.boundary)
    }

    fn boundary_byte(&self, index: usize) -> u8 {
        self.boundary.get(index).copied().unwrap_or(0)
    }

    /// `multipart_parser_error_msg`: the last error, as upstream words it.
    #[must_use]
    pub fn error_message(&self) -> Vec<u8> {
        use MultipartError as E;
        use swoole_rs_corebase::net_address::format_into;
        let Some(reason) = self.error_reason else {
            return Vec::new();
        };
        let head: &[u8] = match reason {
            E::Paused => return b"parser paused".to_vec(),
            E::Unknown => return b"parser unknown".to_vec(),
            E::BoundaryEndNoCrlf => b"no CRLF at first boundary end: ",
            E::BadStartBoundary => b"first boundary mismatching: ",
            E::InvalidHeaderFieldChar => b"invalid char in header field: ",
            E::InvalidHeaderValueChar => b"invalid char in header value: ",
            E::BadPartEnd => b"no next part or final hyphen: expecting CR or '-' ",
            E::EndBoundaryNoDash => b"bad final hyphen: ",
            // Upstream's message switch has no case for it.
            E::HeaderValueIncomplete => return b"parser abort".to_vec(),
        };
        let byte = [self.error_expected, b'\'', b' '];
        let (expecting, quoted): (&[u8], &[u8]) = match self.error_expected {
            0 => (b"", b""),
            CR => (b"expecting CR ", b""),
            LF => (b"expecting LF ", b""),
            _ => (b"expecting '", &byte),
        };
        let mut digits = [0; usize::MAX.ilog10() as usize + 1];
        let offset = format_into(&mut digits, format_args!("{}", self.error_i)).unwrap_or_default();
        let found = self.error_unexpected;
        let mut escaped = [0; b"\\xffffffff".len()];
        let shown = if (b' '..=b'~').contains(&found) {
            std::slice::from_ref(&found)
        } else {
            // `%.2x` of a signed char: bytes past 0x7f print sign-extended.
            let hex = i32::from(found.cast_signed()).cast_unsigned();
            format_into(&mut escaped, format_args!("\\x{hex:02x}")).unwrap_or_default()
        };
        [
            head,
            expecting,
            quoted,
            b"at ",
            offset,
            b", but it is '",
            shown,
            b"'",
        ]
        .concat()
    }

    fn fail(&mut self, reason: MultipartError, at: usize, found: u8) -> Step {
        self.error_unexpected = found;
        self.error_i = at;
        self.error_reason = Some(reason);
        Step::Return(Err(ParseError::Parser(reason)))
    }

    fn expect(&mut self, reason: MultipartError, expected: u8, at: usize, found: u8) -> Step {
        self.error_expected = expected;
        self.fail(reason, at, found)
    }

    /// `multipart_parser_execute`: how many bytes were consumed, including a pause.
    /// # Errors
    /// A syntax error or a failed callback. A callback failure preserves the
    /// stored diagnostic, matching the original parser.
    /// Header fields and values may span calls.
    pub fn execute<'i>(
        &mut self,
        parts: &mut impl Parts<'i>,
        buf: &'i [u8],
    ) -> Result<usize, ParseError> {
        let mut mark = 0;
        let mut i = 0;
        while let Some(&c) = buf.get(i) {
            if let Step::Return(result) = self.step(parts, buf, i, c, &mut mark) {
                return result;
            }
            i += 1;
            // Part data changes nothing until a CR or the last byte, so go straight there.
            if self.state == State::PartData {
                let rest = buf.get(i..).unwrap_or_default();
                i += rest
                    .iter()
                    .position(|&c| c == CR)
                    .unwrap_or_else(|| rest.len().saturating_sub(1));
            }
        }
        Ok(buf.len())
    }

    /// One byte's work, with upstream's fallthroughs as loops over the state.
    #[expect(
        clippy::too_many_lines,
        reason = "one state machine, as upstream's switch"
    )]
    fn step<'i>(
        &mut self,
        parts: &mut impl Parts<'i>,
        buf: &'i [u8],
        i: usize,
        c: u8,
        mark: &mut usize,
    ) -> Step {
        let is_last = i + 1 == buf.len();
        // NOTIFY_CB / EMIT_DATA_CB: a pause returns the consumed offset.
        let done = |result: Flow, r: usize| match result {
            Flow::Go => None,
            Flow::Pause => Some(Step::Return(Ok(r))),
            Flow::Fail => Some(Step::Return(Err(ParseError::Callback))),
        };
        let span = |from: usize, to: usize| buf.get(from..to).unwrap_or_default();
        loop {
            match self.state {
                State::Start => {
                    self.index = 0;
                    self.state = State::StartBoundary;
                }
                State::StartBoundary => {
                    let length = usize::from(self.boundary_length);
                    if self.index == length {
                        // swoole/swoole-src#5168
                        if c == b'-' {
                            self.state = State::PartDataFinalHyphen;
                        } else if c != CR {
                            return self.expect(MultipartError::BoundaryEndNoCrlf, CR, i, c);
                        }
                        self.index += 1;
                        return Step::Next;
                    }
                    if self.index == length + 1 {
                        if c != LF {
                            return self.expect(MultipartError::BoundaryEndNoCrlf, LF, i, c);
                        }
                        self.index = 0;
                        self.state = State::HeaderFieldStart;
                        let result = parts.on_part_data_begin(self.boundary());
                        return done(result, i + 1).unwrap_or(Step::Next);
                    }
                    let expected = self.boundary_byte(self.index);
                    if c != expected {
                        return self.expect(MultipartError::BadStartBoundary, expected, i, c);
                    }
                    self.index += 1;
                    return Step::Next;
                }
                State::HeaderFieldStart => {
                    *mark = i;
                    self.state = State::HeaderField;
                }
                State::HeaderField => {
                    if c == CR {
                        self.state = State::HeadersAlmostDone;
                        return Step::Next;
                    }
                    if c == b':' {
                        self.state = State::HeaderValueStart;
                        let result = parts.on_header_field(span(*mark, i));
                        return done(result, i + 1).unwrap_or(Step::Next);
                    }
                    let lower = c | 0x20;
                    if c != b'-' && !lower.is_ascii_lowercase() {
                        return self.fail(MultipartError::InvalidHeaderFieldChar, i, c);
                    }
                    if is_last {
                        let result = parts.on_header_field(span(*mark, i + 1));
                        return done(result, i + 1).unwrap_or(Step::Next);
                    }
                    return Step::Next;
                }
                State::HeadersAlmostDone => {
                    if c != LF {
                        return self.expect(MultipartError::InvalidHeaderValueChar, LF, i, c);
                    }
                    self.state = State::PartDataStart;
                    return Step::Next;
                }
                State::HeaderValueStart => {
                    if c == b' ' {
                        return Step::Next;
                    }
                    *mark = i;
                    self.state = State::HeaderValue;
                }
                State::HeaderValue => {
                    if c == CR {
                        self.state = State::HeaderValueAlmostDone;
                        let result = parts.on_header_value(span(*mark, i));
                        return done(result, i + 1)
                            .or_else(|| done(parts.on_header_value_complete(), i + 1))
                            .unwrap_or(Step::Next);
                    }
                    if is_last {
                        return done(parts.on_header_value(span(*mark, i + 1)), i + 1)
                            .unwrap_or(Step::Next);
                    }
                    return Step::Next;
                }
                State::HeaderValueAlmostDone => {
                    if c != LF {
                        return self.expect(MultipartError::InvalidHeaderValueChar, LF, i, c);
                    }
                    self.state = State::HeaderFieldStart;
                    return Step::Next;
                }
                State::PartDataStart => {
                    *mark = i;
                    self.state = State::PartData;
                    if let Some(step) = done(parts.on_headers_complete(), i) {
                        return step;
                    }
                }
                State::PartData => {
                    let mark_end = i + 1;
                    if c == CR {
                        if i > *mark {
                            let result = parts.on_part_data(span(*mark, i));
                            if let Some(step) = done(result, i + 1) {
                                return step;
                            }
                        }
                        *mark = i;
                        self.state = State::PartDataAlmostBoundary;
                        return Step::Next;
                    }
                    if is_last {
                        let result = parts.on_part_data(span(*mark, mark_end));
                        return done(result, i + 1).unwrap_or(Step::Next);
                    }
                    return Step::Next;
                }
                State::PartDataAlmostBoundary => {
                    if c == LF {
                        self.state = State::PartDataBoundary;
                        self.index = 0;
                        return Step::Next;
                    }
                    if let Some(step) = done(parts.on_part_data(b"\r"), i + 1) {
                        return step;
                    }
                    self.state = State::PartData;
                    *mark = i;
                }
                State::PartDataBoundary => {
                    if self.boundary_byte(self.index) == c {
                        self.index += 1;
                        if self.index == usize::from(self.boundary_length) {
                            self.state = State::PartDataAlmostEnd;
                        }
                        return Step::Next;
                    }
                    if let Some(step) = done(parts.on_part_data(b"\r\n"), i + 1) {
                        return step;
                    }
                    if self.index > 0 {
                        let result = parts.on_part_data(self.prefix(self.index));
                        if let Some(step) = done(result, i + 1) {
                            return step;
                        }
                    }
                    *mark = i;
                    self.state = State::PartData;
                }
                State::PartDataAlmostEnd => {
                    if c == b'-' {
                        self.state = State::PartDataFinalHyphen;
                        return done(parts.on_part_data_end(), i + 1).unwrap_or(Step::Next);
                    }
                    if c == CR {
                        self.state = State::PartDataEnd;
                        return done(parts.on_part_data_end(), i + 1).unwrap_or(Step::Next);
                    }
                    return self.fail(MultipartError::BadPartEnd, i, c);
                }
                State::PartDataFinalHyphen => {
                    if c == b'-' {
                        self.state = State::End;
                        let result = parts.on_body_end(self.boundary());
                        return done(result, i).unwrap_or(Step::Next);
                    }
                    return self.expect(MultipartError::EndBoundaryNoDash, b'-', i, c);
                }
                State::PartDataEnd => {
                    if c == LF {
                        self.state = State::HeaderFieldStart;
                        let result = parts.on_part_data_begin(self.boundary());
                        return done(result, i + 1).unwrap_or(Step::Next);
                    }
                    return self.expect(MultipartError::EndBoundaryNoDash, b'-', i, c);
                }
                State::End => return Step::Next,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &[u8] = b"--AB\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\nhello\r\n--AB\r\nX-Y: z\r\n\r\nworld\r\n--AB--";

    /// Every callback as an event; the event numbered `pause` pauses.
    #[derive(Default)]
    struct Recorder {
        events: Vec<String>,
        pause: Option<usize>,
    }

    impl Recorder {
        fn push(&mut self, event: String) -> Flow {
            let paused = self.pause == Some(self.events.len());
            self.events.push(event);
            if paused { Flow::Pause } else { Flow::Go }
        }
    }

    impl Parts<'_> for Recorder {
        fn on_header_field(&mut self, data: &[u8]) -> Flow {
            self.push(format!("field {}", String::from_utf8_lossy(data)))
        }
        fn on_header_value(&mut self, data: &[u8]) -> Flow {
            self.push(format!("value {}", String::from_utf8_lossy(data)))
        }
        fn on_part_data(&mut self, data: &[u8]) -> Flow {
            self.push(format!("data {}", String::from_utf8_lossy(data)))
        }
        fn on_part_data_begin(&mut self, _: &[u8]) -> Flow {
            self.push("begin".into())
        }
        fn on_headers_complete(&mut self) -> Flow {
            self.push("headers".into())
        }
        fn on_part_data_end(&mut self) -> Flow {
            self.push("end".into())
        }
        fn on_body_end(&mut self, _: &[u8]) -> Flow {
            self.push("body end".into())
        }
    }

    fn whole() -> Vec<String> {
        [
            "begin",
            "field Content-Disposition",
            "value form-data; name=\"a\"",
            "headers",
            "data hello",
            "end",
            "begin",
            "field X-Y",
            "value z",
            "headers",
            "data world",
            "end",
            "body end",
        ]
        .map(String::from)
        .to_vec()
    }

    fn len(bytes: &[u8]) -> isize {
        isize::try_from(bytes.len()).unwrap()
    }

    fn find(needle: &[u8]) -> usize {
        BODY.windows(needle.len())
            .position(|w| w == needle)
            .unwrap()
    }

    /// Pausing at event `pause` returns `consumed` and leaves the parser in `state`;
    /// resuming from there yields the same events as an unpaused parse.
    fn pause_and_resume(pause: usize, consumed: usize, state: State) {
        let mut parser = MultipartParser::new(b"AB").unwrap();
        let mut parts = Recorder {
            pause: Some(pause),
            ..Recorder::default()
        };
        assert_eq!(
            parser
                .execute(&mut parts, BODY)
                .map_or(-1, usize::cast_signed),
            len(&BODY[..consumed]),
            "paused"
        );
        assert_eq!(parts.events, whole()[..=pause], "events up to the pause");
        assert_eq!(parser.state, state, "state after the pause");
        assert_eq!(parser.error_reason, None, "a pause is not an error");
        parts.pause = None;
        let rest = &BODY[consumed..];
        assert_eq!(
            parser
                .execute(&mut parts, rest)
                .map_or(-1, usize::cast_signed),
            len(rest),
            "resumed"
        );
        assert_eq!(parts.events, whole(), "the parts in full");
        assert_eq!(parser.state, State::End, "body ended");
    }

    #[test]
    fn unpaused() {
        let mut parser = MultipartParser::new(b"AB").unwrap();
        let mut parts = Recorder::default();
        assert_eq!(
            parser
                .execute(&mut parts, BODY)
                .map_or(-1, usize::cast_signed),
            len(BODY),
            "all parsed"
        );
        assert_eq!(parts.events, whole(), "the parts in full");
    }

    #[test]
    fn pause_in_part_data_begin() {
        // After "--AB\r\n".
        pause_and_resume(0, 6, State::HeaderFieldStart);
    }

    #[test]
    fn pause_in_header_field() {
        // After the colon.
        pause_and_resume(1, find(b":") + 1, State::HeaderValueStart);
    }

    #[test]
    fn pause_in_headers_complete() {
        // At the part's first data byte, which is not consumed.
        pause_and_resume(3, find(b"hello"), State::PartData);
    }

    #[test]
    fn pause_in_part_data_end() {
        // After the CR that follows the first part's closing boundary.
        pause_and_resume(5, find(b"--AB\r\nX") + 5, State::PartDataEnd);
    }

    #[test]
    fn pause_in_part_data_at_chunk_end() {
        // A data span flushed at a chunk's end: the whole chunk is consumed.
        let mut parser = MultipartParser::new(b"AB").unwrap();
        let mut parts = Recorder {
            pause: Some(4),
            ..Recorder::default()
        };
        let split = find(b"hello") + 3;
        let (head, rest) = BODY.split_at(split);
        assert_eq!(
            parser
                .execute(&mut parts, head)
                .map_or(-1, usize::cast_signed),
            len(head),
            "paused"
        );
        assert_eq!(
            parts.events.last().map(String::as_str),
            Some("data hel"),
            "paused in data"
        );
        assert_eq!(parser.state, State::PartData, "state after the pause");
        parts.pause = None;
        assert_eq!(
            parser
                .execute(&mut parts, rest)
                .map_or(-1, usize::cast_signed),
            len(rest),
            "resumed"
        );
        let mut expected = whole();
        expected.splice(4..5, ["data hel".to_owned(), "data lo".to_owned()]);
        assert_eq!(parts.events, expected, "the parts in full");
    }

    #[test]
    fn diagnostic_keeps_binary_expected_byte_and_full_offset() {
        let mut parser = MultipartParser::new(b"AB").unwrap();
        parser.error_reason = Some(MultipartError::BadStartBoundary);
        parser.error_i = usize::MAX;
        parser.error_expected = 0xff;
        parser.error_unexpected = 0x80;
        let tail = format!("at {}, but it is '\\xffffff80'", usize::MAX);
        assert_eq!(
            parser.error_message(),
            [
                b"first boundary mismatching: expecting '\xff' ".as_slice(),
                tail.as_bytes()
            ]
            .concat()
        );
    }

    #[test]
    fn callback_failure_keeps_the_stored_diagnostic() {
        struct Failing;
        impl Parts<'_> for Failing {
            fn on_part_data_begin(&mut self, _: &[u8]) -> Flow {
                Flow::Fail
            }
        }
        for prior in [None, Some(MultipartError::BadStartBoundary)] {
            let mut parser = MultipartParser::new(b"AB").unwrap();
            parser.error_reason = prior;
            parser.error_i = 7;
            parser.error_expected = b'x';
            parser.error_unexpected = b'y';
            let message = parser.error_message();
            assert_eq!(
                parser.execute(&mut Failing, BODY),
                Err(ParseError::Callback)
            );
            assert_eq!(parser.state, State::HeaderFieldStart);
            assert_eq!(parser.error_reason, prior);
            assert_eq!(parser.error_i, 7);
            assert_eq!(parser.error_expected, b'x');
            assert_eq!(parser.error_unexpected, b'y');
            assert_eq!(parser.error_message(), message);
            // The failed begin callback consumed the boundary, as before.
            let mut parts = Recorder::default();
            assert_eq!(parser.execute(&mut parts, &BODY[6..]), Ok(BODY.len() - 6));
            assert_eq!(parts.events, whole()[1..]);
        }
    }

    #[test]
    fn syntax_failure_carries_its_reason_and_empty_input_consumes_zero() {
        let mut parser = MultipartParser::new(b"AB").unwrap();
        let mut parts = Recorder::default();
        assert_eq!(parser.execute(&mut parts, b""), Ok(0));
        assert_eq!(
            parser.execute(&mut parts, b"--AX"),
            Err(ParseError::Parser(MultipartError::BadStartBoundary))
        );
        assert_eq!(parser.error_reason, Some(MultipartError::BadStartBoundary));
        assert_eq!(parser.error_i, 3);
        assert_eq!(parser.error_expected, b'B');
        assert_eq!(parser.error_unexpected, b'X');
        assert_eq!(parts.events, Vec::<String>::new());
    }
}
