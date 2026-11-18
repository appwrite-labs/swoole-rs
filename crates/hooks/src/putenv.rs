//! Assignment syntax and process-lifetime string storage for ZTS putenv.
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::sync::Arc;

pub type Table = HashMap<Vec<u8>, Arc<CStr>>;

pub struct Assignment<'a> {
    pub key: &'a [u8],
    text: Arc<CStr>,
    unset: bool,
}

impl<'a> Assignment<'a> {
    #[must_use]
    pub fn parse(setting: &'a [u8]) -> Option<Self> {
        if setting.first().is_none_or(|&first| first == b'=') {
            return None;
        }
        let text = setting.split(|&b| b == 0).next().unwrap_or_default();
        let separator = text.iter().position(|&b| b == b'=');
        Some(Self {
            key: separator.and_then(|end| text.get(..end)).unwrap_or(setting),
            text: CString::new(text).unwrap_or_default().into(),
            unset: separator.is_none(),
        })
    }

    /// On failure libc still references the previous allocation.
    pub fn store(self, table: &mut Table, update: impl FnOnce(&CStr, bool) -> bool) -> bool {
        if !update(&self.text, self.unset) {
            return false;
        }
        table.insert(self.key.to_vec(), self.text);
        true
    }
}

#[cfg(test)]
#[path = "../../../tests/modules/hooks/putenv.rs"]
mod tests;
