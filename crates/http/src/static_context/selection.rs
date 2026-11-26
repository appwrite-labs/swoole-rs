//! Upstream `StaticHandler::try_serve_index_file` and `make_index_page`.
use super::{HttpStaticContext, StaticEffects, StaticFileKind};
use crate::index_page::index_page;
use crate::static_meta::{append_parts, joined};

impl HttpStaticContext {
    /// Upstream's `try_serve_index_file`: with index `files` configured and a
    /// directory, the first of them the directory holds becomes the file; without
    /// one, serve only with `autoindex`. False to stop serving.
    pub fn try_serve_index_file(
        &mut self,
        files: &[impl AsRef<[u8]>],
        autoindex: bool,
        effects: &impl StaticEffects,
    ) -> bool {
        if files.is_empty()
            || !self.facts.file_ready
            || self.facts.kind != StaticFileKind::Directory
        {
            return true;
        }
        files
            .iter()
            .any(|name| self.set_filename(name.as_ref(), effects))
            || autoindex
    }

    fn set_filename(&mut self, name: &[u8], effects: &impl StaticEffects) -> bool {
        let Some(base) = self.filename.get(..self.path.filename_length) else {
            return false;
        };
        let separator = if base.ends_with(b"/") {
            b"".as_slice()
        } else {
            b"/"
        };
        let mut candidate = Vec::new();
        if append_parts(
            &mut candidate,
            &[base, separator, name, b"\0"],
            self.capacity.min(libc::PATH_MAX as usize),
        )
        .is_err()
        {
            return false;
        }
        let Some(file) = effects.index_stat(crate::c_text(&candidate)) else {
            return false;
        };
        if file.kind != StaticFileKind::File {
            return false;
        }
        let Ok(index) = joined(&[name]) else {
            return false;
        };
        self.path.filename_length = candidate.len() - 1;
        self.filename = candidate;
        self.index = index;
        self.observed(&file);
        true
    }
    /// Upstream's `make_index_page`: the directory's listing, `css` in its head and
    /// `powered` in its foot.
    pub fn index_page(
        &mut self,
        css: &[u8],
        powered: &[u8],
        effects: &impl StaticEffects,
    ) -> Vec<u8> {
        // Upstream pages whatever the listing's outcome.
        self.get_dir_files(effects);
        let mut page = Vec::new();
        if !self.directory_path.ends_with(b"/")
            && crate::static_meta::append_parts(&mut self.directory_path, &[b"/"], 1).is_err()
        {
            return page;
        }
        index_page(
            &mut page,
            &self.directory_path,
            self.directory.iter().map(Vec::as_slice),
            css,
            powered,
        );
        page
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BOUNDARY_SIZE;
    use core::cell::RefCell;

    /// A directory listing `names`; any other path is a file.
    struct Listing {
        names: Vec<Vec<u8>>,
        lists: RefCell<usize>,
    }
    impl StaticEffects for Listing {
        fn stat(&self, path: &[u8]) -> Option<crate::HttpStaticObservation> {
            Some(crate::HttpStaticObservation {
                kind: if path.ends_with(b"/files") {
                    StaticFileKind::Directory
                } else {
                    StaticFileKind::File
                },
                ..Default::default()
            })
        }
        fn index_stat(&self, path: &[u8]) -> Option<crate::HttpStaticObservation> {
            let name = path.strip_prefix(b"/root/files/")?;
            if name.is_empty() {
                return Some(crate::HttpStaticObservation {
                    kind: StaticFileKind::Directory,
                    ..Default::default()
                });
            }
            self.names
                .iter()
                .any(|entry| entry == name)
                .then(|| crate::HttpStaticObservation {
                    kind: StaticFileKind::File,
                    ..Default::default()
                })
        }
        fn list(&self, _: &[u8]) -> Option<Vec<Vec<u8>>> {
            *self.lists.borrow_mut() += 1;
            Some(self.names.clone())
        }
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
    fn listing(names: &[&[u8]]) -> Listing {
        Listing {
            names: names.iter().map(|name| name.to_vec()).collect(),
            lists: RefCell::new(0),
        }
    }
    /// A context on the `/root/files` directory.
    fn directory(effects: &Listing) -> HttpStaticContext {
        let mut owner = HttpStaticContext::new(b"/files", 4096).unwrap();
        assert_eq!(
            owner.publish(super::super::StaticText::Filename, b"/root/files"),
            Ok(())
        );
        assert_eq!(owner.stat(effects), Ok(true));
        owner
    }
    fn files(names: &[&[u8]]) -> Vec<Vec<u8>> {
        names.iter().map(|name| name.to_vec()).collect()
    }

    #[test]
    fn directory_listing_is_kept_sorted_and_unique() {
        let effects = listing(&[b"z", b"index.html", b"z", b"a"]);
        let mut owner = directory(&effects);
        assert!(owner.get_dir_files(&effects));
        assert_eq!(owner.directory.len(), 3);
        assert_eq!(
            owner.directory.first().map(Vec::as_slice),
            Some(b"a".as_slice())
        );
        assert!(owner.get_dir_files(&effects));
        assert_eq!(*effects.lists.borrow(), 1);
        assert!(owner.try_serve_index_file(&files(&[b"missing", b"z"]), false, &effects));
        assert_eq!(owner.index, b"z");
        assert_eq!(
            owner.text(super::super::StaticText::Filename),
            b"/root/files/z"
        );
    }

    #[test]
    fn large_directory_listing_keeps_sorted_unique_owned_names() {
        let expected: Vec<_> = (0..4096)
            .map(|i| format!("entry-{i:04}-é").into_bytes())
            .collect();
        let effects = Listing {
            names: expected
                .iter()
                .rev()
                .flat_map(|name| [name.clone(), name.clone()])
                .collect(),
            lists: RefCell::new(0),
        };
        let mut owner = directory(&effects);
        assert!(owner.get_dir_files(&effects));
        drop(effects);
        assert_eq!(owner.directory, expected);
    }

    /// Upstream `TEST(base, intersection)`: the first configured index file the directory
    /// holds, in configured order, or none. `try_serve_index_file` does
    /// `swoole::intersection`'s job.
    #[test]
    fn base_intersection() {
        fn intersection(candidates: &[&[u8]], names: &[&[u8]]) -> Vec<u8> {
            let effects = listing(names);
            let mut owner = directory(&effects);
            owner.try_serve_index_file(&files(candidates), true, &effects);
            owner.index.clone()
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
        let vec3: &[&[u8]] = &[b"a", "zh中".as_bytes(), "、r\n".as_bytes()];
        assert_eq!(intersection(vec1, vec3), b"");
    }

    #[test]
    fn no_match_preserves_directory_and_answers_autoindex() {
        let effects = listing(&[b"other"]);
        let mut owner = directory(&effects);
        let base = owner.filename().to_vec();
        assert!(owner.try_serve_index_file(&files(&[b"absent"]), true, &effects));
        assert_eq!(owner.filename(), base);
        assert_eq!(*effects.lists.borrow(), 0);
        assert!(!owner.try_serve_index_file(&files(&[b"absent"]), false, &effects));
    }

    #[test]
    fn directory_candidate_is_skipped_before_a_regular_file() {
        let effects = listing(&[b"", b"index.html"]);
        let mut owner = directory(&effects);
        assert!(owner.try_serve_index_file(&files(&[b"", b"index.html"]), false, &effects));
        assert_eq!(owner.index, b"index.html");
        assert_eq!(owner.path.filename_length, owner.filename().len());
        assert_eq!(*effects.lists.borrow(), 0);
    }

    #[test]
    fn empty_listing_relists_but_unconfigured_index_does_not_change_existing_name() {
        let effects = listing(&[]);
        let mut owner = directory(&effects);
        assert!(owner.get_dir_files(&effects));
        assert!(owner.get_dir_files(&effects));
        assert_eq!(*effects.lists.borrow(), 2);
        owner.index = b"existing".to_vec();
        let none: &[Vec<u8>] = &[];
        assert!(owner.try_serve_index_file(none, false, &effects));
        assert_eq!(owner.index, b"existing");
    }

    #[test]
    fn page_has_header_entries_footer_and_one_slash() {
        let effects = listing(&[b"b", b"a", b"a", b"."]);
        let mut owner = directory(&effects);
        owner.directory_path = b"/normal".to_vec();
        let expected: &[u8] = b"<html>\n<head>\n\t<meta charset='UTF-8'>\n<title>Index of /normal/</title></head>\n<body>\ncss<h1>Index of /normal/</h1>\t<ul>\n\
            \t\t<li><a href=\"/normal/a\">a</a></li>\n\t\t<li><a href=\"/normal/b\">b</a></li>\n\
            \t</ul>\npowered</body>\n</html>\n";
        assert_eq!(owner.index_page(b"css", b"powered", &effects), expected);
        assert_eq!(owner.index_page(b"css", b"powered", &effects), expected);
        assert_eq!(owner.directory_path, b"/normal/");
    }
}
