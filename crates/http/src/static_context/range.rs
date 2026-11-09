//! Complete selected range orchestration. Native executes requested MIME/RNG/
//! calendar effects after return; Rust retains only owned values and scalar cursors.
use super::{HttpStaticContext, swr_v2_http_context_format};
use crate::{
    HttpProxyText, HttpRangeFinish, HttpRangeFormat, HttpRangeState, HttpSpan, INVALID, OVERFLOW,
    bytes,
};

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
#[repr(C)]
pub struct HttpContextRangeObservation {
    pub mime: HttpProxyText,
    pub boundary_ready: u32,
    pub modified: u32,
}
fn finishing(owner: &mut HttpStaticContext) {
    owner.range.finish = HttpRangeFinish {
        content_length: owner.facts.content_length,
        last_length: owner.range.last_length,
        task_count: owner.tasks.count(),
        filesize: owner.facts.filesize,
        status: owner.facts.status,
        if_range: owner.range.if_range,
        ..HttpRangeFinish::default()
    };
    owner.range.stage = Stage::Finish;
}
fn full_task(owner: &mut HttpStaticContext) -> i32 {
    owner.tasks.prepare_full(0, owner.range.last_length);
    owner.tasks.push()
}
/// Begin one method invocation; previous committed tasks persist, scratch resets.
/// # Safety
/// Unique live context. Length is the initialized Native C-prefix span re-observed
/// by each step, never a pointer retained in Rust. Native effects start after return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_range_begin(
    owner: *mut HttpStaticContext,
    length: usize,
    if_range: u32,
) -> i32 {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let Some(owner) = (unsafe { owner.as_mut() }) else {
        return INVALID;
    };
    if length > isize::MAX as usize {
        return OVERFLOW;
    }
    owner.range = HttpContextRange {
        length,
        if_range,
        ..HttpContextRange::default()
    };
    owner.tasks.begin();
    if length != 0 {
        owner.facts.content_length = 0;
        owner.range.stage = Stage::Parsing;
    } else {
        finishing(owner);
    }
    0
}
/// Resume all pure parsing/accounting/commit work until Native work is needed:
/// done0, header MIME/RNG1, final-boundary RNG2, If-Range date3. Negative results
/// stop the invocation; Native throws only after Rust returns. Header accounting/
/// push waits for the call after Native has applied the returned errno effect.
/// # Safety
/// Unique live context; Native range bytes are initialized/disjoint and unchanged
/// for the invocation, with the same length as begin. No input pointer is retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_range_step(
    owner: *mut HttpStaticContext,
    text: *const u8,
    length: usize,
) -> i32 {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let Some(owner) = (unsafe { owner.as_mut() }) else {
        return INVALID;
    };
    if length != owner.range.length {
        return INVALID;
    }
    // Validate the current Native span even when this call will only finish.
    // SAFETY: the caller lends `length` initialized bytes at `text` (null only when empty).
    let input = match unsafe { bytes(text, length) } {
        Ok(input) => input,
        Err(error) => return error,
    };
    loop {
        match owner.range.stage {
            Stage::Parsing => {
                let result =
                    owner
                        .range
                        .cursor
                        .step(input, owner.facts.filesize, &mut owner.range.part);
                if result < 0 {
                    owner.facts.status = 416;
                    owner.range.stage = Stage::Done;
                    return 0;
                }
                match result {
                    0 => finishing(owner),
                    2 => {
                        owner.range.last_length = owner.facts.filesize;
                        owner.facts.content_length = owner.facts.filesize;
                        let result = full_task(owner);
                        owner.range.stage = Stage::Done;
                        return result;
                    }
                    _ => {
                        owner.range.last_length = owner.range.part.length;
                        owner.range.stage = Stage::Header;
                        return 1;
                    }
                }
            }
            Stage::CommitHeader => {
                owner.facts.content_length = owner
                    .facts
                    .content_length
                    .wrapping_add(owner.range.written)
                    .wrapping_add(owner.range.last_length);
                let result = owner.tasks.push();
                if result < 0 {
                    owner.range.stage = Stage::Done;
                    return result;
                }
                owner.range.stage = Stage::Parsing;
            }
            Stage::Finish => {
                let action = owner.range.finish.step();
                owner.facts.content_length = owner.range.finish.content_length;
                owner.facts.status = owner.range.finish.status;
                match action {
                    0 => {
                        owner.range.stage = Stage::Done;
                        return 0;
                    }
                    1 => {
                        owner.range.last_length = owner.range.finish.filesize;
                        let result = full_task(owner);
                        if result < 0 {
                            owner.range.stage = Stage::Done;
                            return result;
                        }
                    }
                    2 => {
                        owner.range.stage = Stage::End;
                        return 2;
                    }
                    3 => {
                        owner.range.stage = Stage::Date;
                        return 3;
                    }
                    4 => {
                        owner.tasks.clear();
                    }
                    _ => return INVALID,
                }
            }
            Stage::Done => return 0,
            _ => return INVALID,
        }
    }
}
/// Admit a completed Native effect; no Native object is retained. Header returns1
/// only for the existing EOVERFLOW signal; Native applies errno before next step.
/// # Safety
/// Unique live context and initialized observation/mime span disjoint from owner.
/// Native RNG/MIME/calendar effects have finished before this call starts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_range_resume(
    owner: *mut HttpStaticContext,
    observed: *const HttpContextRangeObservation,
) -> i32 {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let owner = unsafe { owner.as_mut() };
    // SAFETY: `observed` is null or live and aligned for this call.
    let observed = unsafe { observed.as_ref() };
    let (Some(owner), Some(observed)) = (owner, observed) else {
        return INVALID;
    };
    match owner.range.stage {
        Stage::Header => {
            if observed.boundary_ready == 0 {
                return INVALID;
            }
            let format = HttpRangeFormat {
                boundary: HttpProxyText {
                    data: owner.boundary.as_ptr(),
                    length: owner.boundary.len(),
                },
                mime: HttpProxyText {
                    data: observed.mime.data,
                    length: observed.mime.length,
                },
                offset: owner.range.part.offset,
                length: owner.range.part.length,
                filesize: owner.facts.filesize,
                part: 0,
                separator: u32::from(owner.tasks.count() != 0),
            };
            // SAFETY: `format`'s spans are this owner's boundary and the caller's MIME.
            let result = unsafe {
                owner.tasks.prepare_range(
                    i64::try_from(owner.range.part.offset).unwrap_or(i64::MAX),
                    owner.range.part.length,
                    &format,
                    &mut owner.range.written,
                )
            };
            if result >= 0 {
                owner.range.stage = Stage::CommitHeader;
            }
            result
        }
        Stage::End => {
            if observed.boundary_ready == 0 {
                return INVALID;
            }
            // SAFETY: `owner` is this live, exclusive context.
            let result = unsafe { swr_v2_http_context_format(owner, 1) };
            if result < 0 {
                return result;
            }
            owner.range.finish.end_length = owner.end.len();
            owner.range.stage = Stage::Finish;
            0
        }
        Stage::Date => {
            owner.range.finish.modified = observed.modified;
            owner.range.stage = Stage::Finish;
            0
        }
        _ => INVALID,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HttpStaticTask;
    use crate::static_context::{
        HttpStaticObservation, swr_v2_http_context_drop, swr_v2_http_context_new,
        swr_v2_http_context_observed,
    };
    fn owner() -> *mut HttpStaticContext {
        let owner = unsafe { swr_v2_http_context_new(b"/file".as_ptr(), 5, 128) };
        assert!(!owner.is_null());
        let file = HttpStaticObservation {
            filesize: 10,
            mtime: 1_700_000_000,
            kind: 32,
            ..HttpStaticObservation::default()
        };
        assert_eq!(
            unsafe { swr_v2_http_context_observed(owner, 0, &raw const file) },
            0
        );
        assert_eq!(unsafe { &mut *owner }.publish(5, b"boundary"), 0);
        owner
    }
    fn observed(modified: u32) -> HttpContextRangeObservation {
        HttpContextRangeObservation {
            mime: HttpProxyText {
                data: b"text/plain".as_ptr(),
                length: 10,
            },
            boundary_ready: 1,
            modified,
        }
    }
    fn complete(owner: *mut HttpStaticContext, text: &[u8], modified: u32) {
        assert_eq!(
            unsafe { swr_v2_http_context_range_begin(owner, text.len(), 1) },
            0
        );
        loop {
            match unsafe { swr_v2_http_context_range_step(owner, text.as_ptr(), text.len()) } {
                0 => break,
                1..=3 => assert_eq!(
                    unsafe { swr_v2_http_context_range_resume(owner, &observed(modified)) },
                    0
                ),
                action => panic!("unexpected range action {action}"),
            }
        }
    }
    #[test]
    fn ordinary_empty_file_has_one_initialized_zero_length_task() {
        let owner = owner();
        unsafe { (*owner).facts.filesize = 0 };
        complete(owner, b"bytes=0-", 0);
        assert_eq!(unsafe { (*owner).tasks.count() }, 1);
        assert_eq!(unsafe { (*owner).facts.content_length }, 0);
        assert_eq!(unsafe { (*owner).facts.status }, 200);
        assert_eq!(
            unsafe { (*owner).tasks.items() },
            [HttpStaticTask::default()]
        );
        unsafe { swr_v2_http_context_drop(owner) };
    }
    #[test]
    fn header_accounting_waits_for_native_errno_boundary_then_commits() {
        let owner = owner();
        let text = b"bytes=1-3,5-6";
        assert_eq!(
            unsafe { swr_v2_http_context_range_begin(owner, text.len(), 0) },
            0
        );
        assert_eq!(
            unsafe { swr_v2_http_context_range_step(owner, text.as_ptr(), text.len()) },
            1
        );
        assert_eq!(
            unsafe { swr_v2_http_context_range_resume(owner, &observed(0)) },
            0
        );
        assert_eq!(unsafe { (*owner).facts.content_length }, 0);
        assert_eq!(unsafe { (*owner).tasks.count() }, 0);
        // Native applies any errno effect before this next call can commit.
        assert_eq!(
            unsafe { swr_v2_http_context_range_step(owner, text.as_ptr(), text.len()) },
            1
        );
        assert_eq!(unsafe { (*owner).tasks.count() }, 1);
        assert!(unsafe { (*owner).facts.content_length } > 3);
        assert_eq!(
            unsafe { swr_v2_http_context_range_resume(owner, &observed(0)) },
            0
        );
        assert_eq!(
            unsafe { swr_v2_http_context_range_step(owner, text.as_ptr(), text.len()) },
            2
        );
        assert_eq!(
            unsafe { swr_v2_http_context_range_resume(owner, &observed(0)) },
            0
        );
        assert_eq!(
            unsafe { swr_v2_http_context_range_step(owner, text.as_ptr(), text.len()) },
            0
        );
        assert_eq!(unsafe { (*owner).facts.status }, 206);
        let tasks: [HttpStaticTask; 2] = unsafe { (*owner).tasks.items() }.try_into().unwrap();
        assert_eq!(
            (
                tasks[0].offset,
                tasks[0].length,
                tasks[1].offset,
                tasks[1].length
            ),
            (1, 3, 5, 2)
        );
        assert_eq!(unsafe { &(*owner).end }, b"\r\n--boundary--\r\n");
        unsafe { swr_v2_http_context_drop(owner) };
        assert!(tasks[0].part_header.starts_with(b"--boundary"));
    }
    #[test]
    fn date_replacement_and_repeated_empty_parse_preserve_existing_records() {
        let owner = owner();
        complete(owner, b"bytes=1-3,5-6", 1);
        assert_eq!(unsafe { (*owner).facts.status }, 200);
        assert_eq!(unsafe { (*owner).facts.content_length }, 10);
        assert_eq!(unsafe { (*owner).tasks.count() }, 1);
        complete(owner, b"", 0);
        assert_eq!(unsafe { (*owner).tasks.count() }, 2);
        assert_eq!(unsafe { (*owner).facts.content_length }, 10);
        assert_eq!(unsafe { (*owner).facts.status }, 200);
        unsafe { swr_v2_http_context_drop(owner) };
    }
    #[test]
    fn wrong_stage_and_changed_span_metadata_do_not_consume_native_effects() {
        let owner = owner();
        let text = b"bytes=0-0";
        assert_eq!(
            unsafe { swr_v2_http_context_range_resume(owner, &observed(0)) },
            INVALID
        );
        assert_eq!(
            unsafe { swr_v2_http_context_range_begin(owner, text.len(), 0) },
            0
        );
        assert_eq!(
            unsafe { swr_v2_http_context_range_step(owner, text.as_ptr(), text.len() - 1) },
            INVALID
        );
        assert_eq!(unsafe { (*owner).tasks.count() }, 0);
        assert_eq!(
            unsafe { swr_v2_http_context_range_step(owner, text.as_ptr(), text.len()) },
            1
        );
        unsafe { swr_v2_http_context_drop(owner) };
    }
}
