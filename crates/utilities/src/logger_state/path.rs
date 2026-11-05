//! Scoped filename owners used by real open, rotation and reopen callers.
use super::{ALLOCATION, CAPACITY, INVALID, LogState, OK, Text, admission};
use std::alloc::{Layout, alloc, dealloc};
use std::ffi::c_char;

pub(super) struct LogPath {
    text: Text,
    native_max: usize,
}

fn c_prefix(bytes: &[u8]) -> &[u8] {
    bytes.split(|byte| *byte == 0).next().unwrap_or_default()
}

/// Capture the logical C filename (operation0), or an external binary filename (1).
/// # Safety
/// State is live/quiescent, output is exclusive pointer storage. For operation1,
/// input is readable for its length, null only when empty. No input is
/// retained. A successful sole owner must be destroyed or adopted exactly once.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn swr_v1_utils_log_path_create(
    state: *const LogState,
    operation: u32,
    input: *const u8,
    length: usize,
    output: *mut *mut LogPath,
) -> u32 {
    if output.is_null() {
        return INVALID;
    }
    // SAFETY: non-null (checked above), exclusive pointer storage.
    unsafe { output.write(std::ptr::null_mut()) };
    // SAFETY: `state` is null or live and aligned for this call.
    let Some(state) = (unsafe { state.as_ref() }) else {
        return INVALID;
    };
    let text = match operation {
        0 => Text::copy(
            // SAFETY: the state is quiescent; nothing mutates the logical path during the copy.
            c_prefix(unsafe { &*state.texts[0].get() }.bytes()),
            state.native_max,
        ),
        1 => {
            if let Err(status) = admission(length, state.native_max) {
                return status;
            }
            if length != 0 && input.is_null() {
                return INVALID;
            }
            let bytes = if length == 0 {
                &[]
            } else {
                // SAFETY: non-null, `length` admitted (<= isize::MAX), readable for this call.
                unsafe { std::slice::from_raw_parts(input, length) }
            };
            Text::copy(bytes, state.native_max)
        }
        _ => return INVALID,
    };
    let text = match text {
        Ok(text) => text,
        Err(status) => return status,
    };
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "alloc honours Layout::new::<LogPath>()"
    )]
    // SAFETY: `LogPath` is not zero-sized; null is handled below.
    let path = unsafe { alloc(Layout::new::<LogPath>()) }.cast::<LogPath>();
    if path.is_null() {
        return ALLOCATION;
    }
    // SAFETY: a fresh, non-null allocation with `LogPath`'s layout.
    unsafe {
        path.write(LogPath {
            text,
            native_max: state.native_max,
        });
    };
    // SAFETY: as for the first write to `output`.
    unsafe { output.write(path) };
    OK
}

/// Complete captured filename+dot+date after native calendar effects return.
/// # Safety
/// Path is exclusively owned and date readable for `date_length` bytes, null only
/// when empty. Replacement is published only after full fallible construction.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn swr_v1_utils_log_path_rotate(
    path: *mut LogPath,
    date: *const u8,
    date_length: usize,
) -> u32 {
    // SAFETY: `path` is null or live, aligned and exclusively lent for this call.
    let Some(path) = (unsafe { path.as_mut() }) else {
        return INVALID;
    };
    let Some(total) = path
        .text
        .bytes()
        .len()
        .checked_add(1)
        .and_then(|n| n.checked_add(date_length))
    else {
        return CAPACITY;
    };
    if let Err(status) = admission(total, path.native_max) {
        return status;
    }
    if date_length != 0 && date.is_null() {
        return INVALID;
    }
    let date = if date_length == 0 {
        &[]
    } else {
        // SAFETY: non-null, `date_length` within the admitted total; readable for this call.
        unsafe { std::slice::from_raw_parts(date, date_length) }
    };
    match Text::assemble(&[path.text.bytes(), b".", date], path.native_max) {
        Ok(text) => {
            path.text = text;
            OK
        }
        Err(status) => status,
    }
}

/// Borrow complete bytes and their length; the allocation also ends in NUL.
/// # Safety
/// Path is null or live, length is null or exclusive writable `size_t` storage.
/// Borrowed bytes remain valid until destroy/adopt; no Rust reference escapes.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn swr_v1_utils_log_path_get(
    path: *const LogPath,
    length: *mut usize,
) -> *const c_char {
    // SAFETY: `path` is null or live and aligned for this call.
    let path = unsafe { path.as_ref() };
    if !length.is_null() {
        // SAFETY: non-null (checked above), exclusive writable size_t storage.
        unsafe { length.write(path.map_or(0, |p| p.text.bytes().len())) };
    }
    path.map_or(c"".as_ptr(), |p| p.text.pointer())
}

/// # Safety
/// Path is null or solely owned, quiescent, and not previously consumed.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn swr_v1_utils_log_path_destroy(path: *mut LogPath) {
    if !path.is_null() {
        // SAFETY: solely owned from create, quiescent, and not previously consumed.
        unsafe { path.drop_in_place() };
        // SAFETY: allocated with this layout by create; its value was just dropped.
        unsafe { dealloc(path.cast(), Layout::new::<LogPath>()) };
    }
}

/// Move completed bytes into the resolved filename, consuming path on success.
/// # Safety
/// State and path are live/disjoint and exclusive against the relevant mutation.
/// Failure retains both owners. Success invalidates path but retains its byte
/// allocation in state, without another reserve or copy.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn swr_v1_utils_log_path_adopt(
    state: *const LogState,
    path: *mut LogPath,
) -> u32 {
    // SAFETY: `state` is null or live and aligned for this call.
    let state = unsafe { state.as_ref() };
    // SAFETY: `path` is null or live and aligned for this call.
    let borrowed = unsafe { path.as_ref() };
    let (Some(state), Some(borrowed)) = (state, borrowed) else {
        return INVALID;
    };
    if let Err(status) = admission(borrowed.text.bytes().len(), state.native_max) {
        return status;
    }
    // SAFETY: non-null and live (checked above); the move-out is final, since the
    // allocation is freed below without dropping in place.
    let owned = unsafe { path.read() };
    // SAFETY: the resolved-path field is exclusive against this mutation.
    unsafe { *state.texts[1].get() = owned.text };
    // SAFETY: allocated with this layout by create; its value was moved out above.
    unsafe { dealloc(path.cast(), Layout::new::<LogPath>()) };
    OK
}

/// Emission with scoped path comparison at the original fresh-observation phase.
/// # Safety
/// State/phase obey the existing owned emit contract. At phase14 path is live
/// and readable; elsewhere it may be null. Native effects follow return, with
/// the native RAII handle retaining path through the original rotation scope.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn swr_v1_utils_log_path_emit_step(
    state: *const LogState,
    phase: *mut u32,
    level: i32,
    path: *const LogPath,
    acquired: u8,
    worker_pid: i32,
) -> u32 {
    if phase.is_null() {
        return 0;
    }
    // SAFETY: `phase` is non-null (checked above), aligned and initialized.
    let changed = if unsafe { phase.read() } == 14 {
        // SAFETY: `state` is null or live and aligned for this call.
        let state = unsafe { state.as_ref() };
        // SAFETY: `path` is null or live and aligned for this call.
        let path = unsafe { path.as_ref() };
        let (Some(state), Some(path)) = (state, path) else {
            return 0;
        };
        // SAFETY: nothing mutates the resolved path during this comparison.
        path.text.bytes() != c_prefix(unsafe { &*state.texts[1].get() }.bytes())
    } else {
        false
    };
    // SAFETY: forwards the caller's state and phase contract unchanged.
    unsafe {
        super::swr_v1_utils_log_state_emit_step(
            state,
            phase,
            level,
            u8::from(changed),
            acquired,
            worker_pid,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;
    use std::ffi::CStr;
    #[test]
    fn normal_rotated_value_capture_and_transfer() {
        unsafe {
            let mut state = std::ptr::null_mut();
            assert_eq!(swr_v1_utils_log_state_create(1024, &raw mut state), OK);
            let logical = b"original\0hidden";
            assert_eq!(
                swr_v1_utils_log_state_text_set(state, 0, logical.as_ptr(), logical.len()),
                OK
            );
            let mut captured = std::ptr::null_mut();
            assert_eq!(
                swr_v1_utils_log_path_create(state, 0, std::ptr::null(), 0, &raw mut captured),
                OK
            );
            swr_v1_utils_log_state_clear(state, 0);
            assert_eq!(
                CStr::from_ptr(swr_v1_utils_log_path_get(captured, std::ptr::null_mut()))
                    .to_bytes(),
                b"original"
            );
            swr_v1_utils_log_path_destroy(captured);
            for file in [b"".as_slice(), b"plain", b"binary\0tail"] {
                for date in [b"".as_slice(), b"20400102"] {
                    let mut path = std::ptr::null_mut();
                    assert_eq!(
                        swr_v1_utils_log_path_create(
                            state,
                            1,
                            file.as_ptr(),
                            file.len(),
                            &raw mut path
                        ),
                        OK
                    );
                    assert_eq!(
                        swr_v1_utils_log_path_rotate(path, date.as_ptr(), date.len()),
                        OK
                    );
                    let mut length = 0;
                    let pointer = swr_v1_utils_log_path_get(path, &raw mut length);
                    let expected = [file, b".", date].concat();
                    assert_eq!(
                        std::slice::from_raw_parts(pointer.cast::<u8>(), length),
                        expected
                    );
                    // Compare at the fresh observation point, not construction.
                    assert_eq!(
                        swr_v1_utils_log_state_text_set(
                            state,
                            1,
                            expected.as_ptr(),
                            expected.len()
                        ),
                        OK
                    );
                    let mut current = 14;
                    let mut frozen = 14;
                    let changed = expected.iter().position(|byte| *byte == 0).is_some();
                    let reference = crate::swr_v1_utils_log_emit_step(
                        &raw mut frozen,
                        3,
                        0,
                        0,
                        0,
                        0,
                        0,
                        u8::from(changed),
                        0,
                        123,
                    );
                    assert_eq!(
                        swr_v1_utils_log_path_emit_step(state, &raw mut current, 3, path, 0, 123),
                        reference
                    );
                    assert_eq!(current, frozen);
                    assert_eq!(swr_v1_utils_log_path_adopt(state, path), OK);
                    assert_eq!(swr_v1_utils_log_state_text_get(state, 1), pointer);
                }
            }
            swr_v1_utils_log_state_destroy(state);
        }
    }
}
