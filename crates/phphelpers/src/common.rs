//! Known-string ownership, hash merging and function lookup over typed Zend data.
use std::{ffi::c_char, ptr};
use swoole_rs_zend::{
    bailout,
    sys::{self, HashTable, zend_function, zend_string, zval},
    type_of,
};

fn failed(code: i32) -> bool {
    if code == 0 {
        false
    } else {
        bailout::Bailout.defer();
        true
    }
}

unsafe fn append(array: *mut zval, value: *mut zval) {
    // SAFETY: Zend's append operation is trapped before returning to Rust.
    failed(unsafe { sys::swr_zend_try_facade_append(array, value) });
}

/// # Safety
/// A writable array, borrowed binary key and live value whose reference is transferred.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_array_merge(
    array: *mut zval,
    key: *const c_char,
    length: usize,
    value: *mut zval,
) {
    let mut index = 0;
    // SAFETY: Zend's numeric-key classification reads only the borrowed key span.
    let numeric = unsafe { sys::swr_zend_facade_numeric_key(key, length, &raw mut index) };
    // SAFETY: a live array zval.
    let table = unsafe { swoole_rs_zend::value::table(array) };
    let mut inserted = ptr::null_mut();
    let error = if numeric {
        // SAFETY: an insertion into the caller's writable table; it takes the value reference.
        unsafe { sys::swr_zend_try_facade_add_index(table, index, value, &raw mut inserted) }
    } else {
        // SAFETY: a readable binary key and writable table; Zend copies the key.
        unsafe { sys::swr_zend_try_facade_add_bytes(table, key, length, value, &raw mut inserted) }
    };
    if !failed(error) && inserted.is_null() {
        let current = if numeric {
            // SAFETY: the failed add indicates this key is already present.
            unsafe { sys::zend_hash_index_find(table, index) }
        } else {
            // SAFETY: the failed add indicates this binary key is already present.
            unsafe { sys::zend_hash_str_find(table, key, length) }
        };
        // SAFETY: lookup returned the existing initialized value.
        if unsafe { type_of(current) } == sys::IS_ARRAY {
            // SAFETY: append transfers the new reference into the existing subarray.
            unsafe { append(current, value) };
        } else {
            // SAFETY: a zeroed temporary is undefined before array construction.
            let mut promoted: zval = unsafe { std::mem::zeroed() };
            // SAFETY: construct the two-element promotion with upstream's initial size.
            if !failed(unsafe { sys::swr_zend_try_facade_array_two(&raw mut promoted) }) {
                // SAFETY: promotion retains the original payload before replacing its outer slot.
                unsafe { sys::swr_zval_facade_addref(current) };
                // SAFETY: the temporary array takes the retained original reference.
                unsafe { append(&raw mut promoted, current) };
                if !bailout::pending() {
                    // SAFETY: append transfers the new element's reference to the promotion.
                    unsafe { append(&raw mut promoted, value) };
                }
                if !bailout::pending() {
                    let mut stored = ptr::null_mut();
                    let error = if numeric {
                        // SAFETY: replace the original entry with the promoted array.
                        unsafe {
                            sys::swr_zend_try_facade_update_index(
                                table,
                                index,
                                &raw mut promoted,
                                &raw mut stored,
                            )
                        }
                    } else {
                        // SAFETY: replace the original binary-keyed entry with the promoted array.
                        unsafe {
                            sys::swr_zend_try_facade_update_bytes(
                                table,
                                key,
                                length,
                                &raw mut promoted,
                                &raw mut stored,
                            )
                        }
                    };
                    failed(error);
                }
            }
        }
    }
    bailout::finish();
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

/// # Safety
/// A live function table (null selects the executor's) and readable binary name span.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_function_find(
    table: *const HashTable,
    name: *const c_char,
    length: usize,
) -> *mut zend_function {
    let table = if table.is_null() {
        // SAFETY: the current thread's initialized executor function table.
        unsafe { sys::swr_zend_facade_function_table() }
    } else {
        table
    };
    // SAFETY: a live table and borrowed lookup key.
    let value = unsafe { sys::zend_hash_str_find(table, name, length) };
    if value.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: function tables contain initialized pointer-valued zvals.
    let payload = unsafe { (*value).value };
    // SAFETY: a function-table pointer value identifies a process-lived zend_function.
    unsafe { payload.ptr }.cast()
}
