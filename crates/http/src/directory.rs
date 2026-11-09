//! Private `StaticHandler` directory owner. No native container or name mirror.
use crate::proxy::HttpProxyText;
use crate::{HttpIndexPage, INVALID, swr_v1_http_index_page};
use std::ptr;

#[derive(Default)]
pub struct HttpDirectory {
    names: Vec<std::ops::Range<usize>>,
    bytes: Vec<u8>,
    pending: bool,
}
const ALLOCATION: i32 = -3;

/// The bytes a stored span names; spans are in range by construction.
fn at<'a>(bytes: &'a [u8], span: &std::ops::Range<usize>) -> &'a [u8] {
    bytes.get(span.clone()).unwrap_or_default()
}

impl HttpDirectory {
    fn name(&self, index: usize) -> Option<&[u8]> {
        self.names.get(index).map(|span| at(&self.bytes, span))
    }
    fn copy_name(&mut self, name: &[u8]) -> Result<std::ops::Range<usize>, i32> {
        self.names.try_reserve(1).map_err(|_| ALLOCATION)?;
        self.bytes.try_reserve(name.len()).map_err(|_| ALLOCATION)?;
        let start = self.bytes.len();
        self.bytes.extend_from_slice(name);
        Ok(start..self.bytes.len())
    }
    /// The names stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
    /// Whether the finished store holds `name`.
    ///
    /// # Errors
    /// `INVALID` while a scan is unfinished.
    pub fn contains(&self, name: &[u8]) -> Result<bool, i32> {
        if self.pending {
            return Err(INVALID);
        }
        Ok(self
            .names
            .binary_search_by(|entry| at(&self.bytes, entry).cmp(name))
            .is_ok())
    }
    /// Appends a copied name to the unfinished scan, which [`Self::scan_finish`]
    /// sorts and deduplicates.
    ///
    /// # Errors
    /// `ALLOCATION` when the copy cannot be stored; no name is published.
    pub fn scan_append(&mut self, name: &[u8]) -> Result<(), i32> {
        let span = self.copy_name(name)?;
        self.names.push(span);
        self.pending = true;
        Ok(())
    }
    /// Finishes the scan in place: unsigned byte sort, then duplicates go.
    pub fn scan_finish(&mut self) {
        if self.pending {
            let bytes = &self.bytes;
            self.names
                .sort_unstable_by(|a, b| at(bytes, a).cmp(at(bytes, b)));
            self.names.dedup_by(|a, b| at(bytes, a) == at(bytes, b));
            self.pending = false;
        }
    }

    /// Render directly from owned ordered name bytes, without exporting a borrowed
    /// pointer or making a persistent native copy. Part0/2 ignore index; part1 uses
    /// the indexed directory entry. Existing page serialization/size policy is shared.
    /// # Safety
    /// The page's spans are initialized; part1 requires a live indexed entry.
    /// Output/written obey `index_page`'s disjointness contract and cannot alias
    /// this owner's storage. Query and fill are separate complete calls.
    pub(crate) unsafe fn page(
        &self,
        index: usize,
        page: &HttpIndexPage,
        output: *mut u8,
        capacity: usize,
        written: *mut usize,
    ) -> i32 {
        let owner = self;
        let entry = if page.part == 1 {
            if owner.pending {
                return INVALID;
            }
            let Some(name) = owner.name(index) else {
                return INVALID;
            };
            HttpProxyText {
                data: name.as_ptr(),
                length: name.len(),
            }
        } else {
            HttpProxyText {
                data: ptr::null(),
                length: 0,
            }
        };
        let copy_span = |text: &HttpProxyText| HttpProxyText {
            data: text.data,
            length: text.length,
        };
        let page = HttpIndexPage {
            path: copy_span(&page.path),
            entry,
            css: copy_span(&page.css),
            powered: copy_span(&page.powered),
            part: page.part,
        };
        // SAFETY: `page` is a local copy of the caller's spans; output follows its contract.
        unsafe { swr_v1_http_index_page(&raw const page, output, capacity, written) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    #[test]
    fn scan_finishes_same_owner_with_sorted_unique_copied_bytes() {
        let mut owner = HttpDirectory::default();
        let mut expected = BTreeSet::new();
        for i in (0..4096u32).rev() {
            let mut name = format!("entry-{i:04}-\u{e9}").into_bytes();
            for _ in 0..2 {
                assert_eq!(owner.scan_append(&name), Ok(()));
            }
            expected.insert(name.clone());
            name.fill(0xcc);
        }
        assert_eq!(owner.contains(b"x"), Err(INVALID));
        owner.scan_finish();
        assert_eq!(
            (0..owner.len())
                .map(|i| owner.name(i).unwrap())
                .collect::<Vec<_>>(),
            expected.iter().map(Vec::as_slice).collect::<Vec<_>>()
        );
        assert_eq!(owner.len(), expected.len());
        owner.scan_finish();
        for name in &expected {
            assert_eq!(owner.contains(name), Ok(true));
        }
        assert_eq!(owner.contains(b"x"), Ok(false));
    }
    #[test]
    fn rendering_reads_owned_entry_and_preserves_page_query_prefix_contract() {
        let mut owner = HttpDirectory::default();
        let span = |bytes: &[u8]| HttpProxyText {
            data: bytes.as_ptr(),
            length: bytes.len(),
        };
        assert_eq!(owner.scan_append(b"a & <name>"), Ok(()));
        owner.scan_finish();
        let page = HttpIndexPage {
            path: span(b"/dir/"),
            entry: span(b"ignored"),
            css: span(b""),
            powered: span(b""),
            part: 1,
        };
        let expected = b"\t\t<li><a href=/dir/a & <name>>a & <name></a></li>\n";
        for capacity in 0..=expected.len() + 2 {
            let mut output = vec![0xcc; capacity + 1];
            let mut written = 99;
            assert_eq!(
                unsafe { owner.page(0, &page, output.as_mut_ptr(), capacity, &raw mut written) },
                0
            );
            assert_eq!(written, expected.len());
            assert_eq!(
                &output[..capacity.min(written)],
                &expected[..capacity.min(written)]
            );
            assert_eq!(output[capacity], 0xcc);
        }
    }
}
