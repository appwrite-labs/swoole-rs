//! Static byte-range selection and multipart response assembly.
use super::{BOUNDARY_SIZE, HttpStaticContext, StaticEffects};
use crate::range::select;
use crate::range_response::HttpRangeFormat;
use crate::static_meta::{static_boundary, static_multipart};
use crate::{HttpSpan, OVERFLOW};

impl HttpStaticContext {
    /// Select the file spans and build their response headers.
    pub fn parse_range(
        &mut self,
        range: Option<&[u8]>,
        if_range: Option<&[u8]>,
        effects: &impl StaticEffects,
    ) {
        self.tasks.clear();
        self.end.clear();
        self.facts.content_length = 0;
        let range = crate::c_text(range.unwrap_or_default());
        let size = self.facts.filesize;
        let mut storage = std::array::from_fn(|_| HttpSpan::default());
        let if_range = crate::c_text(if_range.unwrap_or_default());
        let selection = if size == 0
            || !crate::case_prefix(range, b"bytes=")
            || (!if_range.is_empty() && effects.modified_range(if_range, self.facts.mtime))
        {
            Ok(None)
        } else {
            select(range.get(6..).unwrap_or_default(), size, &mut storage)
        };
        let full = [HttpSpan {
            offset: 0,
            length: size,
        }];
        if let Ok(parts) = selection
            && let Ok(length) = self.prepare_parts(parts.unwrap_or(&full), effects)
        {
            self.facts.content_length = length;
            self.facts.status = if parts.is_some() {
                crate::status::PARTIAL_CONTENT
            } else {
                200
            };
            return;
        }
        self.tasks.clear();
        self.end.clear();
        self.facts.status = crate::status::RANGE_NOT_SATISFIABLE;
    }

    fn prepare_parts(
        &mut self,
        parts: &[HttpSpan],
        effects: &impl StaticEffects,
    ) -> Result<usize, i32> {
        let mut total = 0usize;
        for part in parts {
            let offset = i64::try_from(part.offset).map_err(|_| OVERFLOW)?;
            self.tasks.begin();
            let mut written = 0;
            if parts.len() == 1 {
                self.tasks.prepare_full(offset, part.length);
            } else {
                self.ensure_boundary(effects);
                let format = HttpRangeFormat {
                    boundary: &self.boundary,
                    mime: &effects.mime(self.filename()),
                    offset: part.offset,
                    length: part.length,
                    filesize: self.facts.filesize,
                    part: 0,
                    separator: self.tasks.count() != 0,
                };
                self.tasks
                    .prepare_range(offset, part.length, &format, &mut written)?;
            }
            total = total
                .checked_add(part.length)
                .and_then(|n| n.checked_add(written))
                .ok_or(OVERFLOW)?;
            self.tasks.push()?;
        }
        if parts.len() > 1 {
            self.format_end()?;
        }
        total.checked_add(self.end.len()).ok_or(OVERFLOW)
    }
    /// Upstream's `ensure_boundary`: the boundary, made from random characters
    /// the first time.
    fn ensure_boundary(&mut self, effects: &impl StaticEffects) {
        if let Some(count) = static_boundary(self.boundary.len()) {
            let mut random = [0u8; BOUNDARY_SIZE];
            let count = count.min(BOUNDARY_SIZE - 1);
            effects.random(&mut random, count);
            _ = self.set_boundary(random.get(..count).unwrap_or_default());
        }
    }
    /// The response's `Content-Type`: multipart with several ranges, else the file's.
    pub fn content_type(&mut self, effects: &impl StaticEffects) -> Vec<u8> {
        if !static_multipart(self.tasks.count()) {
            return effects.mime(self.filename());
        }
        self.ensure_boundary(effects);
        _ = self.format_content();
        self.content.clone()
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
            kind: crate::StaticFileKind::File,
            ..HttpStaticObservation::default()
        });
        assert_eq!(
            owner.publish(super::super::StaticText::Boundary, b"boundary"),
            Ok(())
        );
        owner
    }
    fn complete(owner: &mut HttpStaticContext, text: &[u8], modified: bool) {
        owner.parse_range(Some(text), Some(b"date"), &Fixed { modified });
    }
    #[test]
    fn range_errors_discard_complete_and_partial_plans() {
        let mut owner = owner();
        for header in [
            "bytes=word",
            "bytes=0-1,word",
            "bytes=0-1,4-5,word",
            "bytes=0-1,",
            "bytes=0-1,999999999999999999999999999999999999-",
            "bytes=10-20",
            "bytes=8-2",
        ] {
            complete(&mut owner, b"bytes=0-1,4-5", false);
            assert_eq!(owner.facts.status, 206);
            complete(&mut owner, header.as_bytes(), false);
            assert_eq!(owner.facts.status, 416, "{header}");
            assert_eq!(owner.tasks(), []);
            assert_eq!(owner.facts.content_length, 0);
            assert_eq!(owner.end, b"");
        }
    }

    #[test]
    fn range_boundaries_and_cost_limits() {
        let mut owner = owner();
        owner.facts.filesize = 100;
        complete(&mut owner, b"bytes=0-0,-1", false);
        assert_eq!(owner.facts.status, 206);
        assert_eq!(owner.tasks.count(), 2);
        assert_eq!((owner.tasks()[0].offset, owner.tasks()[1].offset), (0, 99));
        let length = owner.tasks().iter().fold(owner.end.len(), |sum, task| {
            sum + crate::c_text(&task.part_header).len() + task.length
        });
        assert_eq!(owner.facts.content_length, length);
        complete(&mut owner, b"bytes=0-99,0-99", false);
        assert_eq!((owner.facts.status, owner.facts.content_length), (200, 100));
        complete(
            &mut owner,
            format!("bytes={}", ["0-0"; 17].join(",")).as_bytes(),
            false,
        );
        assert_eq!(owner.facts.status, 200);
        complete(&mut owner, b"bytes=-0", false);
        assert_eq!(owner.facts.status, 416);
        owner.facts.filesize = 0;
        complete(&mut owner, b"bytes=0-0", false);
        assert_eq!((owner.facts.status, owner.facts.content_length), (200, 0));
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
