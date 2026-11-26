//! The static handler's effects, and `try_serve` and `get_dir_files` over them.
use super::serve::Event;
use super::{BOUNDARY_SIZE, HttpStaticContext, HttpStaticObservation, INVALID, StaticFileKind};
use crate::static_path::Action;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use swoole_rs_corebase::CCast;

/// `PATH_MAX`: the longest path upstream's handler copies before an effect.
const PATH_MAX: usize = libc::PATH_MAX as usize;

impl From<fs::Metadata> for HttpStaticObservation {
    fn from(meta: fs::Metadata) -> Self {
        let kind = meta.file_type();
        // The native `off_t` to `size_t` conversion, on narrow targets too.
        let filesize = meta.size().c_cast::<usize>();
        Self {
            filesize,
            mtime: meta.mtime(),
            mtime_nsec: meta.mtime_nsec(),
            kind: if kind.is_dir() {
                StaticFileKind::Directory
            } else if kind.is_file() {
                StaticFileKind::File
            } else if kind.is_symlink() {
                StaticFileKind::Symlink
            } else {
                StaticFileKind::Other
            },
            ..HttpStaticObservation::default()
        }
    }
}

/// What a static handler asks of the world, in the pinned handler's order. The
/// filesystem defaults to the process's own through `std::fs`.
pub trait StaticEffects {
    /// `lstat(path)`: the size, modification time and file kind of `path`, or None.
    fn stat(&self, path: &[u8]) -> Option<HttpStaticObservation> {
        let meta = fs::symlink_metadata(OsStr::from_bytes(path)).ok()?;
        Some(meta.into())
    }
    /// `stat(path)` for an index candidate, following symbolic links.
    fn index_stat(&self, path: &[u8]) -> Option<HttpStaticObservation> {
        fs::metadata(OsStr::from_bytes(path)).ok().map(Into::into)
    }
    /// `realpath(path)`, or None.
    fn canonical(&self, path: &[u8]) -> Option<Vec<u8>> {
        let path = fs::canonicalize(OsStr::from_bytes(path)).ok()?;
        Some(path.into_os_string().into_vec())
    }
    /// `opendir`, `readdir` to the end and `closedir`: every name in the directory,
    /// `.` and `..` included, or None when it cannot be opened.
    fn list(&self, path: &[u8]) -> Option<Vec<Vec<u8>>> {
        let entries = fs::read_dir(OsStr::from_bytes(path)).ok()?;
        let names = entries
            .map_while(Result::ok)
            .map(|entry| entry.file_name().into_vec());
        Some(
            [b".".to_vec(), b"..".to_vec()]
                .into_iter()
                .chain(names)
                .collect(),
        )
    }
    /// `mime_type::get(filename)`.
    fn mime(&self, filename: &[u8]) -> Vec<u8>;
    /// `mime_type::exists(filename)`.
    fn mime_exists(&self, filename: &[u8]) -> bool;
    /// `swoole_random_string(out, count)`: `count` (below `BOUNDARY_SIZE`) random
    /// characters and a NUL.
    fn random(&self, out: &mut [u8; BOUNDARY_SIZE], count: usize);
    /// `is_modified_range(if_range)`: the `If-Range` date differs from the file's
    /// modification time `mtime`.
    fn modified_range(&self, if_range: &[u8], mtime: i64) -> bool;
    /// A quoted If-Range validator must match the file's strong entity tag exactly.
    fn if_range_changed(&self, value: &[u8], file: &HttpStaticObservation) -> bool {
        if value.starts_with(b"\"") {
            value != file.etag().as_bytes()
        } else {
            self.modified_range(value, file.mtime)
        }
    }
    /// A range part's header overflowed libc's int: `errno = EOVERFLOW`.
    fn overflow(&self);
}

impl HttpStaticContext {
    /// The filename an effect is asked about, when upstream's `PATH_MAX` copy holds it.
    fn effect_path(&self) -> Option<&[u8]> {
        Some(self.filename()).filter(|path| path.len() < PATH_MAX)
    }
    /// `lstat` the filename, admitting the facts on success: whether it exists, or
    /// the error when the path is too long to ask about.
    ///
    /// # Errors
    /// `INVALID` when the filename doesn't fit upstream's `PATH_MAX` copy.
    pub fn stat(&mut self, effects: &impl StaticEffects) -> Result<bool, i32> {
        let path = self.effect_path().ok_or(INVALID)?;
        let Some(facts) = effects.stat(path) else {
            return Ok(false);
        };
        self.observed(&facts);
        Ok(true)
    }
    /// Upstream's `try_serve`: whether the URL names a file under `root` to serve,
    /// within one of `locations` when there are any.
    pub fn try_serve(
        &mut self,
        root: &[u8],
        locations: &[impl AsRef<[u8]>],
        has_index: bool,
        autoindex: bool,
        effects: &impl StaticEffects,
    ) -> bool {
        let begin = Event::Begin {
            has_locations: !locations.is_empty(),
            has_index,
            autoindex,
        };
        let mut action = self.serve_step(root, &begin);
        loop {
            action = match action {
                Ok(Action::Done(served)) => return served,
                Ok(Action::Locations) => {
                    for location in locations {
                        if self.location(location.as_ref()).is_err() {
                            break;
                        }
                    }
                    self.serve_step(root, &Event::Locations)
                }
                Ok(Action::Lstat) => {
                    let Ok(exists) = self.stat(effects) else {
                        return false;
                    };
                    self.serve_step(root, &Event::Stat(exists))
                }
                Ok(Action::Realpath) => {
                    let Some(path) = self.effect_path() else {
                        return false;
                    };
                    let canonical = effects.canonical(path);
                    self.serve_step(root, &Event::Canonical(canonical.as_deref()))
                }
                Ok(Action::Mime) => {
                    let filename = self.filename();
                    let exists = effects.mime_exists(filename);
                    self.serve_step(root, &Event::Mime(exists))
                }
                _ => return false,
            };
        }
    }
    /// Upstream's `get_dir_files`: list the directory once. True when listed (or
    /// already listed); false when it is not a directory or cannot be listed.
    pub fn get_dir_files(&mut self, effects: &impl StaticEffects) -> bool {
        if !self.directory.is_empty() {
            return true;
        }
        if !self.facts.file_ready || self.facts.kind != StaticFileKind::Directory {
            return false;
        }
        let Some(mut names) = self.effect_path().and_then(|path| effects.list(path)) else {
            return false;
        };
        names.sort_unstable();
        names.dedup();
        self.directory = names;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::StaticText::Filename;
    use super::*;

    struct Native;
    impl StaticEffects for Native {
        fn mime(&self, _: &[u8]) -> Vec<u8> {
            Vec::new()
        }
        fn mime_exists(&self, _: &[u8]) -> bool {
            false
        }
        fn random(&self, _: &mut [u8; BOUNDARY_SIZE], _: usize) {}
        fn modified_range(&self, _: &[u8], _: i64) -> bool {
            false
        }
        fn overflow(&self) {}
    }

    #[test]
    fn set_filename_rejects_path_too_long() {
        let base = vec![b'a'; PATH_MAX - b"/index.html".len()];
        let mut owner = HttpStaticContext::new(b"/", PATH_MAX).unwrap();
        owner.publish(Filename, &base).unwrap();
        owner.observed(&HttpStaticObservation {
            kind: StaticFileKind::Directory,
            ..Default::default()
        });
        assert!(!owner.try_serve_index_file(&[b"index.html"], false, &Native));
        assert_eq!(owner.filename(), base);
    }

    #[test]
    fn set_filename_accepts_path_with_room_for_null() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("http-index-file-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("index.html");
        fs::write(&file, b"ok").unwrap();
        let mut owner = HttpStaticContext::new(b"/", PATH_MAX).unwrap();
        owner
            .publish(Filename, root.as_os_str().as_bytes())
            .unwrap();
        assert_eq!(owner.stat(&Native), Ok(true));
        assert!(owner.try_serve_index_file(&[b"index.html"], false, &Native));
        assert_eq!(owner.filename(), file.as_os_str().as_bytes());
        assert_eq!(owner.observe().filesize, 2);
        fs::remove_file(file).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn production_filesystem_keeps_lstat_symlinks_directory_cache_and_failed_stat_facts() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("http-static-fs-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("index.html"), b"payload").unwrap();
        std::os::unix::fs::symlink("index.html", root.join("link")).unwrap();
        let mut owner = HttpStaticContext::new(b"", PATH_MAX).unwrap();
        let set = |owner: &mut HttpStaticContext, path: &std::path::Path| {
            assert_eq!(owner.publish(Filename, path.as_os_str().as_bytes()), Ok(()));
        };
        set(&mut owner, &root.join("index.html"));
        assert_eq!(owner.stat(&Native), Ok(true));
        assert_eq!(owner.facts.filesize, 7);
        assert_eq!(owner.facts.kind as u32, crate::static_path::FILE);
        let mtime = owner.facts.mtime;
        set(&mut owner, &root.join("absent"));
        assert_eq!(owner.stat(&Native), Ok(false));
        assert_eq!(owner.facts.mtime, mtime);
        set(&mut owner, &root.join("link"));
        assert_eq!(owner.stat(&Native), Ok(true));
        assert_eq!(owner.facts.kind as u32, crate::static_path::LINK);
        assert_eq!(
            Native.canonical(owner.effect_path().unwrap()).unwrap(),
            std::fs::canonicalize(root.join("index.html"))
                .unwrap()
                .as_os_str()
                .as_bytes()
        );
        set(&mut owner, &root);
        assert_eq!(owner.stat(&Native), Ok(true));
        assert!(owner.get_dir_files(&Native));
        for name in [b".".as_slice(), b"..", b"index.html", b"link"] {
            assert!(owner.directory.iter().any(|entry| entry == name));
        }
        assert_eq!(owner.directory.len(), 4);
        assert!(owner.try_serve_index_file(&[b"absent".as_slice(), b"link"], false, &Native));
        assert_eq!(owner.filename(), root.join("link").as_os_str().as_bytes());
        assert_eq!(owner.facts.kind, StaticFileKind::File);
        assert_eq!(owner.facts.filesize, 7);
        std::fs::remove_dir_all(&root).unwrap();
        // Existing nonempty cache avoids reopening even after removal.
        assert!(owner.get_dir_files(&Native));
        assert_eq!(Native.list(root.as_os_str().as_bytes()), None);
    }
}
