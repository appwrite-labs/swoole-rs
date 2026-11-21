//! Upstream `StaticHandler::parse_range`, the lazy boundary and the content type: the
//! range parser and the response's accounting, stepped between the caller's effects.
use super::{BOUNDARY_SIZE, HttpStaticContext, StaticEffects};
use crate::range_response::HttpRangeFormat;
use crate::static_meta::{static_boundary, static_multipart};
use crate::{HttpRangeFinish, HttpRangeState, HttpSpan, INVALID, OVERFLOW};

#[derive(Default, Clone, Copy)]
enum Stage {
    #[default]
    Idle,
    Parsing,
    Header,
    CommitHeader,
    Finish,
    End,
    Date,
    Done,
}
#[derive(Default)]
pub(super) struct HttpContextRange {
    stage: Stage,
    cursor: HttpRangeState,
    finish: HttpRangeFinish,
    part: HttpSpan,
    last_length: usize,
    written: usize,
    length: usize,
    if_range: u32,
}
/// What the last effect observed: the part's MIME type, the boundary, the date.
#[derive(Default)]
struct Observation<'a> {
    mime: &'a [u8],
    boundary_ready: u32,
    modified: u32,
}
impl HttpStaticContext {
    /// Upstream's `parse_range(range, if_range)`: the `Range` and `If-Range` headers
    /// (absent as None) into the response's status, content length and tasks.
    pub fn parse_range(
        &mut self,
        range: Option<&[u8]>,
        if_range: Option<&[u8]>,
        effects: &impl StaticEffects,
    ) {
        let range = range.unwrap_or_default();
        if self.range_begin(range.len(), u32::from(if_range.is_some())) < 0 {
            return;
        }
        loop {
            let observed = match self.range_step(range) {
                1 => {
                    // The pinned compiler's argument order: the boundary, then the MIME type.
                    let boundary_ready = self.ensure_boundary(effects);
                    let mime = effects.mime(self.filename());
                    let observed = Observation {
                        mime: &mime,
                        boundary_ready,
                        modified: 0,
                    };
                    if self.range_resume(&observed) == 1 {
                        effects.overflow();
                    }
                    continue;
                }
                2 => Observation {
                    boundary_ready: self.ensure_boundary(effects),
                    ..Observation::default()
                },
                3 => Observation {
                    modified: u32::from(
                        effects.modified_range(if_range.unwrap_or_default(), self.facts.mtime),
                    ),
                    ..Observation::default()
                },
                _ => return,
            };
            self.range_resume(&observed);
        }
    }
    /// Upstream's `ensure_boundary`: the boundary, made from random characters
    /// the first time.
    fn ensure_boundary(&mut self, effects: &impl StaticEffects) -> u32 {
        if let Some(count) = static_boundary(self.boundary.len()) {
            let mut random = [0u8; BOUNDARY_SIZE];
            let count = count.min(BOUNDARY_SIZE - 1);
            effects.random(&mut random, count);
            self.set_boundary(random.get(..count).unwrap_or_default());
        }
        1
    }
    /// The response's `Content-Type`: multipart with several ranges, else the file's.
    pub fn content_type(&mut self, effects: &impl StaticEffects) -> Vec<u8> {
        if !static_multipart(self.tasks.count()) {
            return effects.mime(self.filename());
        }
        self.ensure_boundary(effects);
        self.format_content();
        self.content.clone()
    }
    /// Begin one invocation; previous committed tasks persist, scratch resets.
    fn range_begin(&mut self, length: usize, if_range: u32) -> i32 {
        if length > isize::MAX as usize {
            return OVERFLOW;
        }
        self.range = HttpContextRange {
            length,
            if_range,
            ..HttpContextRange::default()
        };
        self.tasks.begin();
        if length != 0 {
            self.facts.content_length = 0;
            self.range.stage = Stage::Parsing;
        } else {
            self.finishing();
        }
        0
    }
    fn finishing(&mut self) {
        self.range.finish = HttpRangeFinish {
            content_length: self.facts.content_length,
            last_length: self.range.last_length,
            task_count: self.tasks.count(),
            filesize: self.facts.filesize,
            status: self.facts.status,
            if_range: self.range.if_range,
            ..HttpRangeFinish::default()
        };
        self.range.stage = Stage::Finish;
    }
    fn full_task(&mut self) -> i32 {
        self.tasks.prepare_full(0, self.range.last_length);
        self.tasks.push()
    }
    /// Resume the parsing, accounting and commit work until an effect is needed:
    /// done0, header MIME/RNG1, final-boundary RNG2, If-Range date3. Negative
    /// results stop the invocation. Header accounting and push wait for the call
    /// after the caller has applied the returned errno effect.
    fn range_step(&mut self, input: &[u8]) -> i32 {
        if input.len() != self.range.length {
            return INVALID;
        }
        loop {
            match self.range.stage {
                Stage::Parsing => {
                    let result =
                        self.range
                            .cursor
                            .step(input, self.facts.filesize, &mut self.range.part);
                    if result < 0 {
                        self.facts.status = 416;
                        self.range.stage = Stage::Done;
                        return 0;
                    }
                    match result {
                        0 => self.finishing(),
                        2 => {
                            self.range.last_length = self.facts.filesize;
                            self.facts.content_length = self.facts.filesize;
                            let result = self.full_task();
                            self.range.stage = Stage::Done;
                            return result;
                        }
                        _ => {
                            self.range.last_length = self.range.part.length;
                            self.range.stage = Stage::Header;
                            return 1;
                        }
                    }
                }
                Stage::CommitHeader => {
                    self.facts.content_length = self
                        .facts
                        .content_length
                        .wrapping_add(self.range.written)
                        .wrapping_add(self.range.last_length);
                    let result = self.tasks.push();
                    if result < 0 {
                        self.range.stage = Stage::Done;
                        return result;
                    }
                    self.range.stage = Stage::Parsing;
                }
                Stage::Finish => {
                    let action = self.range.finish.step();
                    self.facts.content_length = self.range.finish.content_length;
                    self.facts.status = self.range.finish.status;
                    match action {
                        0 => {
                            self.range.stage = Stage::Done;
                            return 0;
                        }
                        1 => {
                            self.range.last_length = self.range.finish.filesize;
                            let result = self.full_task();
                            if result < 0 {
                                self.range.stage = Stage::Done;
                                return result;
                            }
                        }
                        2 => {
                            self.range.stage = Stage::End;
                            return 2;
                        }
                        3 => {
                            self.range.stage = Stage::Date;
                            return 3;
                        }
                        4 => self.tasks.clear(),
                        _ => return INVALID,
                    }
                }
                Stage::Done => return 0,
                _ => return INVALID,
            }
        }
    }
    /// Admit a completed effect. A header returns 1 only for the EOVERFLOW signal,
    /// which the caller applies before the next step.
    fn range_resume(&mut self, observed: &Observation<'_>) -> i32 {
        match self.range.stage {
            Stage::Header => {
                if observed.boundary_ready == 0 {
                    return INVALID;
                }
                let format = HttpRangeFormat {
                    boundary: &self.boundary,
                    mime: observed.mime,
                    offset: self.range.part.offset,
                    length: self.range.part.length,
                    filesize: self.facts.filesize,
                    part: 0,
                    separator: u32::from(self.tasks.count() != 0),
                };
                let result = self.tasks.prepare_range(
                    i64::try_from(self.range.part.offset).unwrap_or(i64::MAX),
                    self.range.part.length,
                    &format,
                    &mut self.range.written,
                );
                if result >= 0 {
                    self.range.stage = Stage::CommitHeader;
                }
                result
            }
            Stage::End => {
                if observed.boundary_ready == 0 {
                    return INVALID;
                }
                let result = self.format_end();
                if result < 0 {
                    return result;
                }
                self.range.finish.end_length = self.end.len();
                self.range.stage = Stage::Finish;
                0
            }
            Stage::Date => {
                self.range.finish.modified = observed.modified;
                self.range.stage = Stage::Finish;
                0
            }
            _ => INVALID,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HttpStaticObservation, HttpStaticTask};

    struct Fixed {
        modified: bool,
    }
    impl StaticEffects for Fixed {
        fn mime(&self, _: &[u8]) -> Vec<u8> {
            b"text/plain".to_vec()
        }
        fn mime_exists(&self, _: &[u8]) -> bool {
            true
        }
        fn random(&self, out: &mut [u8; BOUNDARY_SIZE], count: usize) {
            out[..count].fill(b'r');
        }
        fn modified_range(&self, _: &[u8], _: i64) -> bool {
            self.modified
        }
        fn overflow(&self) {}
    }
    fn owner() -> HttpStaticContext {
        let mut owner = HttpStaticContext::new(b"/file", 128).unwrap();
        owner.observed(&HttpStaticObservation {
            filesize: 10,
            mtime: 1_700_000_000,
            kind: 32,
            ..HttpStaticObservation::default()
        });
        assert_eq!(owner.publish(super::super::BOUNDARY, b"boundary"), 0);
        owner
    }
    fn observed(modified: u32) -> Observation<'static> {
        Observation {
            mime: b"text/plain",
            boundary_ready: 1,
            modified,
        }
    }
    fn complete(owner: &mut HttpStaticContext, text: &[u8], modified: bool) {
        owner.parse_range(Some(text), Some(b""), &Fixed { modified });
    }
    #[test]
    fn ordinary_empty_file_has_one_initialized_zero_length_task() {
        let mut owner = owner();
        owner.facts.filesize = 0;
        complete(&mut owner, b"bytes=0-", false);
        assert_eq!(owner.tasks.count(), 1);
        assert_eq!(owner.facts.content_length, 0);
        assert_eq!(owner.facts.status, 200);
        assert_eq!(owner.tasks(), [HttpStaticTask::default()]);
    }
    #[test]
    fn header_accounting_waits_for_native_errno_boundary_then_commits() {
        let mut owner = owner();
        let text = b"bytes=1-3,5-6";
        assert_eq!(owner.range_begin(text.len(), 0), 0);
        assert_eq!(owner.range_step(text), 1);
        assert_eq!(owner.range_resume(&observed(0)), 0);
        assert_eq!(owner.facts.content_length, 0);
        assert_eq!(owner.tasks.count(), 0);
        // The caller applies any errno effect before this next call can commit.
        assert_eq!(owner.range_step(text), 1);
        assert_eq!(owner.tasks.count(), 1);
        assert!(owner.facts.content_length > 3);
        assert_eq!(owner.range_resume(&observed(0)), 0);
        assert_eq!(owner.range_step(text), 2);
        assert_eq!(owner.range_resume(&observed(0)), 0);
        assert_eq!(owner.range_step(text), 0);
        assert_eq!(owner.facts.status, 206);
        let tasks: [HttpStaticTask; 2] = owner.tasks().try_into().unwrap();
        assert_eq!(
            (
                tasks[0].offset,
                tasks[0].length,
                tasks[1].offset,
                tasks[1].length
            ),
            (1, 3, 5, 2)
        );
        assert_eq!(owner.end, b"\r\n--boundary--\r\n");
        assert!(tasks[0].part_header.starts_with(b"--boundary"));
    }
    #[test]
    fn date_replacement_and_repeated_empty_parse_preserve_existing_records() {
        let mut owner = owner();
        complete(&mut owner, b"bytes=1-3,5-6", true);
        assert_eq!(owner.facts.status, 200);
        assert_eq!(owner.facts.content_length, 10);
        assert_eq!(owner.tasks.count(), 1);
        complete(&mut owner, b"", false);
        assert_eq!(owner.tasks.count(), 2);
        assert_eq!(owner.facts.content_length, 10);
        assert_eq!(owner.facts.status, 200);
    }
    #[test]
    fn wrong_stage_and_changed_span_metadata_do_not_consume_effects() {
        let mut owner = owner();
        let text = b"bytes=0-0";
        assert_eq!(owner.range_resume(&observed(0)), INVALID);
        assert_eq!(owner.range_begin(text.len(), 0), 0);
        assert_eq!(owner.range_step(&text[..text.len() - 1]), INVALID);
        assert_eq!(owner.tasks.count(), 0);
        assert_eq!(owner.range_step(text), 1);
    }
    #[test]
    fn content_type_is_multipart_over_the_lazy_boundary_for_several_ranges() {
        let mut owner = HttpStaticContext::new(b"/file", 128).unwrap();
        owner.observed(&HttpStaticObservation {
            filesize: 10,
            ..HttpStaticObservation::default()
        });
        let effects = Fixed { modified: false };
        assert_eq!(owner.content_type(&effects), b"text/plain");
        owner.parse_range(Some(b"bytes=0-1,3-4"), None, &effects);
        assert_eq!(
            owner.content_type(&effects),
            b"multipart/byteranges; boundary=SwooleBoundaryrrrrrrrrrrrrrrrrrrrrrrrr"
        );
    }
}
