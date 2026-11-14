//! Known-string ownership: `sw_zend_known_strings`, the table C++'s
//! `SW_ZSTR_KNOWN` reads.
use crate::{
    bailout,
    sys::{self, zend_string},
};
use std::{
    io::IoSlice,
    ptr,
    sync::atomic::{AtomicPtr, Ordering::Relaxed},
};

unsafe extern "C" {
    /// `zend_string_init(bytes, len, 1)`: it reads the lent bytes (an `IoSlice` is
    /// an iovec) and returns a new persistent string; out of memory, Zend exits.
    pub(crate) safe fn swr_bytes_persistent(bytes: IoSlice<'_>) -> *mut zend_string;
}

/// Zend's interned known strings, in `SW_ZEND_STR_*` order: null until
/// MINIT fills it, and again after MSHUTDOWN. C++ reads it as a
/// `zend_string **`: `AtomicPtr` has a pointer's layout.
#[unsafe(no_mangle)]
pub static sw_zend_known_strings: AtomicPtr<*mut zend_string> = AtomicPtr::new(ptr::null_mut());

fn failed(code: i32) -> bool {
    if code == 0 {
        false
    } else {
        bailout::Bailout.defer();
        true
    }
}

/// The table itself: one slot for each known string, null until interned.
static TABLE: [AtomicPtr<zend_string>; super::known_strings::STRINGS.len()] =
    [const { AtomicPtr::new(ptr::null_mut()) }; super::known_strings::STRINGS.len()];

/// MINIT: publish the table, then intern each string into it.
pub fn known_init() {
    // Published before it is filled, matching upstream MINIT ordering. An
    // `AtomicPtr` has a pointer's layout, so C++ reads the slots as pointers.
    sw_zend_known_strings.store(TABLE.as_ptr().cast_mut().cast(), Relaxed);
    for (slot, bytes) in TABLE.iter().zip(super::known_strings::STRINGS) {
        // Frozen bytes include their NUL, which is not counted in the string length.
        let text = bytes.split_last().map_or(&[][..], |(_, text)| text);
        let string = swr_bytes_persistent(IoSlice::new(text));
        let mut interned = ptr::null_mut();
        // SAFETY: the intern operation takes the newly allocated persistent string.
        if failed(unsafe { sys::swr_zend_try_facade_intern(string, &raw mut interned) }) {
            break;
        }
        slot.store(interned, Relaxed);
    }
    bailout::finish();
}

/// MSHUTDOWN: withdraw the table `known_init` published (the interned strings stay Zend's).
pub extern "C" fn known_free() {
    sw_zend_known_strings.store(ptr::null_mut(), Relaxed);
    for slot in &TABLE {
        slot.store(ptr::null_mut(), Relaxed);
    }
}
