//! Sole selected static-handler context. Native effects see independent copied
//! observations; no Rust-owned text or container pointer crosses the boundary.
use crate::{
    HttpDirectory, HttpIndexPage, HttpProxyText, HttpRangeFormat, HttpStaticPath, HttpStaticTask,
    HttpTasks, INVALID, OVERFLOW, bytes, swr_v1_http_range_format, swr_v1_http_static_catch,
    swr_v1_http_static_content_type, swr_v1_http_static_index_path, swr_v1_http_static_location,
    swr_v1_http_static_path, swr_v2_http_directory_page, swr_v2_http_tasks_begin,
    swr_v2_http_tasks_clear, swr_v2_http_tasks_count, swr_v2_http_tasks_prepare,
    swr_v2_http_tasks_push, swr_v2_http_tasks_snapshot,
};
use std::alloc::{Layout, alloc};
mod filesystem;
pub use filesystem::{
    HttpStaticIo, swr_v3_http_context_directory, swr_v3_http_context_serve,
    swr_v3_http_context_stat,
};
mod range;
mod selection;
mod serve;
pub use range::{
    HttpContextRangeObservation, swr_v2_http_context_range_begin, swr_v2_http_context_range_resume,
    swr_v2_http_context_range_step,
};
pub use selection::{
    HttpContextPageBuffer, swr_v2_http_context_index, swr_v2_http_context_page_emit,
    swr_v2_http_context_page_step, swr_v2_http_context_scan,
};
pub use serve::{HttpContextServeObservation, swr_v2_http_context_serve};

const ALLOCATION: i32 = -3;
const URL: u32 = 0;
const ORIGINAL: u32 = 1;
const DIRECTORY: u32 = 2;
const INDEX: u32 = 3;
const FILENAME: u32 = 4;
const BOUNDARY: u32 = 5;
const CONTENT: u32 = 6;
const END: u32 = 7;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct HttpStaticObservation {
    pub filesize: usize,
    pub mtime: i64,
    pub content_length: usize,
    pub directory_count: usize,
    pub task_count: usize,
    pub status: i32,
    pub kind: u32,
    pub file_ready: u32,
    pub last: u32,
}
pub struct HttpStaticContext {
    original: Vec<u8>,
    rewritten: Option<Vec<u8>>,
    directory_path: Vec<u8>,
    index: Vec<u8>,
    filename: Vec<u8>,
    boundary: Vec<u8>,
    content: Vec<u8>,
    end: Vec<u8>,
    path: HttpStaticPath,
    range: range::HttpContextRange,
    scan: selection::Scan,
    index_phase: selection::Index,
    page: selection::Page,
    serve: serve::Serve,
    facts: HttpStaticObservation,
    directory: HttpDirectory,
    tasks: HttpTasks,
}
fn copied(input: &[u8]) -> Result<Vec<u8>, i32> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(input.len())
        .map_err(|_| ALLOCATION)?;
    result.extend_from_slice(input);
    Ok(result)
}
impl HttpStaticContext {
    /// Upstream's `apply_rewrite_rules`: rewrite the current URL by the first rule that
    /// applies. True when one did.
    pub fn rewrite(&mut self, rules: &crate::RewriteRules) -> bool {
        let rewritten = rules.apply(self.url());
        let changed = rewritten.is_some();
        if changed {
            self.rewritten = rewritten;
        }
        changed
    }
    fn path_step(&mut self, root: &[u8], facts: u32) -> i32 {
        let url = self.rewritten.as_ref().unwrap_or(&self.original);
        // SAFETY: all spans are this owner's live buffers, disjoint from each other and from
        // `self.path`; `filename` is writable through its length.
        let result = unsafe {
            swr_v1_http_static_path(
                url.as_ptr(),
                url.len(),
                root.as_ptr(),
                root.len(),
                self.filename.as_mut_ptr(),
                self.filename.len(),
                &raw mut self.path,
                facts,
            )
        };
        if result == 3 {
            let Some(url) = url.get(..self.path.url_length) else {
                return INVALID;
            };
            let value = match copied(url) {
                Ok(value) => value,
                Err(error) => return error,
            };
            self.directory_path = value;
        }
        result
    }
    fn ensure_directory_slash(&mut self) -> i32 {
        if self.directory_path.last() != Some(&b'/') {
            if self.directory_path.try_reserve(1).is_err() {
                return ALLOCATION;
            }
            self.directory_path.push(b'/');
        }
        0
    }
    fn url(&self) -> &[u8] {
        self.rewritten.as_deref().unwrap_or(&self.original)
    }
    fn text(&self, kind: u32) -> Option<&[u8]> {
        Some(match kind {
            URL => self.url(),
            ORIGINAL => &self.original,
            DIRECTORY => &self.directory_path,
            INDEX => &self.index,
            FILENAME => crate::c_text(&self.filename),
            BOUNDARY => &self.boundary,
            CONTENT => &self.content,
            END => &self.end,
            _ => return None,
        })
    }
    fn publish(&mut self, kind: u32, text: &[u8]) -> i32 {
        if kind == FILENAME {
            let Some((nul, head)) = self
                .filename
                .get_mut(..=text.len())
                .and_then(|slots| slots.split_last_mut())
            else {
                return OVERFLOW;
            };
            head.copy_from_slice(text);
            *nul = 0;
            self.path.filename_length = text.len();
            return 0;
        }
        let target = match kind {
            URL => {
                let value = match copied(text) {
                    Ok(value) => value,
                    Err(error) => return error,
                };
                self.rewritten = Some(value);
                return 0;
            }
            DIRECTORY => &mut self.directory_path,
            INDEX => &mut self.index,
            BOUNDARY => &mut self.boundary,

            _ => return INVALID,
        };
        let value = match copied(text) {
            Ok(value) => value,
            Err(error) => return error,
        };
        *target = value;
        0
    }
}
/// Stable borrowed modification-time slot, valid until the context is dropped.
/// # Safety
/// Owner is null or live. Callers read the slot only under the context's existing synchronization.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v3_http_context_mtime(owner: *const HttpStaticContext) -> *const i64 {
    if owner.is_null() {
        return core::ptr::null();
    }
    // SAFETY: the live owner retains its facts at a stable address until drop.
    unsafe { &raw const (*owner).facts.mtime }
}

/// # Safety
/// Input URL initialized/disjoint; capacity is the selected native `PATH_MAX`.
/// Returned context has unique custody and must be destroyed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_new(
    url: *const u8,
    length: usize,
    capacity: usize,
) -> *mut HttpStaticContext {
    // SAFETY: the caller lends `length` initialized bytes at `url` (null only when empty).
    let Ok(input) = (unsafe { bytes(url, length) }) else {
        return std::ptr::null_mut();
    };
    if capacity == 0 || capacity > isize::MAX as usize {
        return std::ptr::null_mut();
    }
    let Ok(original) = copied(input) else {
        return std::ptr::null_mut();
    };
    let mut filename = Vec::new();
    if filename.try_reserve_exact(capacity).is_err() {
        return std::ptr::null_mut();
    }
    filename.resize(capacity, 0);
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "alloc honours Layout::new::<HttpStaticContext>()"
    )]
    // SAFETY: a non-zero-sized layout; null is handled below.
    let context = unsafe { alloc(Layout::new::<HttpStaticContext>()).cast::<HttpStaticContext>() };
    if !context.is_null() {
        // SAFETY: `context` is non-null (checked above), aligned and writable.
        unsafe {
            context.write(HttpStaticContext {
                original,
                rewritten: None,
                directory_path: Vec::new(),
                index: Vec::new(),
                filename,
                boundary: Vec::new(),
                content: Vec::new(),
                end: Vec::new(),
                path: HttpStaticPath::default(),
                range: range::HttpContextRange::default(),
                scan: selection::Scan::default(),
                index_phase: selection::Index::default(),
                page: selection::Page::default(),
                serve: serve::Serve::default(),
                facts: HttpStaticObservation {
                    status: 200,
                    ..HttpStaticObservation::default()
                },
                directory: HttpDirectory::default(),
                tasks: HttpTasks::default(),
            });
        };
    }
    context
}
/// # Safety
/// Null or a unique live context, destroyed once; no native resource in Rust.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_drop(owner: *mut HttpStaticContext) {
    if !owner.is_null() {
        // SAFETY: a unique, live context from `context_new`, destroyed once.
        unsafe { drop(Box::from_raw(owner)) };
    }
}
/// Query/fill copied bytes (without NUL). Invalid/short capacity publishes nothing.
/// # Safety
/// Live unchanged context; output/written initialized writable and disjoint from
/// each other, context and input. No Rust pointer escapes this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_text(
    owner: *const HttpStaticContext,
    kind: u32,
    output: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> i32 {
    // SAFETY: `owner` is null or live and aligned for this call.
    let Some(owner) = (unsafe { owner.as_ref() }) else {
        return INVALID;
    };
    if written.is_null() {
        return INVALID;
    }
    let Some(text) = owner.text(kind) else {
        return INVALID;
    };
    if capacity != 0 {
        if capacity < text.len() || capacity > isize::MAX as usize {
            return OVERFLOW;
        }
        if output.is_null() {
            return INVALID;
        }
        // SAFETY: non-null, `text.len() <= capacity <= isize::MAX` writable bytes, disjoint from the owner.
        unsafe { std::ptr::copy_nonoverlapping(text.as_ptr(), output, text.len()) };
    }
    // SAFETY: `written` is non-null (checked above), aligned and writable per the caller contract.
    unsafe { written.write(text.len()) };
    0
}
/// Admit a copied observation at the original native publication boundary.
/// FILENAME means successful canonical result; index appending is separate and
/// deliberately preserves the logical/base filename length.
/// # Safety
/// Live unique context and initialized input span disjoint from its storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_publish(
    owner: *mut HttpStaticContext,
    kind: u32,
    input: *const u8,
    length: usize,
) -> i32 {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let Some(owner) = (unsafe { owner.as_mut() }) else {
        return INVALID;
    };
    // SAFETY: the caller lends `length` initialized bytes at `input` (null only when empty).
    let text = match unsafe { bytes(input, length) } {
        Ok(text) => text,
        Err(error) => return error,
    };
    owner.publish(kind, text)
}
/// Admit the fixed positive RNG result and assemble the lazy boundary directly
/// in its sole Rust owner, without a Native prefix/string allocation.
/// # Safety
/// Unique live owner and initialized independent random bytes for the length
/// requested by `static_boundary`. No Rust frame/borrow spans the Native RNG call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_boundary(
    owner: *mut HttpStaticContext,
    random: *const u8,
    length: usize,
) -> i32 {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let Some(owner) = (unsafe { owner.as_mut() }) else {
        return INVALID;
    };
    if !owner.boundary.is_empty() {
        return 0;
    }
    let prefix = crate::static_meta::BOUNDARY;
    if length != 39 - prefix.len() {
        return INVALID;
    }
    // SAFETY: the caller lends `length` initialized bytes at `random` (null only when empty).
    let random = match unsafe { bytes(random, length) } {
        Ok(random) => random,
        Err(error) => return error,
    };
    let mut boundary = Vec::new();
    if boundary
        .try_reserve_exact(prefix.len() - 1 + length)
        .is_err()
    {
        return ALLOCATION;
    }
    boundary.extend_from_slice(crate::c_text(prefix));
    boundary.extend_from_slice(random);
    owner.boundary = boundary;
    0
}
/// Copy complete initialized observations. No stat read before native success.
/// # Safety
/// Live unchanged context; aligned writable output disjoint from owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_observe(
    owner: *const HttpStaticContext,
    output: *mut HttpStaticObservation,
) -> i32 {
    // SAFETY: `owner` is null or live and aligned for this call.
    let Some(owner) = (unsafe { owner.as_ref() }) else {
        return INVALID;
    };
    if output.is_null() {
        return INVALID;
    }
    let mut result = owner.facts;
    result.last = owner.path.last;
    result.directory_count = owner.directory.len();
    // SAFETY: `owner.tasks` is a live field of this context.
    result.task_count = unsafe { swr_v2_http_tasks_count(&raw const owner.tasks) };
    // SAFETY: `output` is non-null (checked above), aligned and writable per the caller contract.
    unsafe { output.write(result) };
    0
}
/// Publish only the selected initialized native effect observation: 0=successful
/// stat, 1=range accounting/status. Other metadata never overwrites owner state.
/// # Safety
/// Live unique context, aligned initialized input disjoint from owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_observed(
    owner: *mut HttpStaticContext,
    kind: u32,
    input: *const HttpStaticObservation,
) -> i32 {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let owner = unsafe { owner.as_mut() };
    // SAFETY: `input` is null or live and aligned for this call.
    let input = unsafe { input.as_ref() };
    let (Some(owner), Some(input)) = (owner, input) else {
        return INVALID;
    };
    match kind {
        0 => {
            owner.facts.filesize = input.filesize;
            owner.facts.mtime = input.mtime;
            owner.facts.kind = input.kind;
            owner.facts.file_ready = 1;
        }
        1 => {
            owner.facts.content_length = input.content_length;
            owner.facts.status = input.status;
        }
        _ => return INVALID,
    }
    0
}
/// Format/publish multipart content(0) or final boundary(1) directly into the
/// sole Rust owner using existing byte policies. Native sees only later copies.
/// # Safety
/// Unique live context. RNG admission, if needed, completed before this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_format(
    owner: *mut HttpStaticContext,
    kind: u32,
) -> i32 {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let Some(owner) = (unsafe { owner.as_mut() }) else {
        return INVALID;
    };
    if kind > 1 {
        return INVALID;
    }
    let boundary = HttpProxyText {
        data: owner.boundary.as_ptr(),
        length: owner.boundary.len(),
    };
    let range = HttpRangeFormat {
        boundary: HttpProxyText {
            data: boundary.data,
            length: boundary.length,
        },
        mime: HttpProxyText {
            data: std::ptr::null(),
            length: 0,
        },
        offset: 0,
        length: 0,
        filesize: 0,
        part: 1,
        separator: 0,
    };
    let render = |output, capacity, written| {
        if kind == 0 {
            // SAFETY: `boundary` is a local; output follows the caller's render contract.
            unsafe {
                swr_v1_http_static_content_type(&raw const boundary, output, capacity, written)
            }
        } else {
            // SAFETY: `range` is a local; output follows the caller's render contract.
            unsafe { swr_v1_http_range_format(&raw const range, output, capacity, written) }
        }
    };
    let mut length = 0;
    let status = render(std::ptr::null_mut(), 0, &raw mut length);
    if status < 0 {
        return status;
    }
    let Some(capacity) = length
        .checked_add(usize::from(kind == 1))
        .filter(|&value| isize::try_from(value).is_ok())
    else {
        return OVERFLOW;
    };
    let mut value = Vec::new();
    if value.try_reserve_exact(capacity).is_err() {
        return ALLOCATION;
    }
    value.resize(capacity, 0);
    let status = render(value.as_mut_ptr(), value.len(), &raw mut length);
    if status < 0 {
        return status;
    }
    value.truncate(length);
    if kind == 0 {
        owner.content = value;
    } else {
        owner.end = value;
    }
    0
}

/// Context path effects reuse the original Rust FSM. operation0=resumption with
/// fresh root/facts,1=location,2=append owned index preserving base length,
/// 3=catch,4=append page slash,5=begin a fresh path invocation. No caller can observe a Rust path pointer.
/// # Safety
/// Unique live context; initialized input disjoint from owner. Native effects
/// complete between calls, never through a Rust frame or borrowed span.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_path(
    owner: *mut HttpStaticContext,
    operation: u32,
    input: *const u8,
    length: usize,
    facts: u32,
) -> i32 {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let Some(owner) = (unsafe { owner.as_mut() }) else {
        return INVALID;
    };
    match operation {
        0 => {
            // SAFETY: the caller lends `length` initialized bytes at `input` (null only when empty).
            let root = match unsafe { bytes(input, length) } {
                Ok(root) => root,
                Err(error) => return error,
            };
            owner.path_step(root, facts)
        }
        1 => {
            let url = owner.rewritten.as_ref().unwrap_or(&owner.original);
            // SAFETY: `url` is the owner's live text; `input` lends `length` initialized bytes.
            unsafe {
                swr_v1_http_static_location(
                    url.as_ptr(),
                    url.len(),
                    input,
                    length,
                    &raw mut owner.path,
                )
            }
        }
        // SAFETY: the owner's filename buffer is writable through its length, initialized at
        // `filename_length`, and disjoint from `index`.
        2 => unsafe {
            swr_v1_http_static_index_path(
                owner.filename.as_mut_ptr(),
                owner.filename.len(),
                owner.path.filename_length,
                owner.index.as_ptr(),
                owner.index.len(),
            )
        },
        // SAFETY: `owner.facts.status` is a live, exclusive field.
        3 => unsafe { swr_v1_http_static_catch(owner.path.last, &raw mut owner.facts.status) },
        4 => owner.ensure_directory_slash(),
        5 => {
            owner.path.phase = 0;
            owner.path.url_length = 0;
            0
        }
        _ => INVALID,
    }
}
/// Directory effect adapter:0=append copied entry,1=finish scan,2=contains.
/// # Safety
/// Unique live context; input initialized/disjoint. Native DIR resources stay
/// outside Rust and all calls finish before readdir/closedir resumes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_directory(
    owner: *mut HttpStaticContext,
    operation: u32,
    input: *const u8,
    length: usize,
) -> i32 {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let Some(owner) = (unsafe { owner.as_mut() }) else {
        return INVALID;
    };
    match operation {
        0 | 2 => {
            // SAFETY: the caller lends `length` initialized bytes at `input` (null only when empty).
            let name = match unsafe { bytes(input, length) } {
                Ok(name) => name,
                Err(error) => return error,
            };
            if operation == 0 {
                match owner.directory.scan_append(name) {
                    Ok(()) => 1,
                    Err(error) => error,
                }
            } else {
                match owner.directory.contains(name) {
                    Ok(found) => i32::from(found),
                    Err(error) => error,
                }
            }
        }
        1 => {
            owner.directory.scan_finish();
            0
        }
        _ => INVALID,
    }
}
/// # Safety
/// Live unchanged context; CSS/powered and output obey `index_page`'s initialized,
/// disjoint spans. Copies only; native buffer reserve happens between calls.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_page(
    owner: *const HttpStaticContext,
    index: usize,
    page: *const HttpIndexPage,
    output: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> i32 {
    // SAFETY: `owner` is null or live and aligned for this call.
    let owner = unsafe { owner.as_ref() };
    // SAFETY: `page` is null or live and aligned for this call.
    let page = unsafe { page.as_ref() };
    let (Some(owner), Some(page)) = (owner, page) else {
        return INVALID;
    };
    let input = HttpIndexPage {
        path: HttpProxyText {
            data: owner.directory_path.as_ptr(),
            length: owner.directory_path.len(),
        },
        entry: HttpProxyText {
            data: std::ptr::null(),
            length: 0,
        },
        css: HttpProxyText {
            data: page.css.data,
            length: page.css.length,
        },
        powered: HttpProxyText {
            data: page.powered.data,
            length: page.powered.length,
        },
        part: page.part,
    };
    // SAFETY: `input` is a local copy of the caller's spans; output follows the contract.
    unsafe {
        swr_v2_http_directory_page(
            &raw const owner.directory,
            index,
            &raw const input,
            output,
            capacity,
            written,
        )
    }
}
/// Range scratch/commit:0=begin,1=prepare,2=push,3=clear. Native RNG/MIME/date
/// effects have ended; no pointer to the by-value task owner escapes.
/// # Safety
/// Unique live context; optional format and written obey `tasks_prepare` contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_task(
    owner: *mut HttpStaticContext,
    operation: u32,
    offset: i64,
    length: usize,
    format: *const HttpRangeFormat,
    written: *mut usize,
) -> i32 {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let Some(owner) = (unsafe { owner.as_mut() }) else {
        return INVALID;
    };
    match operation {
        // SAFETY: `owner.tasks` is a live, exclusive field of this context.
        0 => unsafe { swr_v2_http_tasks_begin(&raw mut owner.tasks) },
        // SAFETY: as above; `format` and `written` follow tasks_prepare's contract.
        1 => unsafe {
            swr_v2_http_tasks_prepare(&raw mut owner.tasks, offset, length, format, written)
        },
        // SAFETY: as above.
        2 => unsafe { swr_v2_http_tasks_push(&raw mut owner.tasks) },
        // SAFETY: as above.
        3 => unsafe { swr_v2_http_tasks_clear(&raw mut owner.tasks) },
        _ => INVALID,
    }
}
/// # Safety
/// Live unchanged context and independent native output/written, as `task_snapshot`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_tasks(
    owner: *const HttpStaticContext,
    output: *mut HttpStaticTask,
    capacity: usize,
    written: *mut usize,
) -> i32 {
    // SAFETY: `owner` is null or live and aligned for this call.
    let Some(owner) = (unsafe { owner.as_ref() }) else {
        return INVALID;
    };
    // SAFETY: `owner.tasks` is live; output follows task_snapshot's contract.
    unsafe { swr_v2_http_tasks_snapshot(&raw const owner.tasks, output, capacity, written) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn make() -> *mut HttpStaticContext {
        unsafe { swr_v2_http_context_new(b"/files?q=1".as_ptr(), 10, 128) }
    }
    fn put(owner: *mut HttpStaticContext, kind: u32, value: &[u8]) {
        assert_eq!(
            unsafe { swr_v2_http_context_publish(owner, kind, value.as_ptr(), value.len()) },
            0
        );
    }
    fn read(owner: *const HttpStaticContext, kind: u32) -> Vec<u8> {
        let mut length = 0;
        assert_eq!(
            unsafe {
                swr_v2_http_context_text(owner, kind, std::ptr::null_mut(), 0, &raw mut length)
            },
            0
        );
        let mut value = vec![0; length];
        assert_eq!(
            unsafe {
                swr_v2_http_context_text(
                    owner,
                    kind,
                    value.as_mut_ptr(),
                    value.len(),
                    &raw mut length,
                )
            },
            0
        );
        value
    }
    #[test]
    fn rewrite_publishes_over_the_owned_original() {
        let original = b"/a\0tail";
        let owner = unsafe { swr_v2_http_context_new(original.as_ptr(), original.len(), 128) };
        assert!(!owner.is_null());
        let mut rules = crate::RewriteRules::default();
        rules.add(b"/a\0", b"/b\0");
        rules.add(b"/b\0tail", b"");
        assert!(unsafe { (*owner).rewrite(&rules) });
        assert_eq!(read(owner, URL), b"/b\0tail");
        assert!(unsafe { (*owner).rewrite(&rules) });
        assert_eq!(read(owner, URL), b"");
        assert_eq!(read(owner, ORIGINAL), original);
        assert!(!unsafe { (*owner).rewrite(&crate::RewriteRules::default()) });
        assert_eq!(read(owner, URL), b"");
        unsafe { swr_v2_http_context_drop(owner) };
    }
    #[test]
    fn fresh_path_invocation_retains_handler_flags_and_base_length() {
        let owner = make();
        unsafe {
            (*owner).path.phase = 8;
            (*owner).path.last = 1;
            (*owner).path.filename_length = 12;
            (*owner).path.url_length = 99;
        }
        assert_eq!(
            unsafe { swr_v2_http_context_path(owner, 5, std::ptr::null(), 0, 0) },
            0
        );
        let path = unsafe { &(*owner).path };
        assert_eq!(
            (path.phase, path.last, path.filename_length, path.url_length),
            (0, 1, 12, 0)
        );
        unsafe { swr_v2_http_context_drop(owner) };
    }
    #[test]
    fn content_and_end_render_from_the_owned_binary_boundary() {
        let owner = make();
        let mut boundary = b"one\0two".to_vec();
        put(owner, BOUNDARY, &boundary);
        boundary.fill(b'x');
        assert_eq!(unsafe { swr_v2_http_context_format(owner, 0) }, 0);
        assert_eq!(unsafe { swr_v2_http_context_format(owner, 1) }, 0);
        assert_eq!(
            read(owner, CONTENT),
            b"multipart/byteranges; boundary=one\0two"
        );
        assert_eq!(read(owner, END), b"\r\n--one\0two--\r\n");
        unsafe { swr_v2_http_context_drop(owner) };
    }
    #[test]
    fn owned_original_rewrite_and_atomic_copies_are_independent() {
        let mut input = b"/first".to_vec();
        let owner = unsafe { swr_v2_http_context_new(input.as_ptr(), input.len(), 128) };
        assert!(!owner.is_null());
        input.fill(b'x');
        assert_eq!(read(owner, URL), b"/first");
        put(owner, URL, b"/second\0tail");
        assert_eq!(read(owner, URL), b"/second\0tail");
        assert_eq!(read(owner, ORIGINAL), b"/first");
        let mut output = [0xa5; 4];
        let mut written = 99;
        assert_eq!(
            unsafe {
                swr_v2_http_context_text(
                    owner,
                    URL,
                    output.as_mut_ptr(),
                    output.len(),
                    &raw mut written,
                )
            },
            OVERFLOW
        );
        assert_eq!(output, [0xa5; 4]);
        assert_eq!(written, 99);
        unsafe { swr_v2_http_context_drop(owner) };
    }
    #[test]
    fn path_owner_preserves_index_backing_and_separate_base_length() {
        let owner = make();
        assert!(!owner.is_null());
        let root = b"/root";
        assert_eq!(
            unsafe { swr_v2_http_context_path(owner, 0, root.as_ptr(), root.len(), 0) },
            2
        );
        assert_eq!(
            unsafe { swr_v2_http_context_path(owner, 0, root.as_ptr(), root.len(), 0) },
            3
        );
        assert_eq!(read(owner, DIRECTORY), b"/files");
        assert_eq!(
            unsafe { swr_v2_http_context_path(owner, 0, root.as_ptr(), root.len(), 0) },
            4
        );
        put(owner, INDEX, b"index.html");
        let base = unsafe { &*owner }.path.filename_length;
        assert_eq!(
            unsafe { swr_v2_http_context_path(owner, 2, std::ptr::null(), 0, 0) },
            0
        );
        assert_eq!(unsafe { &*owner }.path.filename_length, base);
        assert_eq!(read(owner, FILENAME), b"/root/files/index.html");
        put(owner, INDEX, b"other.html");
        assert_eq!(
            unsafe { swr_v2_http_context_path(owner, 2, std::ptr::null(), 0, 0) },
            0
        );
        // The pinned helper observes slash at the unchanged base offset,
        // then overwrites from that offset on a repeated append.
        assert_eq!(read(owner, FILENAME), b"/root/filesother.html");
        unsafe { swr_v2_http_context_drop(owner) };
    }
    #[test]
    fn initialized_facts_and_nested_stores_have_one_custodian() {
        let owner = make();
        let mut facts = HttpStaticObservation::default();
        assert_eq!(
            unsafe { swr_v2_http_context_observe(owner, &raw mut facts) },
            0
        );
        assert_eq!(facts.status, 200);
        assert_eq!(facts.file_ready, 0);
        for name in [b"z".as_slice(), b"a", b"z"] {
            assert_eq!(
                unsafe { swr_v2_http_context_directory(owner, 0, name.as_ptr(), name.len()) },
                1
            );
        }
        assert_eq!(
            unsafe { swr_v2_http_context_directory(owner, 1, std::ptr::null(), 0) },
            0
        );
        let mut written = 0;
        assert_eq!(
            unsafe {
                swr_v2_http_context_task(owner, 0, 0, 0, std::ptr::null(), std::ptr::null_mut())
            },
            0
        );
        assert_eq!(
            unsafe {
                swr_v2_http_context_task(owner, 1, 17, 23, std::ptr::null(), &raw mut written)
            },
            0
        );
        assert_eq!(
            unsafe {
                swr_v2_http_context_task(owner, 2, 0, 0, std::ptr::null(), std::ptr::null_mut())
            },
            0
        );
        assert_eq!(
            unsafe { swr_v2_http_context_observe(owner, &raw mut facts) },
            0
        );
        assert_eq!(facts.directory_count, 2);
        assert_eq!(facts.task_count, 1);
        let mut task = HttpStaticTask::default();
        assert_eq!(
            unsafe { swr_v2_http_context_tasks(owner, &raw mut task, 1, &raw mut written) },
            0
        );
        unsafe { swr_v2_http_context_drop(owner) };
        assert_eq!(task.offset, 17);
        assert_eq!(task.length, 23);
        assert_eq!(task.part_header, [0; 256]);
    }
    #[test]
    fn lazy_boundary_assembly_owns_prefix_and_positive_rng_bytes_once() {
        let owner = make();
        let mut random = b"ABCDEFGHIJKLMNOPQRSTUVWX".to_vec();
        assert_eq!(
            unsafe { swr_v2_http_context_boundary(owner, random.as_ptr(), random.len()) },
            0
        );
        random.fill(b'x');
        assert_eq!(
            read(owner, BOUNDARY),
            b"SwooleBoundaryABCDEFGHIJKLMNOPQRSTUVWX"
        );
        assert_eq!(
            unsafe { swr_v2_http_context_boundary(owner, random.as_ptr(), random.len()) },
            0
        );
        assert_eq!(
            read(owner, BOUNDARY),
            b"SwooleBoundaryABCDEFGHIJKLMNOPQRSTUVWX"
        );
        unsafe { swr_v2_http_context_drop(owner) };
    }
}
