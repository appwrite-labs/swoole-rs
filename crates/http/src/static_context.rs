//! Upstream `http_server::StaticHandler`'s state: the request URL, the paths and texts
//! derived from it, the file's facts, the directory listing and the range tasks. The
//! filesystem, MIME, random and calendar effects are the caller's [`StaticEffects`].
use crate::static_meta::joined;
use crate::static_path::{self, Action};
use crate::{HttpStaticPath, HttpStaticTask, HttpTasks, INVALID, OVERFLOW};
mod filesystem;
pub use filesystem::StaticEffects;
mod range;
mod selection;
mod serve;

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
pub use crate::static_meta::BOUNDARY_SIZE;
use StaticText::{Boundary, Content, Directory, End, Filename, Index, Original, Url};

/// The mutually exclusive file kinds returned by `lstat`, as path-policy bits.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum StaticFileKind {
    #[default]
    Other = 0,
    Symlink = static_path::LINK,
    Directory = static_path::DIRECTORY,
    File = static_path::FILE,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct HttpStaticObservation {
    pub filesize: usize,
    pub mtime: i64,
    pub content_length: usize,
    pub directory_count: usize,
    pub task_count: usize,
    pub status: i32,
    pub kind: StaticFileKind,
    pub file_ready: bool,
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
    directory: Vec<Vec<u8>>,
    tasks: HttpTasks,
}
impl HttpStaticContext {
    /// A context for `url` whose filename may hold `capacity` bytes, the native
    /// `PATH_MAX`. None for a zero or oversized capacity, or when allocation fails.
    #[must_use]
    pub fn new(url: &[u8], capacity: usize) -> Option<Self> {
        if capacity == 0 || capacity > isize::MAX as usize {
            return None;
        }
        let original = joined(&[url]).ok()?;
        Some(Self {
            original,
            rewritten: None,
            directory_path: Vec::new(),
            index: Vec::new(),
            filename: Vec::new(),
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
            directory: Vec::new(),
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
            self.directory_path = joined(&[url])?;
        }
        result
    }
    /// A configured location: the path is the request's last when the URL starts with
    /// it. `Err` outside the locations step.
    fn location(&mut self, location: &[u8]) -> Result<bool, i32> {
        let url = self.rewritten.as_ref().unwrap_or(&self.original);
        crate::static_path::location_match(url, location, &mut self.path)
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
    fn publish(&mut self, kind: StaticText, text: &[u8]) -> Result<(), i32> {
        if kind == Filename {
            let (nul, head) =
                crate::static_path::span(&mut self.filename, self.capacity, 0, text.len() + 1)
                    .and_then(<[u8]>::split_last_mut)
                    .ok_or(OVERFLOW)?;
            head.copy_from_slice(text);
            *nul = 0;
            self.path.filename_length = text.len();
            return Ok(());
        }
        let target = match kind {
            Url => {
                self.rewritten = Some(joined(&[text])?);
                return Ok(());
            }
            Directory => &mut self.directory_path,
            Index => &mut self.index,
            Boundary => &mut self.boundary,
            Original | Filename | Content | End => return Err(INVALID),
        };
        *target = joined(&[text])?;
        Ok(())
    }
    /// Admit the random bytes `static_boundary` asked for into an empty boundary,
    /// after the fixed prefix.
    fn set_boundary(&mut self, random: &[u8]) -> Result<(), i32> {
        if !self.boundary.is_empty() {
            return Ok(());
        }
        let prefix = crate::static_meta::BOUNDARY;
        if crate::static_meta::static_boundary(self.boundary.len()) != Some(random.len()) {
            return Err(INVALID);
        }
        self.boundary = joined(&[prefix, random])?;
        Ok(())
    }
    /// Admit a stat's facts.
    fn observed(&mut self, input: &HttpStaticObservation) {
        self.facts.filesize = input.filesize;
        self.facts.mtime = input.mtime;
        self.facts.kind = input.kind;
        self.facts.file_ready = true;
    }
    /// The multipart content type over the boundary.
    fn format_content(&mut self) -> Result<(), i32> {
        self.content = crate::static_meta::static_content_type(&self.boundary)?;
        Ok(())
    }
    /// The multipart body's closing boundary.
    fn format_end(&mut self) -> Result<(), i32> {
        self.end = joined(&crate::range_response::closing_boundary(&self.boundary))?;
        Ok(())
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
        assert!(rules.add(b"/a\0", b"/b\0").is_ok());
        assert!(rules.add(b"/b\0tail", b"").is_ok());
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
        assert_eq!(owner.publish(Boundary, &boundary), Ok(()));
        boundary.fill(b'x');
        assert_eq!(owner.format_content(), Ok(()));
        assert_eq!(owner.format_end(), Ok(()));
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
        assert_eq!(owner.publish(Url, b"/second\0tail"), Ok(()));
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
        assert_eq!(owner.publish(Index, b"index.html"), Ok(()));
        let base = owner.path.filename_length;
        let index_path = |owner: &mut HttpStaticContext| {
            crate::static_path::static_index_path(
                &mut owner.filename,
                owner.capacity,
                owner.path.filename_length,
                &owner.index,
            )
        };
        assert_eq!(index_path(&mut owner), Ok(()));
        assert_eq!(owner.path.filename_length, base);
        assert_eq!(owner.text(Filename), b"/root/files/index.html");
        assert_eq!(owner.publish(Index, b"other.html"), Ok(()));
        assert_eq!(index_path(&mut owner), Ok(()));
        // The pinned helper observes slash at the unchanged base offset,
        // then overwrites from that offset on a repeated append.
        assert_eq!(owner.text(Filename), b"/root/filesother.html");
    }
    #[test]
    fn initialized_facts_and_nested_stores_have_one_custodian() {
        let mut owner = make();
        let facts = owner.observe();
        assert_eq!(facts.status, 200);
        assert_eq!(u32::from(facts.file_ready), 0);
        owner.directory = vec![b"a".to_vec(), b"z".to_vec()];
        owner.tasks.begin();
        owner.tasks.prepare_full(17, 23);
        assert_eq!(owner.tasks.push(), Ok(()));
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
        assert_eq!(owner.set_boundary(&random[..23]), Err(INVALID));
        assert_eq!(owner.set_boundary(&random), Ok(()));
        random.fill(b'x');
        assert_eq!(
            owner.text(Boundary),
            b"SwooleBoundaryABCDEFGHIJKLMNOPQRSTUVWX"
        );
        assert_eq!(owner.set_boundary(&random), Ok(()));
        assert_eq!(
            owner.text(Boundary),
            b"SwooleBoundaryABCDEFGHIJKLMNOPQRSTUVWX"
        );
    }
}
