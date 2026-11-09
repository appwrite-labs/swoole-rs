//! The coroutine socket's timeouts: `TimeoutSetter`'s save/set/restore of the
//! selected timeouts.
use core::ffi::{c_int, c_void};

/// `SW_TIMEOUT_DNS`, `_CONNECT`, `_READ` and `_WRITE`, in `timeout_type_list`'s order.
const TIMEOUT_TYPES: [c_int; 4] = [1, 2, 4, 8];

#[repr(C)]
pub struct TimeoutOps {
    /// The socket's timeout of one type, and setting it.
    pub get: unsafe extern "C" fn(*mut c_void, c_int) -> f64,
    pub set: unsafe extern "C" fn(*mut c_void, f64, c_int),
}

/// `TimeoutSetter`: unless `timeout` is 0, for each type in `types`, saves the
/// socket's timeout in `original` (indexed as `timeout_type_list`) and sets
/// `timeout` where it differs; with `restore`, puts back each saved timeout that
/// differed instead.
///
/// # Safety
/// `ops` is live, `socket` is what its effects take, and `original` holds four
/// writable doubles.
#[unsafe(no_mangle)]
#[expect(
    clippy::float_cmp,
    reason = "upstream compares the requested and saved timeouts exactly"
)]
pub unsafe extern "C" fn swr_v1_hook_timeouts(
    ops: *const TimeoutOps,
    socket: *mut c_void,
    timeout: f64,
    types: c_int,
    original: *mut f64,
    restore: bool,
) {
    if timeout == 0.0 {
        return;
    }
    // SAFETY: the caller's live table.
    let ops = unsafe { &*ops };
    for (index, kind) in TIMEOUT_TYPES.into_iter().enumerate() {
        if types & kind == 0 {
            continue;
        }
        // SAFETY: one of the caller's four doubles.
        let saved = unsafe { original.add(index) };
        if restore {
            // SAFETY: as above.
            let value = unsafe { *saved };
            if timeout != value {
                // SAFETY: the live socket's timeout.
                unsafe { (ops.set)(socket, value, kind) };
            }
            continue;
        }
        // SAFETY: the live socket's timeout.
        let value = unsafe { (ops.get)(socket, kind) };
        // SAFETY: as above.
        unsafe { *saved = value };
        if timeout != value {
            // SAFETY: the live socket's timeout.
            unsafe { (ops.set)(socket, timeout, kind) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static TIMEOUTS: RefCell<[f64; 4]> = const { RefCell::new([1.0, 2.0, 3.0, 4.0]) };
        static SETS: RefCell<Vec<(f64, c_int)>> = const { RefCell::new(Vec::new()) };
    }

    unsafe extern "C" fn get(_: *mut c_void, kind: c_int) -> f64 {
        TIMEOUTS.with(|t| t.borrow()[kind.trailing_zeros() as usize])
    }
    unsafe extern "C" fn set(_: *mut c_void, value: f64, kind: c_int) {
        SETS.with(|s| s.borrow_mut().push((value, kind)));
        TIMEOUTS.with(|t| t.borrow_mut()[kind.trailing_zeros() as usize] = value);
    }

    #[test]
    fn timeouts_save_set_and_restore() {
        let ops = TimeoutOps { get, set };
        let mut original = [0.0; 4];
        // SAFETY: a local table and array.
        unsafe {
            swr_v1_hook_timeouts(
                &raw const ops,
                std::ptr::null_mut(),
                3.0,
                4 | 8,
                original.as_mut_ptr(),
                false,
            );
        }
        assert_eq!(original, [0.0, 0.0, 3.0, 4.0]);
        assert_eq!(
            SETS.with(|s| s.borrow().clone()),
            [(3.0, 8)],
            "the read timeout already matched"
        );
        SETS.with(|s| s.borrow_mut().clear());
        // SAFETY: as above.
        unsafe {
            swr_v1_hook_timeouts(
                &raw const ops,
                std::ptr::null_mut(),
                3.0,
                4 | 8,
                original.as_mut_ptr(),
                true,
            );
        }
        assert_eq!(SETS.with(|s| s.borrow().clone()), [(4.0, 8)]);
        SETS.with(|s| s.borrow_mut().clear());
        // SAFETY: as above.
        unsafe {
            swr_v1_hook_timeouts(
                &raw const ops,
                std::ptr::null_mut(),
                0.0,
                15,
                original.as_mut_ptr(),
                false,
            );
        }
        assert!(
            SETS.with(|s| s.borrow().is_empty()),
            "a zero timeout changes nothing"
        );
    }
}
