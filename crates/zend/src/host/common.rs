//! Known-string ownership: `sw_zend_known_strings`, the table C++'s
//! `SW_ZSTR_KNOWN` reads.
use crate::{
    bailout,
    sys::{self, zend_string},
};
use std::ptr;

/// Zend's interned known strings, in `SW_ZEND_KNOWN_STRINGS` order: null until
/// MINIT fills it, and again after MSHUTDOWN.
#[unsafe(no_mangle)]
pub static mut sw_zend_known_strings: *mut *mut zend_string = ptr::null_mut();

fn failed(code: i32) -> bool {
    if code == 0 {
        false
    } else {
        bailout::Bailout.defer();
        true
    }
}

/// MINIT: allocate and publish the table, then intern each string into it.
pub fn known_init() {
    let slot = &raw mut sw_zend_known_strings;
    // A persistent allocation never bails out: one that fails exits the process.
    // SAFETY: persistent allocation holds one pointer for each frozen known string.
    let table = unsafe {
        sys::swr_zend_facade_palloc(
            super::known_strings::STRINGS.len() * size_of::<*mut zend_string>(),
        )
    }
    .cast::<*mut zend_string>();
    // SAFETY: MINIT alone writes the slot; the table is published before it is filled,
    // matching upstream MINIT ordering.
    unsafe { slot.write(table) };
    for (index, bytes) in super::known_strings::STRINGS.iter().enumerate() {
        // SAFETY: frozen bytes include their NUL, which is not counted in the string length.
        let string =
            unsafe { sys::swr_zend_string_persistent(bytes.as_ptr().cast(), bytes.len() - 1) };
        let mut interned = ptr::null_mut();
        // SAFETY: the intern operation takes the newly allocated persistent string.
        if failed(unsafe { sys::swr_zend_try_facade_intern(string, &raw mut interned) }) {
            break;
        }
        // SAFETY: each initialized interned pointer is published exactly once in bounds.
        unsafe { table.wrapping_add(index).write(interned) };
    }
    bailout::finish();
}

/// MSHUTDOWN: free the table `known_init` allocated (the interned strings stay Zend's).
pub extern "C" fn known_free() {
    let slot = &raw mut sw_zend_known_strings;
    // SAFETY: the slot is exclusively owned during module shutdown.
    let table = unsafe { slot.read() };
    // SAFETY: release only the persistent pointer array, not Zend's interned strings.
    unsafe { sys::swr_zend_facade_pfree(table.cast()) };
    // SAFETY: publish null only after freeing, matching upstream shutdown ordering.
    unsafe { slot.write(ptr::null_mut()) };
}
