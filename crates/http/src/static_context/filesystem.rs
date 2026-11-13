//! The static handler's effects, and `try_serve` and `get_dir_files` over them.
use super::serve::Event;
use super::{BOUNDARY_SIZE, HttpStaticContext, HttpStaticObservation, INVALID};
use crate::static_path::{DIRECTORY, FILE, LINK};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;

/// `PATH_MAX`: the longest path upstream's handler copies before an effect.
const PATH_MAX: usize = libc::PATH_MAX as usize;

/// What a static handler asks of the world, in the pinned handler's order. The
/// filesystem defaults to the process's own through `std::fs`.
pub trait StaticEffects {
    /// `lstat(path)`: the size, modification time and kind (link8, directory16,
    /// file32) of `path`, or None.
    fn stat(&self, path: &[u8]) -> Option<HttpStaticObservation> {
        let meta = fs::symlink_metadata(OsStr::from_bytes(path)).ok()?;
        let kind = meta.file_type();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "Preserve native off_t to size_t conversion on narrow targets"
        )]
        let filesize = meta.size() as usize;
        Some(HttpStaticObservation {
            filesize,
            mtime: meta.mtime(),
            kind: if kind.is_dir() {
                DIRECTORY
            } else if kind.is_file() {
                FILE
            } else if kind.is_symlink() {
                LINK
            } else {
                0
            },
            ..HttpStaticObservation::default()
        })
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
    /// A range part's header overflowed libc's int: `errno = EOVERFLOW`.
    fn overflow(&self);
}

impl HttpStaticContext {
    /// The filename an effect is asked about, when upstream's `PATH_MAX` copy holds it.
    fn effect_path(&self) -> Option<&[u8]> {
        Some(self.filename()).filter(|path| path.len() < PATH_MAX)
    }
    /// `lstat` the filename, admitting the facts on success: 1, else 0, or negative.
    pub fn stat(&mut self, effects: &impl StaticEffects) -> i32 {
        let Some(path) = self.effect_path() else {
            return INVALID;
        };
        let Some(facts) = effects.stat(path) else {
            return 0;
        };
        self.observed(&facts);
        1
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
                0 => return false,
                1 => return true,
                2 => {
                    for location in locations {
                        if self.location(location.as_ref()).is_err() {
                            break;
                        }
                    }
                    self.serve_step(root, &Event::Locations)
                }
                4 => {
                    let result = self.stat(effects);
                    if result < 0 {
                        return false;
                    }
                    self.serve_step(root, &Event::Stat(result == 1))
                }
                5 => {
                    let Some(path) = self.effect_path() else {
                        return false;
                    };
                    let canonical = effects.canonical(path);
                    self.serve_step(root, &Event::Canonical(canonical.as_deref()))
                }
                6 => {
                    let filename = self.filename();
                    let exists = effects.mime_exists(filename);
                    self.serve_step(root, &Event::Mime(exists))
                }
                _ => return false,
            };
        }
    }
    /// Upstream's `get_dir_files`: list the directory once. True when listed (or
    /// already listed).
    pub fn get_dir_files(&mut self, effects: &impl StaticEffects) -> bool {
        self.directory(effects) == 1
    }
    /// `get_dir_files`: 1 when listed (or already listed), 0 when not a directory or
    /// it cannot be opened, else negative.
    pub(super) fn directory(&mut self, effects: &impl StaticEffects) -> i32 {
        if !self.directory.is_empty() {
            return 1;
        }
        if self.facts.file_ready == 0 || self.facts.kind & DIRECTORY == 0 {
            return 0;
        }
        let Some(path) = self.effect_path() else {
            return INVALID;
        };
        let Some(names) = effects.list(path) else {
            return 0;
        };
        for name in &names {
            if let Err(error) = self.directory.scan_append(name) {
                return error;
            }
        }
        self.directory.scan_finish();
        1
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
    fn production_filesystem_keeps_lstat_symlinks_directory_cache_and_failed_stat_facts() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("http-static-fs-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("index.html"), b"payload").unwrap();
        std::os::unix::fs::symlink("index.html", root.join("link")).unwrap();
        let mut owner = HttpStaticContext::new(b"", PATH_MAX).unwrap();
        let set = |owner: &mut HttpStaticContext, path: &std::path::Path| {
            assert_eq!(owner.publish(Filename, path.as_os_str().as_bytes()), 0);
        };
        set(&mut owner, &root.join("index.html"));
        assert_eq!(owner.stat(&Native), 1);
        assert_eq!(owner.facts.filesize, 7);
        assert_eq!(owner.facts.kind, FILE);
        let mtime = owner.facts.mtime;
        set(&mut owner, &root.join("absent"));
        assert_eq!(owner.stat(&Native), 0);
        assert_eq!(owner.facts.mtime, mtime);
        set(&mut owner, &root.join("link"));
        assert_eq!(owner.stat(&Native), 1);
        assert_eq!(owner.facts.kind, LINK);
        assert_eq!(
            Native.canonical(owner.effect_path().unwrap()).unwrap(),
            std::fs::canonicalize(root.join("index.html"))
                .unwrap()
                .as_os_str()
                .as_bytes()
        );
        set(&mut owner, &root);
        assert_eq!(owner.stat(&Native), 1);
        assert_eq!(owner.directory(&Native), 1);
        for name in [b".".as_slice(), b"..", b"index.html", b"link"] {
            assert!(owner.directory.contains(name).unwrap());
        }
        assert_eq!(owner.directory.len(), 4);
        std::fs::remove_dir_all(&root).unwrap();
        // Existing nonempty cache avoids reopening even after removal.
        assert_eq!(owner.directory(&Native), 1);
        assert_eq!(Native.list(root.as_os_str().as_bytes()), None);
    }
}
