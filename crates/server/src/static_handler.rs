//! Upstream's `http_server::StaticHandler` (`src/server/static_handler.cc`):
//! a request URI resolved against the server's document root, locations,
//! index files and rewrite rules into a file, a directory listing or byte
//! ranges. The state is crates/http's static context; this drives it with
//! the server's configuration.
use crate::view;
use core::cell::RefCell;
use core::ffi::c_void;
use swoole_rs_http::{
    BOUNDARY_SIZE, Calendar, HttpStaticContext, HttpStaticTask, StaticEffects, http_date,
};

/// The path kinds' directory bit (`crates/http/src/static_path.rs`).
const PATH_IS_DIRECTORY: u32 = 16;
/// The static collections (`ServerStrings` kinds).
const LOCATIONS: u32 = 0;
const INDEX_FILES: u32 = 1;
/// `PATH_MAX`, the context's filename capacity.
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

/// Production's effects: the process's filesystem, the MIME table, random
/// characters and libc's calendar.
pub trait Effects: StaticEffects + Calendar {}

/// Production's effects.
pub struct Native;
impl StaticEffects for Native {
    fn mime(&self, filename: &[u8]) -> Vec<u8> {
        let kind = swoole_rs_framing::mime::get(filename);
        // SAFETY: the table's NUL-terminated type, copied before it can change.
        unsafe { core::ffi::CStr::from_ptr(kind) }
            .to_bytes()
            .to_vec()
    }
    fn mime_exists(&self, filename: &[u8]) -> bool {
        swoole_rs_framing::mime::exists(filename)
    }
    fn random(&self, out: &mut [u8; BOUNDARY_SIZE], count: usize) {
        // SAFETY: `count` characters and a NUL fit.
        unsafe { swoole_rs_corebase::swoole_random_string(out.as_mut_ptr().cast(), count) };
    }
    fn modified_range(&self, if_range: &[u8], mtime: i64) -> bool {
        swoole_rs_http::conditional(if_range, 1, mtime, 0, self)
    }
    fn overflow(&self) {
        errno::set(libc::EOVERFLOW);
    }
}
impl Calendar for Native {}
impl Effects for Native {}

/// A `StaticHandler` for one request URI on one server.
pub struct StaticHandler<E: Effects = Native> {
    server: view::Server<'static>,
    /// Borrowed for one call at a time; no effect calls back into the handler.
    context: RefCell<HttpStaticContext>,
    effects: E,
}

impl StaticHandler {
    /// # Safety
    /// `server` is a live `Server` that outlives the handler.
    #[must_use]
    pub unsafe fn new(server: *mut c_void, url: &[u8]) -> Option<Self> {
        // SAFETY: per the caller.
        unsafe { Self::with_effects(server, url, Native) }
    }
}

impl<E: Effects> StaticHandler<E> {
    /// # Safety
    /// `server` is a live `Server` that outlives the handler.
    #[must_use]
    pub unsafe fn with_effects(server: *mut c_void, url: &[u8], effects: E) -> Option<Self> {
        Some(Self {
            context: RefCell::new(HttpStaticContext::new(url, PATH_MAX)?),
            // SAFETY: the caller vouches that `server` is live and outlives the handler,
            // which keeps it private.
            server: unsafe { view::Server::from_raw(server) },
            effects,
        })
    }

    fn strings(&self, kind: u32) -> (bool, Vec<Vec<u8>>) {
        self.server.static_strings(kind)
    }

    fn autoindex(&self) -> bool {
        self.server.config().http_autoindex
    }

    #[must_use]
    pub fn status(&self) -> i32 {
        self.context.borrow().observe().status
    }

    #[must_use]
    pub fn filesize(&self) -> usize {
        self.context.borrow().observe().filesize
    }

    #[must_use]
    pub fn content_length(&self) -> usize {
        self.context.borrow().observe().content_length
    }

    #[must_use]
    pub fn filename(&self) -> Vec<u8> {
        self.context.borrow().filename().to_vec()
    }

    #[must_use]
    pub fn end_part(&self) -> Vec<u8> {
        self.context.borrow().end_part().to_vec()
    }

    /// No index file was found, `http_autoindex` is on and the path is a directory.
    #[must_use]
    pub fn lists_directory(&self) -> bool {
        let context = self.context.borrow();
        let value = context.observe();
        context.index_file().is_empty()
            && self.autoindex()
            && value.file_ready != 0
            && value.kind & PATH_IS_DIRECTORY != 0
    }

    #[must_use]
    pub fn mimetype(&self) -> Vec<u8> {
        self.effects.mime(&self.filename())
    }

    /// `get_date()`: now, as an HTTP date.
    #[must_use]
    pub fn date() -> Vec<u8> {
        http_date(None)
    }

    #[must_use]
    pub fn date_last_modified(&self) -> Vec<u8> {
        http_date(Some(self.context.borrow().mtime()))
    }

    /// The file changed since `date` (an `If-Modified-Since`).
    #[must_use]
    pub fn is_modified(&self, date: &[u8]) -> bool {
        let timezone = self.server.config().timezone_;
        let mtime = self.context.borrow().mtime();
        swoole_rs_http::conditional(date, 0, mtime, timezone, &self.effects)
    }

    /// Whether the static handler answers the request.
    #[must_use]
    pub fn try_serve(&self) -> bool {
        let mut context = self.context.borrow_mut();
        self.server.static_rewrite(&mut context);
        let root = self.server.document_root();
        let (_, locations) = self.strings(LOCATIONS);
        let (index_enabled, index_files) = self.strings(INDEX_FILES);
        let has_index = index_enabled && !index_files.is_empty();
        let autoindex = self.autoindex();
        context.try_serve(&root, &locations, has_index, autoindex, &self.effects)
    }

    /// With `http_index_files`, the directory's first index file becomes the
    /// file. False to stop serving.
    #[must_use]
    pub fn try_serve_index_file(&self) -> bool {
        let (enabled, files) = self.strings(INDEX_FILES);
        let files = if enabled { files } else { Vec::new() };
        let autoindex = self.autoindex();
        self.context
            .borrow_mut()
            .try_serve_index_file(&files, autoindex, &self.effects)
    }

    /// The directory's listing page.
    #[must_use]
    pub fn index_page(&self) -> Vec<u8> {
        self.context
            .borrow_mut()
            .index_page(CSS, POWER_BY, &self.effects)
    }

    /// The `Range` and `If-Range` headers into the response's status and tasks.
    pub fn parse_range(&self, range: Option<&[u8]>, if_range: Option<&[u8]>) {
        self.context
            .borrow_mut()
            .parse_range(range, if_range, &self.effects);
    }

    /// The response's byte ranges.
    #[must_use]
    pub fn tasks(&self) -> Vec<HttpStaticTask> {
        self.context.borrow().tasks().to_vec()
    }

    /// The response's `Content-Type`: multipart with several ranges, else the file's.
    #[must_use]
    pub fn content_type(&self) -> Vec<u8> {
        self.context.borrow_mut().content_type(&self.effects)
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
mod tests;
