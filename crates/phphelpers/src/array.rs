//! Owned keyed values and sparse bucket cursors. Zend owns the hash storage.
use std::ffi::c_void;
use swoole_rs_zend::{
    bailout,
    sys::{self, Bucket, IS_INDIRECT, IS_REFERENCE, IS_UNDEF, zend_string, zval},
    type_of,
};

pub(crate) struct KeyValue {
    index: u64,
    key: *mut zend_string,
    value: zval,
}

/// # Safety
/// Live source zval and optional string key; both remain borrowed during this call.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_key_new(
    index: u64,
    key: *mut zend_string,
    mut value: *mut zval,
) -> *mut KeyValue {
    let key = if key.is_null() {
        key
    } else {
        // SAFETY: take an independent reference to the live key.
        unsafe { sys::swr_zend_string_copy(key) }
    };
    // SAFETY: the source is a live zval; ZVAL_DEREF unwraps one reference.
    if unsafe { type_of(value) } == IS_REFERENCE {
        // SAFETY: a reference zval holds a live zend_reference.
        let payload = unsafe { (*value).value };
        // SAFETY: a reference zval stores the reference union member.
        let reference = unsafe { payload.ref_ };
        // SAFETY: a live reference contains this initialized value.
        value = unsafe { &raw mut (*reference).val };
    }
    // SAFETY: copying Zend's plain data followed by retaining its payload.
    let mut value = unsafe { value.read() };
    // SAFETY: this owner takes one independent reference.
    unsafe { sys::swr_zval_try_addref(&raw mut value) };
    Box::into_raw(Box::new(KeyValue { index, key, value }))
}

/// # Safety
/// A live uniquely owned key/value handle and array zval.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_key_add(handle: *mut KeyValue, array: *mut zval) {
    // SAFETY: the caller lends its handle for this operation.
    let state = unsafe { &mut *handle };
    // SAFETY: array is a live array zval.
    let table = unsafe { swoole_rs_zend::value::table(array) };
    // SAFETY: Zend's update can destroy the replaced element; the shim contains bailouts.
    let added = unsafe {
        sys::swr_zend_try_key_update(table, state.key, state.index, &raw mut state.value)
    };
    if added.is_null() {
        bailout::Bailout.defer();
    } else {
        // SAFETY: Zend returned the inserted live element, which takes a reference.
        unsafe { sys::swr_zval_try_addref(added) };
    }
    bailout::finish();
}

/// # Safety
/// A uniquely owned live handle, consumed once. Destructors stay behind the C trap.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_key_free(handle: *mut KeyValue) {
    // SAFETY: return the owned Box to its allocator.
    let mut state = unsafe { Box::from_raw(handle) };
    if !state.key.is_null() {
        // SAFETY: this handle owns its key reference.
        unsafe { sys::swr_zend_string_release(state.key) };
    }
    // SAFETY: a live owned zval, whose destructor may execute PHP.
    bailout::defer(unsafe {
        bailout::trap(
            sys::swr_zend_zval_release,
            (&raw mut state.value).cast::<c_void>(),
        )
    });
    drop(state);
    bailout::finish();
}

/// # Safety
/// A live handle whose caller borrows this stable zval until `key_free`.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_key_value(handle: *mut KeyValue) -> *mut zval {
    // SAFETY: the handle is live and Rust does not borrow its state concurrently.
    unsafe { &raw mut (*handle).value }
}

/// A copyable cursor: borrowed buckets are never retained or freed by an iterator.
#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct Iterator {
    value: *mut zval,
    key: *mut zend_string,
    current: *mut Bucket,
    end: *mut Bucket,
    index: u64,
}

impl Iterator {
    unsafe fn skip(&mut self) {
        while self.current != self.end {
            // SAFETY: current is inside the caller's live bucket span.
            let bucket = unsafe { &mut *self.current };
            self.value = &raw mut bucket.val;
            // SAFETY: the bucket's zval is initialized.
            if unsafe { type_of(self.value) } == IS_INDIRECT {
                // SAFETY: an indirect zval holds a live zval pointer.
                let payload = unsafe { (*self.value).value };
                // SAFETY: an indirect value stores the zval pointer union member.
                self.value = unsafe { payload.zv };
            }
            // SAFETY: a bucket or its initialized indirect value.
            if unsafe { type_of(self.value) } == IS_UNDEF {
                // SAFETY: current is strictly before the end of the borrowed span.
                self.current = unsafe { self.current.add(1) };
                continue;
            }
            self.key = bucket.key;
            self.index = if self.key.is_null() { bucket.h } else { 0 };
            break;
        }
    }
}

/// # Safety
/// A live bucket span with an initialized sentinel, as required by the original constructors.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_iterator_new(
    current: *mut Bucket,
    end: *mut Bucket,
    skip: bool,
) -> Iterator {
    // SAFETY: the caller provides an initialized first bucket, including for an end iterator.
    let bucket = unsafe { &mut *current };
    let mut cursor = Iterator {
        value: &raw mut bucket.val,
        key: bucket.key,
        current,
        end,
        index: bucket.h,
    };
    if skip {
        // SAFETY: current..end is a borrowed live bucket span.
        unsafe { cursor.skip() };
    }
    cursor
}

/// # Safety
/// A live cursor, not at end; its borrowed array remains unchanged while walking.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_iterator_next(cursor: *mut Iterator) {
    // SAFETY: the caller exclusively lends initialized cursor storage.
    let cursor = unsafe { &mut *cursor };
    // SAFETY: the cursor is not at end, so its successor is within or one-past the span.
    cursor.current = unsafe { cursor.current.add(1) };
    // SAFETY: the borrowed span remains live.
    unsafe { cursor.skip() };
}

/// # Safety
/// `array` is a live array zval.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_array_count(array: *const zval) -> usize {
    // SAFETY: the caller supplies a live array.
    let table = unsafe { swoole_rs_zend::value::table(array) };
    // SAFETY: Zend's live array stores its element count.
    unsafe { (*table).nNumOfElements as usize }
}

/// # Safety
/// Live array and source zval. On success the array takes the source's reference.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_phphelpers_array_set(
    array: *mut zval,
    index: u64,
    value: *mut zval,
    append: bool,
) -> bool {
    let mut result = 0;
    // SAFETY: replacing an entry can execute a destructor; the shim contains its bailout.
    let bailed =
        unsafe { sys::swr_zend_try_array_insert(array, index, value, append, &raw mut result) };
    if bailed != 0 {
        bailout::Bailout.defer();
    }
    bailout::finish();
    result == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    fn value(kind: u32, number: i64) -> zval {
        let mut value: zval = unsafe { std::mem::zeroed() };
        value.u1 = sys::_zval_struct__bindgen_ty_1 { type_info: kind };
        value.value = sys::zend_value { lval: number };
        value
    }

    #[test]
    fn all_4681_pinned_sparse_and_indirect_walks() {
        let mut cases = 0;
        let mut key: zend_string = unsafe { std::mem::zeroed() };
        for used in 0_usize..=4 {
            for pattern in 0..(1_u32 << (used * 3)) {
                let mut buckets: [Bucket; 6] = unsafe { std::mem::zeroed() };
                let mut targets: [zval; 4] = unsafe { std::mem::zeroed() };
                let mut expected = Vec::new();
                for index in 0..used {
                    let mode = (pattern >> (index * 3)) & 7;
                    let bucket = &mut buckets[index];
                    bucket.h = 10 + index as u64;
                    bucket.key = if mode & 4 != 0 {
                        &raw mut key
                    } else {
                        ptr::null_mut()
                    };
                    targets[index] = value(
                        if mode & 3 == 3 {
                            sys::IS_LONG
                        } else {
                            IS_UNDEF
                        },
                        20 + i64::try_from(index).unwrap(),
                    );
                    bucket.val = value(
                        if mode & 3 >= 2 {
                            IS_INDIRECT
                        } else if mode & 3 == 1 {
                            sys::IS_LONG
                        } else {
                            IS_UNDEF
                        },
                        30 + i64::try_from(index).unwrap(),
                    );
                    if mode & 3 >= 2 {
                        bucket.val.value = sys::zend_value {
                            zv: &raw mut targets[index],
                        };
                    }
                    if mode & 3 == 1 || mode & 3 == 3 {
                        expected.push((
                            index,
                            if mode & 3 == 1 {
                                30 + i64::try_from(index).unwrap()
                            } else {
                                20 + i64::try_from(index).unwrap()
                            },
                            mode & 4 != 0,
                            if mode & 4 != 0 { 0 } else { 10 + index as u64 },
                        ));
                    }
                }
                let start = buckets.as_mut_ptr();
                let end = start.wrapping_add(used);
                let mut cursor = unsafe { swr_v1_phphelpers_iterator_new(start, end, true) };
                let mut actual = Vec::new();
                while cursor.current != end {
                    actual.push((
                        usize::try_from(unsafe { cursor.current.offset_from(start) }).unwrap(),
                        unsafe { (*cursor.value).value.lval },
                        !cursor.key.is_null(),
                        cursor.index,
                    ));
                    unsafe { swr_v1_phphelpers_iterator_next(&raw mut cursor) };
                }
                assert_eq!(actual, expected, "used={used} pattern={pattern}");
                cases += 1;
            }
        }
        assert_eq!(cases, 4681);
    }
}
