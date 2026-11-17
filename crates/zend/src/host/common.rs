//! Known-string ownership: `sw_zend_known_strings`, the table C++'s
//! `SW_ZSTR_KNOWN` reads.
use crate::{Minit, sys::zend_string};
use std::{
    ptr,
    sync::atomic::{AtomicPtr, Ordering::Relaxed},
};

/// Zend's interned known strings, in `SW_ZEND_STR_*` order: null until
/// MINIT fills it, and again after MSHUTDOWN. C++ reads it as a
/// `zend_string **`: `AtomicPtr` has a pointer's layout.
#[unsafe(no_mangle)]
pub static sw_zend_known_strings: AtomicPtr<*mut zend_string> = AtomicPtr::new(ptr::null_mut());

/// The table itself: one slot for each known string, null until interned.
static TABLE: [AtomicPtr<zend_string>; super::known_strings::STRINGS.len()] =
    [const { AtomicPtr::new(ptr::null_mut()) }; super::known_strings::STRINGS.len()];

/// MINIT: publish the table, then intern each string into it.
pub fn known_init(minit: &Minit) {
    // Published before it is filled, matching upstream MINIT ordering. An
    // `AtomicPtr` has a pointer's layout, so C++ reads the slots as pointers.
    sw_zend_known_strings.store(TABLE.as_ptr().cast_mut().cast(), Relaxed);
    for (slot, bytes) in TABLE.iter().zip(super::known_strings::STRINGS) {
        // Frozen bytes include their NUL, which is not counted in the string length.
        let text = bytes.split_last().map_or(&[][..], |(_, text)| text);
        match minit.interned(text) {
            Ok(interned) => slot.store(interned, Relaxed),
            Err(bailout) => {
                bailout.defer();
                break;
            }
        }
    }
}

/// MSHUTDOWN: withdraw the table `known_init` published (the interned strings stay Zend's).
pub extern "C" fn known_free() {
    sw_zend_known_strings.store(ptr::null_mut(), Relaxed);
    for slot in &TABLE {
        slot.store(ptr::null_mut(), Relaxed);
    }
}
