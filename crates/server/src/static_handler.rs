//! Upstream's `http_server::StaticHandler` (`src/server/static_handler.cc`):
//! a request URI resolved against the server's document root, locations,
//! index files and rewrite rules into a file, a directory listing or byte
//! ranges. The state is crates/http's static context; this drives it with
//! the server's configuration.
use crate::view;
use core::ffi::c_void;
use core::ptr;
use swoole_rs_http::{
    HttpContextPageBuffer, HttpContextRangeObservation, HttpContextServeObservation, HttpIndexPage,
    HttpProxyText, HttpStaticContext, HttpStaticObservation, HttpStaticTask,
    swr_v1_http_static_boundary, swr_v1_http_static_content_prefix, swr_v2_http_context_boundary,
    swr_v2_http_context_drop, swr_v2_http_context_format, swr_v2_http_context_index,
    swr_v2_http_context_new, swr_v2_http_context_observe, swr_v2_http_context_page_emit,
    swr_v2_http_context_page_step, swr_v2_http_context_path, swr_v2_http_context_range_begin,
    swr_v2_http_context_range_resume, swr_v2_http_context_range_step, swr_v2_http_context_tasks,
    swr_v2_http_context_text, swr_v3_http_conditional, swr_v3_http_context_directory,
    swr_v3_http_context_mtime, swr_v3_http_context_serve, swr_v3_http_context_stat,
    swr_v3_http_date,
};

/// `SWR_HTTP_CONTEXT_*`: the context's texts.
const INDEX: u32 = 3;
const FILENAME: u32 = 4;
const BOUNDARY: u32 = 5;
const CONTENT: u32 = 6;
const END: u32 = 7;
/// `SWR_HTTP_PATH_*`: `try_serve`'s actions and the path's kind bits.
const PATH_SKIP: i32 = 0;
const PATH_SERVE: i32 = 1;
const PATH_LOCATIONS: i32 = 2;
const PATH_MIME: i32 = 6;
const PATH_IS_DIRECTORY: u32 = 16;
const PATH_FILE: u32 = 32;
/// The static collections (`ServerStrings` kinds).
const LOCATIONS: u32 = 0;
const INDEX_FILES: u32 = 1;
/// `SW_HTTP_SERVER_BOUNDARY_TOTAL_SIZE`.
const BOUNDARY_SIZE: usize = 39;
/// `PATH_MAX`, the context's filename capacity and the index page's first size.
const PATH_MAX: usize = libc::PATH_MAX as usize;
/// `SW_HTTP_PAGE_CSS` and `SW_HTTP_POWER_BY`.
const CSS: &[u8] = b"<style> body { padding: 0.5em; line-height: 2; } h1 { font-size: 1.5em; \
padding-bottom: 0.3em; border-bottom: 1px solid #ccc; } ul { list-style-type: disc; } footer { \
border-top: 1px solid #ccc; } a { color: #0969da; } </style>";
const POWER_BY: &[u8] = b"<footer><i>Powered by Swoole</i></footer>";

/// `SW_HTTP_PAGE_404`.
#[must_use]
pub fn page_404() -> Vec<u8> {
    [
        b"<html><body>".as_slice(),
        CSS,
        b"<h1>HTTP 404 Not Found</h1>",
        POWER_BY,
        b"</body></html>",
    ]
    .concat()
}

fn text_of(bytes: &[u8]) -> HttpProxyText {
    HttpProxyText {
        data: bytes.as_ptr(),
        length: bytes.len(),
    }
}

/// A `StaticHandler` for one request URI on one server.
pub struct StaticHandler {
    server: *mut c_void,
    context: *mut HttpStaticContext,
    /// The context's modification-time slot.
    mtime: *const i64,
}

impl Drop for StaticHandler {
    fn drop(&mut self) {
        // SAFETY: the context `new` made, dropped once.
        unsafe { swr_v2_http_context_drop(self.context) };
    }
}

impl StaticHandler {
    /// # Safety
    /// `server` is a live `Server` that outlives the handler.
    #[must_use]
    pub unsafe fn new(server: *mut c_void, url: &[u8]) -> Option<Self> {
        // SAFETY: the URL's bytes, copied by the context.
        let context = unsafe { swr_v2_http_context_new(url.as_ptr(), url.len(), PATH_MAX) };
        if context.is_null() {
            return None;
        }
        // SAFETY: the live context, whose slot lives as long as it.
        let mtime = unsafe { swr_v3_http_context_mtime(context) };
        Some(Self {
            server,
            context,
            mtime,
        })
    }

    fn strings(&self, kind: u32) -> (bool, Vec<Vec<u8>>) {
        // SAFETY: the live server the caller of `new` vouched for.
        unsafe { view::static_strings(self.server, kind) }
    }

    fn autoindex(&self) -> bool {
        // SAFETY: as above.
        unsafe { view::config(self.server) }.http_autoindex
    }

    fn observe(&self) -> HttpStaticObservation {
        let mut value = HttpStaticObservation::default();
        // SAFETY: the live context and a local output.
        unsafe { swr_v2_http_context_observe(self.context, &raw mut value) };
        value
    }

    fn text(&self, kind: u32) -> Vec<u8> {
        let mut length = 0;
        // SAFETY: the live context; a zero capacity asks for the length.
        unsafe {
            swr_v2_http_context_text(self.context, kind, ptr::null_mut(), 0, &raw mut length)
        };
        let mut value = vec![0; length];
        if length != 0 {
            // SAFETY: `length` writable bytes.
            unsafe {
                swr_v2_http_context_text(
                    self.context,
                    kind,
                    value.as_mut_ptr(),
                    length,
                    &raw mut length,
                );
            };
        }
        value
    }

    #[must_use]
    pub fn status(&self) -> i32 {
        self.observe().status
    }

    #[must_use]
    pub fn filesize(&self) -> usize {
        self.observe().filesize
    }

    #[must_use]
    pub fn content_length(&self) -> usize {
        self.observe().content_length
    }

    #[must_use]
    pub fn filename(&self) -> Vec<u8> {
        self.text(FILENAME)
    }

    #[must_use]
    pub fn end_part(&self) -> Vec<u8> {
        self.text(END)
    }

    /// No index file was found, `http_autoindex` is on and the path is a directory.
    #[must_use]
    pub fn lists_directory(&self) -> bool {
        let value = self.observe();
        self.text(INDEX).is_empty()
            && self.autoindex()
            && value.file_ready != 0
            && value.kind & PATH_IS_DIRECTORY != 0
    }

    #[must_use]
    pub fn mimetype(&self) -> Vec<u8> {
        let kind = swoole_rs_framing::mime::get(&self.filename());
        // SAFETY: the table's NUL-terminated type, copied before it can change.
        unsafe { core::ffi::CStr::from_ptr(kind) }
            .to_bytes()
            .to_vec()
    }

    fn ensure_boundary(&self) -> u32 {
        let mut count = 0;
        // SAFETY: a local output.
        if !unsafe { swr_v1_http_static_boundary(self.text(BOUNDARY).len(), &raw mut count) }
            .is_null()
        {
            let mut random = [0u8; BOUNDARY_SIZE];
            let count = count.min(BOUNDARY_SIZE - 1);
            // SAFETY: `count` characters and a NUL fit.
            unsafe { swoole_rs_corebase::swoole_random_string(random.as_mut_ptr().cast(), count) };
            // SAFETY: the live context and `count` initialized bytes.
            unsafe { swr_v2_http_context_boundary(self.context, random.as_ptr(), count) };
        }
        1
    }

    /// `get_date()`: now, as an HTTP date.
    #[must_use]
    pub fn date() -> Vec<u8> {
        let mut value = [0u8; 64];
        // SAFETY: a null mtime asks for now; a local output.
        let length = unsafe { swr_v3_http_date(ptr::null(), value.as_mut_ptr(), value.len()) };
        value.get(..length).unwrap_or(&value).to_vec()
    }

    #[must_use]
    pub fn date_last_modified(&self) -> Vec<u8> {
        let mut value = [0u8; 64];
        // SAFETY: the context's mtime slot and a local output.
        let length = unsafe { swr_v3_http_date(self.mtime, value.as_mut_ptr(), value.len()) };
        value.get(..length).unwrap_or(&value).to_vec()
    }

    fn conditional(&self, date: &[u8], kind: u32, timezone: *const libc::c_long) -> bool {
        // SAFETY: the date's bytes, the context's mtime slot and the timezone (or null).
        unsafe {
            swr_v3_http_conditional(
                date.as_ptr().cast(),
                date.len(),
                kind,
                self.mtime,
                timezone,
                ptr::null(),
            ) == 1
        }
    }

    /// The file changed since `date` (an `If-Modified-Since`).
    #[must_use]
    pub fn is_modified(&self, date: &[u8]) -> bool {
        // SAFETY: the live server.
        let timezone = &raw const unsafe { view::config(self.server) }.timezone_;
        self.conditional(date, 0, timezone)
    }

    fn serve_step(&self, root: &HttpProxyText, observation: &HttpContextServeObservation) -> i32 {
        // SAFETY: the live context and local descriptors.
        unsafe { swr_v3_http_context_serve(self.context, root, observation, ptr::null()) }
    }

    /// Whether the static handler answers the request.
    #[must_use]
    pub fn try_serve(&self) -> bool {
        // SAFETY: the live server and the context, which no one else uses.
        unsafe { view::static_rewrite(self.server, self.context) };
        // SAFETY: the live server.
        let root = unsafe { view::document_root(self.server) };
        let root = text_of(&root);
        let (_, locations) = self.strings(LOCATIONS);
        let (index_enabled, index_files) = self.strings(INDEX_FILES);
        let mut observation = HttpContextServeObservation {
            event: 0,
            observed: 0,
            has_locations: u32::from(!locations.is_empty()),
            has_index: u32::from(index_enabled && !index_files.is_empty()),
            autoindex: u32::from(self.autoindex()),
            value: text_of(&[]),
        };
        let mut action = self.serve_step(&root, &observation);
        observation.has_locations = 0;
        observation.has_index = 0;
        observation.autoindex = 0;
        loop {
            match action {
                PATH_SKIP => return false,
                PATH_SERVE => return true,
                PATH_LOCATIONS => {
                    for location in &locations {
                        // SAFETY: the live context and the location's bytes.
                        unsafe {
                            swr_v2_http_context_path(
                                self.context,
                                1,
                                location.as_ptr(),
                                location.len(),
                                0,
                            )
                        };
                    }
                    observation.event = 1;
                    observation.observed = 0;
                }
                PATH_MIME => {
                    observation.event = 4;
                    observation.observed =
                        u32::from(swoole_rs_framing::mime::exists(&self.filename()));
                }
                _ => return false,
            }
            action = self.serve_step(&root, &observation);
        }
    }

    fn scan_directory(&self) -> bool {
        // SAFETY: the live context.
        unsafe { swr_v3_http_context_directory(self.context, ptr::null()) == 1 }
    }

    fn set_filename(&self) -> bool {
        // SAFETY: the live context.
        if unsafe { swr_v2_http_context_path(self.context, 2, ptr::null(), 0, 0) } != 0 {
            return false;
        }
        // SAFETY: as above.
        let stat = unsafe { swr_v3_http_context_stat(self.context, ptr::null()) } == 1;
        stat && self.observe().kind & PATH_FILE != 0
    }

    /// With `http_index_files`, the directory's first index file becomes the
    /// file. False to stop serving.
    #[must_use]
    pub fn try_serve_index_file(&self) -> bool {
        let (enabled, files) = self.strings(INDEX_FILES);
        let cursor = core::cell::Cell::new(0);
        let step = |event: u32, value: usize, name: &[u8]| {
            // SAFETY: the live context, the name's bytes and a local cursor.
            unsafe {
                swr_v2_http_context_index(
                    self.context,
                    event,
                    value,
                    name.as_ptr(),
                    name.len(),
                    cursor.as_ptr(),
                )
            }
        };
        let mut action = step(0, if enabled { files.len() } else { 0 }, &[]);
        loop {
            action = match action {
                0 => return false,
                1 => return true,
                2 => {
                    if self.scan_directory() {
                        step(1, files.len(), &[])
                    } else {
                        step(6, 0, &[])
                    }
                }
                3 => {
                    let name = files.get(cursor.get()).cloned().unwrap_or_default();
                    step(2, 0, &name)
                }
                4 => step(4, usize::from(self.set_filename()), &[]),
                5 => step(3, usize::from(self.autoindex()), &[]),
                _ => return false,
            };
        }
    }

    /// The directory's listing page.
    #[must_use]
    pub fn index_page(&self) -> Vec<u8> {
        self.scan_directory();
        let mut buffer = vec![0u8; PATH_MAX];
        let mut length = 0;
        let mut entry = 0;
        // SAFETY: the live context and a local cursor.
        let mut part = unsafe { swr_v2_http_context_page_step(self.context, 1, &raw mut entry) };
        while part > 0 {
            #[expect(clippy::cast_sign_loss, reason = "positive")]
            let input = HttpIndexPage {
                path: text_of(&[]),
                entry: text_of(&[]),
                css: text_of(CSS),
                powered: text_of(POWER_BY),
                part: (part - 1) as u32,
            };
            loop {
                let mut result = HttpContextPageBuffer {
                    required: 0,
                    length: 0,
                };
                // SAFETY: the live context, the page's texts and the buffer's
                // `buffer.len()` writable bytes, `length` of them written.
                let action = unsafe {
                    swr_v2_http_context_page_emit(
                        self.context,
                        entry,
                        &raw const input,
                        buffer.as_mut_ptr(),
                        length,
                        buffer.len(),
                        &raw mut result,
                    )
                };
                match action {
                    1 | 2 => buffer.resize(result.required.max(buffer.len()), 0),
                    3 => {
                        length = result.length;
                        break;
                    }
                    _ => break,
                }
            }
            // SAFETY: as above.
            part = unsafe { swr_v2_http_context_page_step(self.context, 0, &raw mut entry) };
        }
        buffer.truncate(length);
        buffer
    }

    fn range_resume(&self, observed: &HttpContextRangeObservation) -> i32 {
        // SAFETY: the live context and a local observation.
        unsafe { swr_v2_http_context_range_resume(self.context, observed) }
    }

    /// The `Range` and `If-Range` headers into the response's status and tasks.
    pub fn parse_range(&self, range: Option<&[u8]>, if_range: Option<&[u8]>) {
        let range = range.unwrap_or_default();
        // SAFETY: the live context.
        if unsafe {
            swr_v2_http_context_range_begin(
                self.context,
                range.len(),
                u32::from(if_range.is_some()),
            )
        } < 0
        {
            return;
        }
        loop {
            // SAFETY: the live context and the header's bytes.
            let action = unsafe {
                swr_v2_http_context_range_step(self.context, range.as_ptr(), range.len())
            };
            let mut observed = HttpContextRangeObservation {
                mime: text_of(&[]),
                boundary_ready: 0,
                modified: 0,
            };
            match action {
                1 => {
                    observed.boundary_ready = self.ensure_boundary();
                    let mime = self.mimetype();
                    observed.mime = text_of(&mime);
                    if self.range_resume(&observed) == 1 {
                        errno::set(libc::EOVERFLOW);
                    }
                }
                2 => {
                    observed.boundary_ready = self.ensure_boundary();
                    self.range_resume(&observed);
                }
                3 => {
                    observed.modified =
                        u32::from(self.conditional(if_range.unwrap_or_default(), 1, ptr::null()));
                    self.range_resume(&observed);
                }
                _ => return,
            }
        }
    }

    /// The response's byte ranges.
    #[must_use]
    pub fn tasks(&self) -> Vec<HttpStaticTask> {
        let count = self.observe().task_count;
        let mut tasks = vec![HttpStaticTask::default(); count];
        let mut written = 0;
        // SAFETY: the live context and `count` writable tasks.
        unsafe {
            swr_v2_http_context_tasks(self.context, tasks.as_mut_ptr(), count, &raw mut written);
        };
        tasks.truncate(written);
        tasks
    }

    /// The response's `Content-Type`: multipart with several ranges, else the file's.
    #[must_use]
    pub fn content_type(&self) -> Vec<u8> {
        if swr_v1_http_static_content_prefix(self.observe().task_count).is_null() {
            return self.mimetype();
        }
        self.ensure_boundary();
        // SAFETY: the live context.
        unsafe { swr_v2_http_context_format(self.context, 0) };
        self.text(CONTENT)
    }
}

mod errno {
    pub(super) fn set(value: libc::c_int) {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        // SAFETY: returns this thread's errno slot, live for the thread.
        let slot = unsafe { libc::__errno_location() };
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        // SAFETY: returns this thread's errno slot, live for the thread.
        let slot = unsafe { libc::__error() };
        // SAFETY: `slot` is this thread's errno.
        unsafe { slot.write(value) };
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_404_page_is_upstreams() {
        assert_eq!(
            super::page_404(),
            b"<html><body><style> body { padding: 0.5em; line-height: 2; } h1 { font-size: 1.5em; \
              padding-bottom: 0.3em; border-bottom: 1px solid #ccc; } ul { list-style-type: disc; } \
              footer { border-top: 1px solid #ccc; } a { color: #0969da; } </style><h1>HTTP 404 Not \
              Found</h1><footer><i>Powered by Swoole</i></footer></body></html>"
        );
    }
}
