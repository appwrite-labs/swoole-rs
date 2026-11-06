//! Known-string ownership.
use std::ptr;
use swoole_rs_zend::{
    bailout,
    sys::{self, zend_string},
};

fn failed(code: i32) -> bool {
    if code == 0 {
        false
    } else {
        bailout::Bailout.defer();
        true
    }
}

/// # Safety
/// `slot` is the exclusively owned process-global pointer slot, initialized to null before MINIT.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_known_init(slot: *mut *mut *mut zend_string) {
    let mut allocation = ptr::null_mut();
    // SAFETY: persistent allocation holds one pointer for each frozen known string.
    let error = unsafe {
        sys::swr_zend_try_facade_palloc(
            super::known_strings::STRINGS.len() * size_of::<*mut zend_string>(),
            &raw mut allocation,
        )
    };
    if !failed(error) {
        let table = allocation.cast::<*mut zend_string>();
        // SAFETY: publish the table before filling it, matching upstream MINIT ordering.
        unsafe { slot.write(table) };
        for (index, bytes) in super::known_strings::STRINGS.iter().enumerate() {
            let mut string = ptr::null_mut();
            // SAFETY: frozen bytes include their NUL, which is not counted in the string length.
            if failed(unsafe {
                sys::swr_zend_try_facade_pstring(
                    bytes.as_ptr().cast(),
                    bytes.len() - 1,
                    &raw mut string,
                )
            }) {
                break;
            }
            let mut interned = ptr::null_mut();
            // SAFETY: the intern operation takes the newly allocated persistent string.
            if failed(unsafe { sys::swr_zend_try_facade_intern(string, &raw mut interned) }) {
                break;
            }
            // SAFETY: each initialized interned pointer is published exactly once in bounds.
            unsafe { table.wrapping_add(index).write(interned) };
        }
    }
    bailout::finish();
}

/// # Safety
/// `slot` owns the table allocated by `known_init`; interned strings remain Zend's.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_known_free(slot: *mut *mut *mut zend_string) {
    // SAFETY: the process-global slot is exclusively owned during module shutdown.
    let table = unsafe { slot.read() };
    // SAFETY: release only the persistent pointer array, not Zend's interned strings.
    unsafe { sys::swr_zend_facade_pfree(table.cast()) };
    // SAFETY: publish null only after freeing, matching upstream shutdown ordering.
    unsafe { slot.write(ptr::null_mut()) };
}
