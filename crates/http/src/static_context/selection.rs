//! Directory/index policy; Native owns each scoped filesystem effect and configuration view.
use super::{HttpIndexPage, HttpStaticContext, INDEX, INVALID, bytes, swr_v2_http_context_page};
const DIRECTORY_KIND: u32 = 16;

#[derive(Default)]
pub(super) enum Scan {
    #[default]
    Idle,
    Open,
    Read,
    Close,
}
#[derive(Default)]
pub(super) enum Index {
    #[default]
    Idle,
    Scan,
    Candidate {
        next: usize,
        count: usize,
    },
    Auto,
    Filename,
}

#[derive(Default)]
pub(super) enum Page {
    #[default]
    Idle,
    Header,
    Entries {
        next: usize,
        count: usize,
    },
    Footer,
}

/// Begin publishes the owned directory slash and requests header1. Resume then
/// requests entry2(index), footer3 and done0. Count is captured after the Native
/// header append, matching the original observation boundary.
/// # Safety
/// Unique live context and disjoint writable index. Native buffer effects occur
/// after return; no Rust directory borrow spans a reserve/append.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_page_step(
    owner: *mut HttpStaticContext,
    begin: u32,
    index: *mut usize,
) -> i32 {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let Some(owner) = (unsafe { owner.as_mut() }) else {
        return INVALID;
    };
    // SAFETY: `index` is null or live, aligned and exclusively lent for this call.
    let Some(index) = (unsafe { index.as_mut() }) else {
        return INVALID;
    };
    if begin != 0 {
        let result = owner.ensure_directory_slash();
        if result < 0 {
            return result;
        }
        owner.page = Page::Header;
        *index = 0;
        return 1;
    }
    let (next, count) = match owner.page {
        Page::Header => (0, owner.directory.len()),
        Page::Entries { next, count } => (next, count),
        Page::Footer => {
            owner.page = Page::Idle;
            return 0;
        }
        Page::Idle => return INVALID,
    };
    if next < count {
        *index = next;
        owner.page = Page::Entries {
            next: next + 1,
            count,
        };
        2
    } else {
        *index = 0;
        owner.page = Page::Footer;
        3
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HttpContextPageBuffer {
    pub required: usize,
    pub length: usize,
}

/// Render a page part, or request Native capacity before any write. Actions:
/// skip0, reserve1(required), extend2(required), written3(length). Re-observe the
/// Native buffer after reserve/extend; no pointer is retained between calls.
/// # Safety
/// Live unchanged context, independent initialized input spans, writable buffer
/// for capacity bytes, length<=capacity, and disjoint initialized output. Only
/// rendered bytes/terminator are written; unused backing need not be initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_page_emit(
    owner: *const HttpStaticContext,
    entry: usize,
    input: *const HttpIndexPage,
    buffer: *mut u8,
    length: usize,
    capacity: usize,
    output: *mut HttpContextPageBuffer,
) -> i32 {
    let render = |data, size, written| {
        // SAFETY: `owner` and `input` follow the export's contract; emit_page passes a
        // writable span.
        unsafe { swr_v2_http_context_page(owner, entry, input, data, size, written) }
    };
    // SAFETY: forwards the caller's input, buffer and output under emit_page's contract.
    unsafe { crate::index_page::emit_page(input, None, buffer, length, capacity, output, render) }
}

/// Events: begin0, opened1, open-failed2, entry3, end4, closed5.
/// Actions: reject0, accept1, open2, read3, close4. No DIR pointer enters Rust.
/// # Safety
/// Live unique owner; entry bytes are initialized/disjoint and valid for this call only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_scan(
    owner: *mut HttpStaticContext,
    event: u32,
    name: *const u8,
    length: usize,
) -> i32 {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let Some(owner) = (unsafe { owner.as_mut() }) else {
        return INVALID;
    };
    match (&owner.scan, event) {
        (_, 0) => {
            owner.scan = Scan::Idle;
            if !owner.directory.is_empty() {
                1
            } else if owner.facts.file_ready == 0 || owner.facts.kind & DIRECTORY_KIND == 0 {
                0
            } else {
                owner.scan = Scan::Open;
                2
            }
        }
        (Scan::Open, 1) => {
            owner.scan = Scan::Read;
            3
        }
        (Scan::Open, 2) => {
            owner.scan = Scan::Idle;
            0
        }
        (Scan::Read, 3) => {
            // SAFETY: the caller lends `length` initialized bytes at `name` (null only when empty).
            let name = match unsafe { bytes(name, length) } {
                Ok(name) => name,
                Err(error) => return error,
            };
            match owner.directory.scan_append(name) {
                Ok(()) => 3,
                Err(error) => error,
            }
        }
        (Scan::Read, 4) => {
            owner.scan = Scan::Close;
            4
        }
        (Scan::Close, 5) => {
            owner.directory.scan_finish();
            owner.scan = Scan::Idle;
            1
        }
        _ => INVALID,
    }
}

/// Events: begin0(configured count), scanned1(fresh count), candidate2(name), autoindex3(enabled),
/// filename4(success), scan-failed6.
/// Actions: reject0, accept1, scan2, candidate3(cursor), filename4, autoindex5.
/// The candidate cursor and first-match/empty-name decisions belong to Rust.
/// # Safety
/// Live unique owner, initialized disjoint candidate bytes and writable cursor. Configuration
/// strings remain Native-owned and are freshly observed after each complete Rust call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_index(
    owner: *mut HttpStaticContext,
    event: u32,
    value: usize,
    name: *const u8,
    length: usize,
    cursor: *mut usize,
) -> i32 {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let Some(owner) = (unsafe { owner.as_mut() }) else {
        return INVALID;
    };
    // SAFETY: `cursor` is null or live, aligned and exclusively lent for this call.
    let Some(cursor) = (unsafe { cursor.as_mut() }) else {
        return INVALID;
    };
    match (&owner.index_phase, event) {
        (_, 0) => {
            owner.index_phase = Index::Idle;
            if value == 0 || owner.facts.file_ready == 0 || owner.facts.kind & DIRECTORY_KIND == 0 {
                1
            } else {
                owner.index_phase = Index::Scan;
                2
            }
        }
        (Index::Scan, 6) => {
            owner.index_phase = Index::Idle;
            0
        }
        (Index::Scan, 1) if value == 0 => {
            owner.index.clear();
            owner.index_phase = Index::Auto;
            5
        }
        (Index::Scan, 1) => {
            let count = value;
            *cursor = 0;
            owner.index_phase = Index::Candidate { next: 0, count };
            3
        }
        (Index::Candidate { next, count }, 2) => {
            let (next, count) = (*next, *count);
            // SAFETY: the caller lends `length` initialized bytes at `name` (null only when empty).
            let name = match unsafe { bytes(name, length) } {
                Ok(name) => name,
                Err(error) => return error,
            };
            let found = match owner.directory.contains(name) {
                Ok(found) => found,
                Err(error) => return error,
            };
            if found {
                let result = owner.publish(INDEX, name);
                if result < 0 {
                    return result;
                }
                if !owner.index.is_empty() {
                    owner.index_phase = Index::Filename;
                    return 4;
                }
            } else if next + 1 < count {
                *cursor = next + 1;
                owner.index_phase = Index::Candidate {
                    next: next + 1,
                    count,
                };
                return 3;
            } else {
                owner.index.clear();
            }
            owner.index_phase = Index::Auto;
            5
        }
        (Index::Auto, 3) | (Index::Filename, 4) => {
            owner.index_phase = Index::Idle;
            i32::from(value != 0)
        }
        _ => INVALID,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HttpProxyText, swr_v2_http_context_drop, swr_v2_http_context_new};
    use std::ptr;

    struct Context(*mut HttpStaticContext);
    impl Context {
        fn directory() -> Self {
            let owner = unsafe { swr_v2_http_context_new(b"/files".as_ptr(), 6, 4096) };
            assert!(!owner.is_null());
            unsafe {
                (*owner).facts.file_ready = 1;
                (*owner).facts.kind = DIRECTORY_KIND;
            }
            Self(owner)
        }
        fn scan(&self, event: u32, name: &[u8]) -> i32 {
            unsafe { swr_v2_http_context_scan(self.0, event, name.as_ptr(), name.len()) }
        }
        fn index(&self, event: u32, value: usize, name: &[u8], cursor: &mut usize) -> i32 {
            unsafe {
                swr_v2_http_context_index(self.0, event, value, name.as_ptr(), name.len(), cursor)
            }
        }
        fn load(&self, names: &[&[u8]]) {
            assert_eq!(self.scan(0, b""), 2);
            assert_eq!(self.scan(1, b""), 3);
            for name in names {
                assert_eq!(self.scan(3, name), 3);
            }
            assert_eq!(self.scan(4, b""), 4);
            assert_eq!(self.scan(5, b""), 1);
        }
    }
    impl Drop for Context {
        fn drop(&mut self) {
            unsafe { swr_v2_http_context_drop(self.0) }
        }
    }

    #[test]
    fn directory_scan_reuses_owned_sorted_unique_names_after_close() {
        let context = Context::directory();
        context.load(&[b"z", b"index.html", b"z", b"a"]);
        assert_eq!(unsafe { (*context.0).directory.len() }, 3);
        assert_eq!(context.scan(0, b""), 1);
        let mut cursor = usize::MAX;
        assert_eq!(context.index(0, 3, b"", &mut cursor), 2);
        assert_eq!(context.index(1, 3, b"", &mut cursor), 3);
        assert_eq!(cursor, 0);
        assert_eq!(context.index(2, 0, b"missing", &mut cursor), 3);
        assert_eq!(cursor, 1);
        assert_eq!(context.index(2, 0, b"z", &mut cursor), 4);
        assert_eq!(unsafe { &(*context.0).index }, b"z");
        assert_eq!(context.index(4, 1, b"", &mut cursor), 1);
    }

    /// Upstream `TEST(base, intersection)`: the first configured index file the directory
    /// holds, in configured order, or none. This loop does `swoole::intersection`'s job.
    #[test]
    fn base_intersection() {
        fn intersection(candidates: &[&[u8]], names: &[&[u8]]) -> Vec<u8> {
            let context = Context::directory();
            context.load(names);
            let mut cursor = 0;
            assert_eq!(context.index(0, candidates.len(), b"", &mut cursor), 2);
            let mut step = context.index(1, candidates.len(), b"", &mut cursor);
            while step == 3 {
                step = context.index(2, 0, candidates[cursor], &mut cursor);
            }
            assert!(step == 4 || step == 5);
            unsafe { (*context.0).index.clone() }
        }
        let vec1: &[&[u8]] = &[b"index.php", b"index.html", b"default.html"];
        let vec2: &[&[u8]] = &[
            b".",
            b"..",
            b"default.html",
            b"index.php",
            b"test.html",
            b"a.json",
            b"index.php",
        ];
        assert_eq!(intersection(vec1, vec2), b"index.php");
        let vec3: &[&[u8]] = &["a".as_bytes(), "zh中".as_bytes(), "、r\n".as_bytes()];
        assert_eq!(intersection(vec1, vec3), b"");
    }

    #[test]
    fn no_match_clears_old_index_before_fresh_autoindex_observation() {
        let context = Context::directory();
        context.load(&[b"other"]);
        unsafe {
            (*context.0).index = b"stale".to_vec();
        }
        let mut cursor = 0;
        assert_eq!(context.index(0, 2, b"", &mut cursor), 2);
        // Configured names are observed freshly after the Native scan returns.
        assert_eq!(context.index(1, 1, b"", &mut cursor), 3);
        assert_eq!(context.index(2, 0, b"absent", &mut cursor), 5);
        assert!(unsafe { (*context.0).index.is_empty() });
        assert_eq!(context.index(3, 1, b"", &mut cursor), 1);
    }

    #[test]
    fn matching_empty_name_stops_before_later_candidate_and_observes_autoindex() {
        let context = Context::directory();
        context.load(&[b"", b"index.html"]);
        let mut cursor = 0;
        assert_eq!(context.index(0, 2, b"", &mut cursor), 2);
        assert_eq!(context.index(1, 2, b"", &mut cursor), 3);
        assert_eq!(context.index(2, 0, b"", &mut cursor), 5);
        assert_eq!(context.index(3, 0, b"", &mut cursor), 0);
        assert!(unsafe { (*context.0).index.is_empty() });
    }

    #[test]
    fn empty_scan_reopens_but_unconfigured_index_does_not_change_existing_name() {
        let context = Context::directory();
        context.load(&[]);
        assert_eq!(context.scan(0, b""), 2);
        unsafe {
            (*context.0).index = b"existing".to_vec();
        }
        let mut cursor = 0;
        assert_eq!(context.index(0, 0, b"", &mut cursor), 1);
        assert_eq!(unsafe { &(*context.0).index }, b"existing");
        assert_eq!(
            unsafe { swr_v2_http_context_index(context.0, 0, 0, ptr::null(), 0, &raw mut cursor) },
            1
        );
    }

    #[test]
    fn page_sequence_preserves_header_entries_footer_and_repeated_slash() {
        for names in [
            &[][..],
            &[b"b".as_slice(), b"a".as_slice(), b"a".as_slice()][..],
        ] {
            let context = Context::directory();
            context.load(names);
            unsafe {
                (*context.0).directory_path = b"/normal".to_vec();
            }
            for _ in 0..2 {
                let mut index = usize::MAX;
                assert_eq!(
                    unsafe { swr_v2_http_context_page_step(context.0, 1, &raw mut index) },
                    1
                );
                assert_eq!(index, 0);
                let count = usize::from(!names.is_empty()) * 2;
                for expected in 0..count {
                    assert_eq!(
                        unsafe { swr_v2_http_context_page_step(context.0, 0, &raw mut index) },
                        2
                    );
                    assert_eq!(index, expected);
                }
                assert_eq!(
                    unsafe { swr_v2_http_context_page_step(context.0, 0, &raw mut index) },
                    3
                );
                assert_eq!(index, 0);
                assert_eq!(
                    unsafe { swr_v2_http_context_page_step(context.0, 0, &raw mut index) },
                    0
                );
                assert_eq!(unsafe { &(*context.0).directory_path }, b"/normal/");
            }
        }
    }

    #[test]
    fn page_emit_requests_native_capacity_then_writes_bytes_and_terminator() {
        let context = Context::directory();
        context.load(&[b"a.txt"]);
        unsafe {
            (*context.0).directory_path = b"/normal/".to_vec();
        }
        let empty = HttpProxyText {
            data: ptr::null(),
            length: 0,
        };
        for part in 0..3 {
            let input = HttpIndexPage {
                path: empty,
                entry: empty,
                css: HttpProxyText {
                    data: b"css".as_ptr(),
                    length: 3,
                },
                powered: HttpProxyText {
                    data: b"powered".as_ptr(),
                    length: 7,
                },
                part,
            };
            let mut count = 0;
            assert_eq!(
                unsafe {
                    swr_v2_http_context_page(
                        context.0,
                        0,
                        &raw const input,
                        ptr::null_mut(),
                        0,
                        &raw mut count,
                    )
                },
                0
            );
            assert!(count > 0);
            let mut expected = vec![0; count];
            assert_eq!(
                unsafe {
                    swr_v2_http_context_page(
                        context.0,
                        0,
                        &raw const input,
                        expected.as_mut_ptr(),
                        count,
                        &raw mut count,
                    )
                },
                0
            );
            let mut buffer = b"prefix".to_vec();
            let mut result = HttpContextPageBuffer::default();
            assert_eq!(
                unsafe {
                    swr_v2_http_context_page_emit(
                        context.0,
                        0,
                        &raw const input,
                        buffer.as_mut_ptr(),
                        6,
                        6,
                        &raw mut result,
                    )
                },
                if part == 2 { 1 } else { 2 }
            );
            let terminator = usize::from(part != 2);
            assert_eq!(result.required, 6 + count + terminator);
            assert_eq!(buffer, b"prefix");
            buffer.resize(result.required + 17, 0xa5);
            let capacity = buffer.len();
            assert_eq!(
                unsafe {
                    swr_v2_http_context_page_emit(
                        context.0,
                        0,
                        &raw const input,
                        buffer.as_mut_ptr(),
                        6,
                        capacity,
                        &raw mut result,
                    )
                },
                3
            );
            assert_eq!(result.length, 6 + count);
            assert_eq!(&buffer[..6], b"prefix");
            assert_eq!(&buffer[6..result.length], expected);
            if terminator != 0 {
                assert_eq!(buffer[result.length], 0);
            }
            assert_eq!(&buffer[result.required..], &[0xa5; 17]);
        }
    }

    #[test]
    fn filtered_page_entry_does_not_request_growth_or_publish_output() {
        let context = Context::directory();
        context.load(&[b"."]);
        let empty = HttpProxyText {
            data: ptr::null(),
            length: 0,
        };
        let input = HttpIndexPage {
            path: empty,
            entry: empty,
            css: empty,
            powered: empty,
            part: 1,
        };
        let mut buffer = b"prefix".to_vec();
        let mut result = HttpContextPageBuffer::default();
        assert_eq!(
            unsafe {
                swr_v2_http_context_page_emit(
                    context.0,
                    0,
                    &raw const input,
                    buffer.as_mut_ptr(),
                    6,
                    6,
                    &raw mut result,
                )
            },
            0
        );
        assert_eq!((result.required, result.length), (0, 0));
        assert_eq!(buffer, b"prefix");
    }
}
