//! Upstream `http_server::StaticHandler`'s state: the request URL, the paths and texts
//! derived from it, the file's facts, the directory listing and the range tasks. The
//! filesystem, MIME, random and calendar effects are the caller's [`StaticEffects`].
use crate::range_response::{HttpRangeFormat, range_format};
use crate::static_path::Action;
use crate::{HttpDirectory, HttpStaticPath, HttpStaticTask, HttpTasks, INVALID, OVERFLOW};
mod filesystem;
pub use filesystem::StaticEffects;
mod range;
mod selection;
mod serve;

const ALLOCATION: i32 = -3;
/// One of the context's texts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaticText {
    /// The URL, rewritten or not.
    Url,
    /// The URL as the request gave it.
    Original,
    Directory,
    /// The index file.
    Index,
    /// The filename, to its NUL.
    Filename,
    Boundary,
    /// The multipart content type.
    Content,
    /// The closing boundary.
    End,
}
use StaticText::{Boundary, Content, Directory, End, Filename, Index, Original, Url};
/// `SW_HTTP_SERVER_BOUNDARY_TOTAL_SIZE`: the boundary and its NUL.
pub const BOUNDARY_SIZE: usize = 39;

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
    /// Written up to its NUL, never past `capacity`; see [`crate::static_path::span`].
    filename: Vec<u8>,
    capacity: usize,
    boundary: Vec<u8>,
    content: Vec<u8>,
    end: Vec<u8>,
    path: HttpStaticPath,
    range: range::HttpContextRange,
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
    /// A context for `url` whose filename holds `capacity` bytes, the native
    /// `PATH_MAX`. None for a zero or oversized capacity, or when allocation fails.
    #[must_use]
    pub fn new(url: &[u8], capacity: usize) -> Option<Self> {
        if capacity == 0 || capacity > isize::MAX as usize {
            return None;
        }
        let original = copied(url).ok()?;
        let mut filename = Vec::new();
        filename.try_reserve_exact(capacity).ok()?;
        Some(Self {
            original,
            rewritten: None,
            directory_path: Vec::new(),
            index: Vec::new(),
            filename,
            capacity,
            boundary: Vec::new(),
            content: Vec::new(),
            end: Vec::new(),
            path: HttpStaticPath::default(),
            range: range::HttpContextRange::default(),
            serve: serve::Serve::default(),
            facts: HttpStaticObservation {
                status: 200,
                ..HttpStaticObservation::default()
            },
            directory: HttpDirectory::default(),
            tasks: HttpTasks::default(),
        })
    }
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
    /// The file's modification time, once a stat has succeeded.
    #[must_use]
    pub fn mtime(&self) -> i64 {
        self.facts.mtime
    }
    /// The facts: the file's stat and the response's status, length and task count.
    #[must_use]
    pub fn observe(&self) -> HttpStaticObservation {
        HttpStaticObservation {
            last: self.path.last,
            directory_count: self.directory.len(),
            task_count: self.tasks.count(),
            ..self.facts
        }
    }
    /// The response's byte ranges.
    #[must_use]
    pub fn tasks(&self) -> &[HttpStaticTask] {
        self.tasks.items()
    }
    fn path_step(&mut self, root: &[u8], facts: u32) -> Result<Action, i32> {
        let url = self.rewritten.as_ref().unwrap_or(&self.original);
        let result = crate::static_path::path_step(
            url,
            root,
            &mut self.filename,
            self.capacity,
            &mut self.path,
            facts,
        );
        if result == Ok(Action::Directory) {
            let url = url.get(..self.path.url_length).ok_or(INVALID)?;
            self.directory_path = copied(url)?;
        }
        result
    }
    /// A configured location: the path is the request's last when the URL starts with
    /// it. `Err` outside the locations step.
    fn location(&mut self, location: &[u8]) -> Result<bool, i32> {
        let url = self.rewritten.as_ref().unwrap_or(&self.original);
        crate::static_path::location_match(url, location, &mut self.path)
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
    /// The file to serve, up to its NUL.
    #[must_use]
    pub fn filename(&self) -> &[u8] {
        crate::c_text(&self.filename)
    }
    /// The directory's index file, empty when there is none.
    #[must_use]
    pub fn index_file(&self) -> &[u8] {
        &self.index
    }
    /// The multipart body's closing boundary.
    #[must_use]
    pub fn end_part(&self) -> &[u8] {
        &self.end
    }
    /// One of the context's texts.
    #[must_use]
    pub fn text(&self, kind: StaticText) -> &[u8] {
        match kind {
            Url => self.url(),
            Original => &self.original,
            Directory => &self.directory_path,
            Index => &self.index,
            Filename => crate::c_text(&self.filename),
            Boundary => &self.boundary,
            Content => &self.content,
            End => &self.end,
        }
    }
    fn publish(&mut self, kind: StaticText, text: &[u8]) -> i32 {
        if kind == Filename {
            let Some((nul, head)) =
                crate::static_path::span(&mut self.filename, self.capacity, 0, text.len() + 1)
                    .and_then(<[u8]>::split_last_mut)
            else {
                return OVERFLOW;
            };
            head.copy_from_slice(text);
            *nul = 0;
            self.path.filename_length = text.len();
            return 0;
        }
        let target = match kind {
            Url => {
                let value = match copied(text) {
                    Ok(value) => value,
                    Err(error) => return error,
                };
                self.rewritten = Some(value);
                return 0;
            }
            Directory => &mut self.directory_path,
            Index => &mut self.index,
            Boundary => &mut self.boundary,
            Original | Filename | Content | End => return INVALID,
        };
        let value = match copied(text) {
            Ok(value) => value,
            Err(error) => return error,
        };
        *target = value;
        0
    }
    /// Admit the random bytes `static_boundary` asked for into an empty boundary,
    /// after the fixed prefix.
    fn set_boundary(&mut self, random: &[u8]) -> i32 {
        if !self.boundary.is_empty() {
            return 0;
        }
        let prefix = crate::static_meta::BOUNDARY;
        if random.len() != BOUNDARY_SIZE - prefix.len() {
            return INVALID;
        }
        let mut boundary = Vec::new();
        if boundary
            .try_reserve_exact(prefix.len() - 1 + random.len())
            .is_err()
        {
            return ALLOCATION;
        }
        boundary.extend_from_slice(crate::c_text(prefix));
        boundary.extend_from_slice(random);
        self.boundary = boundary;
        0
    }
    /// Admit a stat's facts.
    fn observed(&mut self, input: &HttpStaticObservation) {
        self.facts.filesize = input.filesize;
        self.facts.mtime = input.mtime;
        self.facts.kind = input.kind;
        self.facts.file_ready = 1;
    }
    /// The multipart content type over the boundary.
    fn format_content(&mut self) -> i32 {
        match crate::static_meta::static_content_type(&self.boundary) {
            Ok(value) => {
                self.content = value;
                0
            }
            Err(error) => error,
        }
    }
    /// The multipart body's closing boundary.
    fn format_end(&mut self) -> i32 {
        let range = HttpRangeFormat {
            boundary: &self.boundary,
            mime: &[],
            offset: 0,
            length: 0,
            filesize: 0,
            part: 1,
            separator: 0,
        };
        let mut length = 0;
        let status = range_format(&range, &mut [], &mut length);
        if status < 0 {
            return status;
        }
        let Some(capacity) = length
            .checked_add(1)
            .filter(|&value| isize::try_from(value).is_ok())
        else {
            return OVERFLOW;
        };
        let mut value = Vec::new();
        if value.try_reserve_exact(capacity).is_err() {
            return ALLOCATION;
        }
        value.resize(capacity, 0);
        let status = range_format(&range, &mut value, &mut length);
        if status < 0 {
            return status;
        }
        value.truncate(length);
        self.end = value;
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn make() -> HttpStaticContext {
        HttpStaticContext::new(b"/files?q=1", 128).unwrap()
    }
    #[test]
    fn rewrite_publishes_over_the_owned_original() {
        let original = b"/a\0tail";
        let mut owner = HttpStaticContext::new(original, 128).unwrap();
        let mut rules = crate::RewriteRules::default();
        rules.add(b"/a\0", b"/b\0");
        rules.add(b"/b\0tail", b"");
        assert!(owner.rewrite(&rules));
        assert_eq!(owner.text(Url), b"/b\0tail");
        assert!(owner.rewrite(&rules));
        assert_eq!(owner.text(Url), b"");
        assert_eq!(owner.text(Original), original);
        assert!(!owner.rewrite(&crate::RewriteRules::default()));
        assert_eq!(owner.text(Url), b"");
    }
    #[test]
    fn content_and_end_render_from_the_owned_binary_boundary() {
        let mut owner = make();
        let mut boundary = b"one\0two".to_vec();
        assert_eq!(owner.publish(Boundary, &boundary), 0);
        boundary.fill(b'x');
        assert_eq!(owner.format_content(), 0);
        assert_eq!(owner.format_end(), 0);
        assert_eq!(
            owner.text(Content),
            b"multipart/byteranges; boundary=one\0two"
        );
        assert_eq!(owner.text(End), b"\r\n--one\0two--\r\n");
    }
    #[test]
    fn owned_original_rewrite_and_copies_are_independent() {
        let mut input = b"/first".to_vec();
        let mut owner = HttpStaticContext::new(&input, 128).unwrap();
        input.fill(b'x');
        assert_eq!(owner.text(Url), b"/first");
        assert_eq!(owner.publish(Url, b"/second\0tail"), 0);
        assert_eq!(owner.text(Url), b"/second\0tail");
        assert_eq!(owner.text(Original), b"/first");
        assert!(HttpStaticContext::new(b"/", 0).is_none());
    }
    #[test]
    fn path_owner_preserves_index_backing_and_separate_base_length() {
        let mut owner = make();
        let root = b"/root";
        assert_eq!(owner.path_step(root, 0), Ok(Action::Locations));
        assert_eq!(owner.path_step(root, 0), Ok(Action::Directory));
        assert_eq!(owner.text(Directory), b"/files");
        assert_eq!(owner.path_step(root, 0), Ok(Action::Lstat));
        assert_eq!(owner.publish(Index, b"index.html"), 0);
        let base = owner.path.filename_length;
        let index_path = |owner: &mut HttpStaticContext| {
            crate::static_path::static_index_path(
                &mut owner.filename,
                owner.capacity,
                owner.path.filename_length,
                &owner.index,
            )
        };
        assert_eq!(index_path(&mut owner), 0);
        assert_eq!(owner.path.filename_length, base);
        assert_eq!(owner.text(Filename), b"/root/files/index.html");
        assert_eq!(owner.publish(Index, b"other.html"), 0);
        assert_eq!(index_path(&mut owner), 0);
        // The pinned helper observes slash at the unchanged base offset,
        // then overwrites from that offset on a repeated append.
        assert_eq!(owner.text(Filename), b"/root/filesother.html");
    }
    #[test]
    fn initialized_facts_and_nested_stores_have_one_custodian() {
        let mut owner = make();
        let facts = owner.observe();
        assert_eq!(facts.status, 200);
        assert_eq!(facts.file_ready, 0);
        for name in [b"z".as_slice(), b"a", b"z"] {
            assert_eq!(owner.directory.scan_append(name), Ok(()));
        }
        owner.directory.scan_finish();
        owner.tasks.begin();
        owner.tasks.prepare_full(17, 23);
        assert_eq!(owner.tasks.push(), 0);
        let facts = owner.observe();
        assert_eq!(facts.directory_count, 2);
        assert_eq!(facts.task_count, 1);
        let task = owner.tasks()[0];
        assert_eq!(task.offset, 17);
        assert_eq!(task.length, 23);
        assert_eq!(task.part_header, [0; 256]);
    }
    #[test]
    fn lazy_boundary_assembly_owns_prefix_and_positive_rng_bytes_once() {
        let mut owner = make();
        let mut random = b"ABCDEFGHIJKLMNOPQRSTUVWX".to_vec();
        assert_eq!(owner.set_boundary(&random[..23]), INVALID);
        assert_eq!(owner.set_boundary(&random), 0);
        random.fill(b'x');
        assert_eq!(
            owner.text(Boundary),
            b"SwooleBoundaryABCDEFGHIJKLMNOPQRSTUVWX"
        );
        assert_eq!(owner.set_boundary(&random), 0);
        assert_eq!(
            owner.text(Boundary),
            b"SwooleBoundaryABCDEFGHIJKLMNOPQRSTUVWX"
        );
    }
}
