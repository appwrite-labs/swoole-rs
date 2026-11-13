//! Upstream `StaticHandler::try_serve_index_file` and `make_index_page`.
use super::{HttpStaticContext, StaticEffects, StaticText::Index};
use crate::index_page::{HttpIndexPage, index_page};
use crate::static_path::{DIRECTORY, FILE, static_index_path};

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
        if files.is_empty() || self.facts.file_ready == 0 || self.facts.kind & DIRECTORY == 0 {
            return true;
        }
        if self.directory(effects) != 1 {
            return false;
        }
        // `swoole::intersection`: the first configured name the directory holds.
        let mut found = None;
        for name in files {
            let name = name.as_ref();
            match self.directory.contains(name) {
                Ok(true) => {
                    found = Some(name);
                    break;
                }
                Ok(false) => {}
                Err(_) => return false,
            }
        }
        match found {
            Some(name) => {
                if self.publish(Index, name) < 0 {
                    return false;
                }
            }
            None => self.index.clear(),
        }
        if self.index.is_empty() {
            autoindex
        } else {
            self.set_filename(effects)
        }
    }
    /// Upstream's `set_filename(index_file)`: the index file appended to the path,
    /// which must stat as a regular file.
    fn set_filename(&mut self, effects: &impl StaticEffects) -> bool {
        if static_index_path(
            &mut self.filename,
            self.capacity,
            self.path.filename_length,
            &self.index,
        ) != 0
        {
            return false;
        }
        self.stat(effects) == 1 && self.facts.kind & FILE != 0
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
        self.directory(effects);
        let mut page = Vec::new();
        if self.ensure_directory_slash() < 0 {
            return page;
        }
        let names = (0..self.directory.len()).filter_map(|index| self.directory.name(index));
        let parts = core::iter::once((0, &[][..]))
            .chain(names.map(|name| (1, name)))
            .chain(core::iter::once((2, &[][..])));
        for (part, entry) in parts {
            let text = HttpIndexPage {
                path: &self.directory_path,
                entry,
                css,
                powered,
            };
            index_page(&mut page, part, &text);
        }
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
                    DIRECTORY
                } else {
                    FILE
                },
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
            0
        );
        assert_eq!(owner.stat(effects), 1);
        owner
    }
    fn files(names: &[&[u8]]) -> Vec<Vec<u8>> {
        names.iter().map(|name| name.to_vec()).collect()
    }

    #[test]
    fn directory_listing_is_kept_sorted_and_unique() {
        let effects = listing(&[b"z", b"index.html", b"z", b"a"]);
        let mut owner = directory(&effects);
        assert_eq!(owner.directory(&effects), 1);
        assert_eq!(owner.directory.len(), 3);
        assert_eq!(owner.directory.name(0), Some(b"a".as_slice()));
        assert_eq!(owner.directory(&effects), 1);
        assert_eq!(*effects.lists.borrow(), 1);
        assert!(owner.try_serve_index_file(&files(&[b"missing", b"z"]), false, &effects));
        assert_eq!(owner.index, b"z");
        assert_eq!(
            owner.text(super::super::StaticText::Filename),
            b"/root/files/z"
        );
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
        let vec3: &[&[u8]] = &["a".as_bytes(), "zh中".as_bytes(), "、r\n".as_bytes()];
        assert_eq!(intersection(vec1, vec3), b"");
    }

    #[test]
    fn no_match_clears_old_index_and_answers_autoindex() {
        let effects = listing(&[b"other"]);
        let mut owner = directory(&effects);
        owner.index = b"stale".to_vec();
        assert!(owner.try_serve_index_file(&files(&[b"absent"]), true, &effects));
        assert_eq!(owner.index, b"");
        assert!(!owner.try_serve_index_file(&files(&[b"absent"]), false, &effects));
    }

    #[test]
    fn matching_empty_name_stops_before_later_candidate_and_observes_autoindex() {
        let effects = listing(&[b"", b"index.html"]);
        let mut owner = directory(&effects);
        assert!(!owner.try_serve_index_file(&files(&[b"", b"index.html"]), false, &effects));
        assert_eq!(owner.index, b"");
    }

    #[test]
    fn empty_listing_relists_but_unconfigured_index_does_not_change_existing_name() {
        let effects = listing(&[]);
        let mut owner = directory(&effects);
        assert_eq!(owner.directory(&effects), 1);
        assert_eq!(owner.directory(&effects), 1);
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
            \t\t<li><a href=/normal/a>a</a></li>\n\t\t<li><a href=/normal/b>b</a></li>\n\
            \t</ul>\npowered</body>\n</html>\n";
        assert_eq!(owner.index_page(b"css", b"powered", &effects), expected);
        assert_eq!(owner.index_page(b"css", b"powered", &effects), expected);
        assert_eq!(owner.directory_path, b"/normal/");
    }
}
