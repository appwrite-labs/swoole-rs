//! Private `StaticHandler` directory owner. No native container or name mirror.
use crate::INVALID;

#[derive(Default)]
pub(crate) struct HttpDirectory {
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
    /// The finished store's name at `index`.
    pub(crate) fn name(&self, index: usize) -> Option<&[u8]> {
        if self.pending {
            return None;
        }
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
    pub(crate) fn len(&self) -> usize {
        self.names.len()
    }
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
    /// Whether the finished store holds `name`.
    ///
    /// # Errors
    /// `INVALID` while a scan is unfinished.
    pub(crate) fn contains(&self, name: &[u8]) -> Result<bool, i32> {
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
    pub(crate) fn scan_append(&mut self, name: &[u8]) -> Result<(), i32> {
        let span = self.copy_name(name)?;
        self.names.push(span);
        self.pending = true;
        Ok(())
    }
    /// Finishes the scan in place: unsigned byte sort, then duplicates go.
    pub(crate) fn scan_finish(&mut self) {
        if self.pending {
            let bytes = &self.bytes;
            self.names
                .sort_unstable_by(|a, b| at(bytes, a).cmp(at(bytes, b)));
            self.names.dedup_by(|a, b| at(bytes, a) == at(bytes, b));
            self.pending = false;
        }
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
}
