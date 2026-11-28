//! Upstream's `http_server::StaticHandler` (`src/server/static_handler.cc`):
//! a request URI resolved against the server's document root, locations,
//! index files and rewrite rules into a file, a directory listing or byte
//! ranges. The state is crates/http's static context; this drives it with
//! the server's configuration.
use crate::view;
use swoole_rs_http::{
    BOUNDARY_SIZE, Calendar, HttpStaticContext, HttpStaticTask, StaticEffects, StaticFileKind,
    http_date,
};

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
        swoole_rs_framing::mime::get(filename)
    }
    fn mime_exists(&self, filename: &[u8]) -> bool {
        swoole_rs_framing::mime::exists(filename)
    }
    fn random(&self, out: &mut [u8; BOUNDARY_SIZE], count: usize) {
        swoole_rs_corebase::swoole_random_string(out.get_mut(..=count).unwrap_or_default());
    }
    fn modified_range(&self, if_range: &[u8], mtime: i64) -> bool {
        swoole_rs_http::conditional(if_range, swoole_rs_http::Conditional::Range, mtime, self)
    }
    fn overflow(&self) {
        swoole_rs_corebase::sys::set_errno(libc::EOVERFLOW);
    }
}
impl Calendar for Native {}
impl Effects for Native {}

/// A `StaticHandler` for one request URI on one server.
pub struct StaticHandler<'a, E: Effects = Native> {
    server: view::Server<'a>,
    /// Owned: the methods that change it take the handler as `&mut`.
    context: HttpStaticContext,
    effects: E,
    opened: Option<std::sync::Arc<swoole_rs_corebase::ossupport::FileRecord>>,
}

impl<'a> StaticHandler<'a> {
    /// A handler of `url` over `server`.
    #[must_use]
    pub fn new(server: view::Server<'a>, url: &[u8]) -> Option<Self> {
        Self::with_effects(server, url, Native)
    }
}

impl<'a, E: Effects> StaticHandler<'a, E> {
    /// A handler of `url` over `server`, with `effects`.
    #[must_use]
    pub fn with_effects(server: view::Server<'a>, url: &[u8], effects: E) -> Option<Self> {
        Some(Self {
            context: HttpStaticContext::new(url, PATH_MAX)?,
            server,
            effects,
            opened: None,
        })
    }

    fn autoindex(&self) -> bool {
        self.server.config().http_autoindex.get()
    }

    #[must_use]
    pub fn status(&self) -> i32 {
        self.context.observe().status
    }

    #[must_use]
    pub fn filesize(&self) -> usize {
        self.context.observe().filesize
    }

    #[must_use]
    pub fn content_length(&self) -> usize {
        self.context.observe().content_length
    }

    #[must_use]
    pub fn filename(&self) -> Vec<u8> {
        self.context.filename().to_vec()
    }

    #[must_use]
    pub fn end_part(&self) -> Vec<u8> {
        self.context.end_part().to_vec()
    }

    /// No index file was found, `http_autoindex` is on and the path is a directory.
    #[must_use]
    pub fn lists_directory(&self) -> bool {
        let value = self.context.observe();
        self.context.index_file().is_empty()
            && self.autoindex()
            && value.file_ready
            && value.kind == StaticFileKind::Directory
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
        http_date(Some(self.context.mtime()))
    }

    #[must_use]
    pub fn directory_redirect(&self) -> Option<Vec<u8>> {
        self.context.directory_redirect()
    }

    #[must_use]
    pub fn etag(&self) -> String {
        self.context.observe().etag()
    }

    #[must_use]
    pub fn preconditions(
        &self,
        if_match: &[u8],
        unmodified: &[u8],
        none_match: &[u8],
        modified: &[u8],
    ) -> swoole_rs_http::PreconditionStatus {
        self.context.observe().preconditions(
            if_match,
            unmodified,
            none_match,
            modified,
            &self.effects,
        )
    }

    /// The file changed since `date` (an `If-Modified-Since`).
    #[must_use]
    pub fn is_modified(&self, date: &[u8]) -> bool {
        let mtime = self.context.mtime();
        swoole_rs_http::conditional(
            date,
            swoole_rs_http::Conditional::ModifiedSince,
            mtime,
            &self.effects,
        )
    }

    /// Whether the static handler answers the request.
    #[must_use]
    pub fn try_serve(&mut self) -> bool {
        if self.server.static_rewrite(&mut self.context).is_err() {
            return true; // The static context has prepared a 404.
        }
        let root = self.server.document_root();
        let has_index = self
            .server
            .static_strings(crate::StaticCollection::IndexFiles, |enabled, files| {
                enabled && !files.is_empty()
            });
        let autoindex = self.autoindex();
        self.server
            .static_strings(crate::StaticCollection::Locations, |_, locations| {
                self.context
                    .try_serve(&root, locations, has_index, autoindex, &self.effects)
            })
    }

    /// With `http_index_files`, the directory's first index file becomes the
    /// file. False to stop serving.
    #[must_use]
    pub fn try_serve_index_file(&mut self) -> bool {
        let autoindex = self.autoindex();
        self.server
            .static_strings(crate::StaticCollection::IndexFiles, |enabled, files| {
                self.context.try_serve_index_file(
                    if enabled { files } else { &[] },
                    autoindex,
                    &self.effects,
                )
            })
    }

    pub(crate) fn open_file(&mut self) -> bool {
        if self.context.observe().kind == StaticFileKind::Directory {
            return true;
        }
        self.opened = self.context.open_file();
        self.opened.is_some()
    }

    pub(crate) fn file(
        &self,
    ) -> Option<&std::sync::Arc<swoole_rs_corebase::ossupport::FileRecord>> {
        self.opened.as_ref()
    }

    /// The directory's listing page.
    #[must_use]
    pub fn index_page(&mut self) -> Vec<u8> {
        self.context.index_page(CSS, POWER_BY, &self.effects)
    }

    /// The `Range` and `If-Range` headers into the response's status and tasks.
    pub fn parse_range(&mut self, range: Option<&[u8]>, if_range: Option<&[u8]>) {
        self.context.parse_range(range, if_range, &self.effects);
    }

    /// The response's byte ranges.
    #[must_use]
    pub fn tasks(&self) -> Vec<HttpStaticTask> {
        self.context.tasks().to_vec()
    }

    /// The response's `Content-Type`: multipart with several ranges, else the file's.
    #[must_use]
    pub fn content_type(&mut self) -> Vec<u8> {
        self.context.content_type(&self.effects)
    }
}

#[cfg(test)]
mod tests;
