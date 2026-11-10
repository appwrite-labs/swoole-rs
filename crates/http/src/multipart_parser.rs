//! The streaming `multipart/form-data` parser upstream bundled as
//! `thirdparty/multipart_parser.c` (node-formidable's, by way of libcat), state for
//! state: the same callbacks, results, quirks and error messages.

/// `MPPE_PAUSED`: a callback's result that stops [`MultipartParser::execute`] early.
pub const MPPE_PAUSED: i32 = 1;

/// `enum multipart_error`, the `error_reason` values.
pub const MPPE_OK: u8 = 0;
pub const MPPE_UNKNOWN: u8 = 2;
pub const MPPE_BOUNDARY_END_NO_CRLF: u8 = 3;
pub const MPPE_BAD_START_BOUNDARY: u8 = 4;
pub const MPPE_INVALID_HEADER_FIELD_CHAR: u8 = 5;
pub const MPPE_INVALID_HEADER_VALUE_CHAR: u8 = 6;
pub const MPPE_BAD_PART_END: u8 = 7;
pub const MPPE_END_BOUNDARY_NO_DASH: u8 = 8;
pub const MPPE_HEADER_VALUE_INCOMPLETE: u8 = 9;

const CR: u8 = b'\r';
const LF: u8 = b'\n';

/// The parser's callbacks. Each answers 0 to go on, [`MPPE_PAUSED`] to stop where
/// the parser is, or anything else to fail the call.
pub trait Parts {
    fn on_header_field(&mut self, _data: &[u8]) -> i32 {
        0
    }
    fn on_header_value(&mut self, _data: &[u8]) -> i32 {
        0
    }
    fn on_part_data(&mut self, _data: &[u8]) -> i32 {
        0
    }
    /// Before a part's headers; `boundary` is `--` and the boundary.
    fn on_part_data_begin(&mut self, _boundary: &[u8]) -> i32 {
        0
    }
    fn on_headers_complete(&mut self) -> i32 {
        0
    }
    fn on_part_data_end(&mut self) -> i32 {
        0
    }
    /// After the final boundary; `boundary` is `--` and the boundary.
    fn on_body_end(&mut self, _boundary: &[u8]) -> i32 {
        0
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
    boundary: Vec<u8>,
    /// Upstream's `unsigned char` boundary length, two more than the boundary's.
    boundary_length: u8,
    index: usize,
    state: State,
    /// Where the last error was, its `MPPE_*` reason, the byte expected (0 for none)
    /// and the byte found.
    pub error_i: usize,
    pub error_reason: u8,
    pub error_expected: u8,
    pub error_unexpected: u8,
}

/// What one step of [`MultipartParser::execute`] did with its byte.
enum Step {
    Next,
    Return(isize),
}

impl MultipartParser {
    /// `multipart_parser_init(boundary, length, settings)`.
    #[must_use]
    pub fn new(boundary: &[u8]) -> Self {
        let mut stored = b"--".to_vec();
        stored.extend_from_slice(boundary);
        // Upstream terminates the copy; the boundary match can read that NUL.
        stored.push(0);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream's unsigned char boundary_length"
        )]
        let boundary_length = boundary.len().wrapping_add(2) as u8;
        MultipartParser {
            boundary: stored,
            boundary_length,
            index: 0,
            state: State::Start,
            error_i: 0,
            error_reason: MPPE_OK,
            error_expected: 0,
            error_unexpected: 0,
        }
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
        let head: &[u8] = match self.error_reason {
            MPPE_OK => return Vec::new(),
            MPPE_PAUSED_REASON => return b"parser paused".to_vec(),
            MPPE_UNKNOWN => return b"parser unknown".to_vec(),
            MPPE_BOUNDARY_END_NO_CRLF => b"no CRLF at first boundary end: ",
            MPPE_BAD_START_BOUNDARY => b"first boundary mismatching: ",
            MPPE_INVALID_HEADER_FIELD_CHAR => b"invalid char in header field: ",
            MPPE_INVALID_HEADER_VALUE_CHAR => b"invalid char in header value: ",
            MPPE_BAD_PART_END => b"no next part or final hyphen: expecting CR or '-' ",
            MPPE_END_BOUNDARY_NO_DASH => b"bad final hyphen: ",
            _ => return b"parser abort".to_vec(),
        };
        let mut text = head.to_vec();
        match self.error_expected {
            0 => {}
            CR => text.extend_from_slice(b"expecting CR "),
            LF => text.extend_from_slice(b"expecting LF "),
            byte => {
                text.extend_from_slice(b"expecting '");
                text.extend_from_slice(&[byte, b'\'', b' ']);
            }
        }
        let found = self.error_unexpected;
        text.extend_from_slice(format!("at {}, but it is '", self.error_i).as_bytes());
        if (b' '..=b'~').contains(&found) {
            text.push(found);
        } else {
            // `%.2x` of a signed char: bytes past 0x7f print sign-extended.
            #[expect(clippy::cast_possible_wrap, reason = "C's signed char")]
            let shown = i32::from(found as i8).cast_unsigned();
            text.extend_from_slice(format!("\\x{shown:02x}").as_bytes());
        }
        text.push(b'\'');
        text
    }

    fn fail(&mut self, reason: u8, at: usize, found: u8) -> Step {
        self.error_unexpected = found;
        self.error_i = at;
        self.error_reason = reason;
        Step::Return(-1)
    }

    fn expect(&mut self, reason: u8, expected: u8, at: usize, found: u8) -> Step {
        self.error_expected = expected;
        self.fail(reason, at, found)
    }

    /// `multipart_parser_execute`: how many bytes were consumed, or -1 on an error.
    /// Header fields and values must each arrive whole within one call.
    pub fn execute(&mut self, parts: &mut impl Parts, buf: &[u8]) -> isize {
        let mut mark = 0;
        for (i, &c) in buf.iter().enumerate() {
            if let Step::Return(result) = self.step(parts, buf, i, c, &mut mark) {
                return result;
            }
        }
        isize::try_from(buf.len()).unwrap_or(isize::MAX)
    }

    /// One byte's work, with upstream's fallthroughs as loops over the state.
    #[expect(
        clippy::too_many_lines,
        reason = "one state machine, as upstream's switch"
    )]
    fn step(
        &mut self,
        parts: &mut impl Parts,
        buf: &[u8],
        i: usize,
        c: u8,
        mark: &mut usize,
    ) -> Step {
        let is_last = i + 1 == buf.len();
        let at = |i: usize| isize::try_from(i).unwrap_or(isize::MAX);
        // NOTIFY_CB / EMIT_DATA_CB: a pause returns `r`, other failures -1.
        let done = |result: i32, r: usize| match result {
            0 => None,
            MPPE_PAUSED => Some(Step::Return(at(r))),
            _ => Some(Step::Return(-1)),
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
                            return self.expect(MPPE_BOUNDARY_END_NO_CRLF, CR, i, c);
                        }
                        self.index += 1;
                        return Step::Next;
                    }
                    if self.index == length + 1 {
                        if c != LF {
                            return self.expect(MPPE_BOUNDARY_END_NO_CRLF, LF, i, c);
                        }
                        self.index = 0;
                        self.state = State::HeaderFieldStart;
                        let result = parts.on_part_data_begin(self.boundary());
                        return done(result, i + 1).unwrap_or(Step::Next);
                    }
                    let expected = self.boundary_byte(self.index);
                    if c != expected {
                        return self.expect(MPPE_BAD_START_BOUNDARY, expected, i, c);
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
                        return self.fail(MPPE_INVALID_HEADER_FIELD_CHAR, i, c);
                    }
                    if is_last {
                        let result = parts.on_header_field(span(*mark, i + 1));
                        return done(result, i + 1).unwrap_or(Step::Next);
                    }
                    return Step::Next;
                }
                State::HeadersAlmostDone => {
                    if c != LF {
                        return self.expect(MPPE_INVALID_HEADER_VALUE_CHAR, LF, i, c);
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
                        return done(result, i + 1).unwrap_or(Step::Next);
                    }
                    if is_last {
                        return self.expect(MPPE_HEADER_VALUE_INCOMPLETE, CR, i, c);
                    }
                    return Step::Next;
                }
                State::HeaderValueAlmostDone => {
                    if c != LF {
                        return self.expect(MPPE_INVALID_HEADER_VALUE_CHAR, LF, i, c);
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
                    return self.fail(MPPE_BAD_PART_END, i, c);
                }
                State::PartDataFinalHyphen => {
                    if c == b'-' {
                        self.state = State::End;
                        let result = parts.on_body_end(self.boundary());
                        return done(result, i).unwrap_or(Step::Next);
                    }
                    return self.expect(MPPE_END_BOUNDARY_NO_DASH, b'-', i, c);
                }
                State::PartDataEnd => {
                    if c == LF {
                        self.state = State::HeaderFieldStart;
                        let result = parts.on_part_data_begin(self.boundary());
                        return done(result, i + 1).unwrap_or(Step::Next);
                    }
                    return self.expect(MPPE_END_BOUNDARY_NO_DASH, b'-', i, c);
                }
                State::End => return Step::Next,
            }
        }
    }
}

/// `MPPE_PAUSED` as an `error_reason`: nothing in the parser sets it.
const MPPE_PAUSED_REASON: u8 = 1;
