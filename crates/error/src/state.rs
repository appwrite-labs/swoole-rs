// SPDX-License-Identifier: Apache-2.0
//! Process-local ignored errors; crates/error-ffi gives native code the handle.
use std::collections::{HashSet, TryReserveError};

/// The codes whose errors are not logged.
pub struct IgnoredErrors(HashSet<i32>);

impl IgnoredErrors {
    /// A set holding `code`.
    /// # Errors
    /// When the set cannot allocate.
    pub fn try_with(code: i32) -> Result<Self, TryReserveError> {
        let mut values = Self(HashSet::new());
        values.try_insert(code)?;
        Ok(values)
    }

    /// Insert idempotently.
    /// # Errors
    /// When the set cannot grow; it is then unchanged.
    pub fn try_insert(&mut self, code: i32) -> Result<(), TryReserveError> {
        if !self.0.contains(&code) {
            self.0.try_reserve(1)?;
            self.0.insert(code);
        }
        Ok(())
    }

    #[must_use]
    pub fn contains(&self, code: i32) -> bool {
        self.0.contains(&code)
    }

    /// The codes the set holds before it must allocate again.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.0.capacity()
    }
}
