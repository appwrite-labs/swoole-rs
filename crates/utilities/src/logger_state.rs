//! Selected Logger's sole logical/string authority. Native code owns IO resources.
use std::alloc::{Layout, alloc, dealloc};
use std::cell::{Cell, UnsafeCell};
use std::ffi::{CStr, c_char};

mod path;

const OK: u32 = 0;
const LENGTH: u32 = 1;
const CAPACITY: u32 = 2;
const ALLOCATION: u32 = 3;
const INVALID: u32 = 4;

enum Text {
    Static(&'static CStr),
    Owned(Vec<u8>),
}
impl Text {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Static(value) => value.to_bytes(),
            // Owned text keeps its NUL terminator for `pointer`.
            Self::Owned(value) => value.split_last().map_or(&[], |(_, text)| text),
        }
    }
    fn pointer(&self) -> *const c_char {
        match self {
            Self::Static(value) => value.as_ptr(),
            Self::Owned(value) => value.as_ptr().cast(),
        }
    }
    fn copy(bytes: &[u8], native_max: usize) -> Result<Self, u32> {
        Self::assemble(&[bytes], native_max)
    }
    fn assemble(chunks: &[&[u8]], native_max: usize) -> Result<Self, u32> {
        let length = chunks.iter().try_fold(0usize, |n, bytes| {
            n.checked_add(bytes.len()).ok_or(CAPACITY)
        })?;
        let capacity = admission(length, native_max)?;
        let mut owned = Vec::new();
        owned.try_reserve_exact(capacity).map_err(|_| ALLOCATION)?;
        for bytes in chunks {
            owned.extend_from_slice(bytes);
        }
        owned.push(0);
        Ok(Self::Owned(owned))
    }
}
fn admission(length: usize, native_max: usize) -> Result<usize, u32> {
    if length > native_max {
        return Err(LENGTH);
    }
    length
        .checked_add(1)
        .filter(|count| isize::try_from(*count).is_ok())
        .ok_or(CAPACITY)
}

/// Opaque owned context; public layout is deliberately absent from the C header.
/// Native mutex/quiescence rules protect each mutable field. No borrow escapes
/// an operation; returned raw text pointers remain valid until relevant mutation.
pub struct LogState {
    opened: Cell<bool>,
    redirected: Cell<bool>,
    backtrace: Cell<bool>,
    level: Cell<i32>,
    microseconds: Cell<bool>,
    rotation: Cell<i32>,
    texts: [UnsafeCell<Text>; 3], // logical path, real path, date format
    native_max: usize,
}
impl LogState {
    fn new(native_max: usize) -> Self {
        let (date, rotation, level, microseconds) = super::log_defaults();
        Self {
            opened: Cell::new(false),
            redirected: Cell::new(false),
            backtrace: Cell::new(false),
            level: Cell::new(level),
            microseconds: Cell::new(microseconds != 0),
            rotation: Cell::new(rotation),
            texts: [
                UnsafeCell::new(Text::Static(c"")),
                UnsafeCell::new(Text::Static(c"")),
                UnsafeCell::new(Text::Static(date)),
            ],
            native_max,
        }
    }
    // No native callback runs while these field borrows exist.
    unsafe fn clear_paths(&self) {
        // SAFETY: callers hold the path fields exclusively, with no outstanding borrow.
        unsafe { *self.texts[0].get() = Text::Static(c"") };
        // SAFETY: as above.
        unsafe { *self.texts[1].get() = Text::Static(c"") };
    }
}

/// Fallible owned context construction; publishes only a complete context.
/// # Safety
/// Output is aligned/writable pointer storage. `Native_max` is the actual native
/// `std::string` maximum. On success its sole owner must later destroy the handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_log_state_create(
    native_max: usize,
    output: *mut *mut LogState,
) -> u32 {
    if output.is_null() {
        return INVALID;
    }
    // SAFETY: non-null (checked above), aligned, writable pointer storage.
    unsafe { output.write(std::ptr::null_mut()) };
    if super::log_defaults().0.to_bytes().len() > native_max {
        return LENGTH;
    }
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "alloc honours Layout::new::<LogState>()"
    )]
    // SAFETY: `LogState` is not zero-sized; null is handled below.
    let state = unsafe { alloc(Layout::new::<LogState>()) }.cast::<LogState>();
    if state.is_null() {
        return ALLOCATION;
    }
    // SAFETY: a fresh, non-null allocation with `LogState`'s layout.
    unsafe { state.write(LogState::new(native_max)) };
    // SAFETY: as for the first write to `output`.
    unsafe { output.write(state) };
    OK
}

/// Release owned bytes/context only; FILE/descriptors are never closed here.
/// # Safety
/// Handle is null or exclusively owned from create, with all users quiescent.
/// It must be destroyed at most once; no filename pointer may remain in use.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_log_state_destroy(state: *mut LogState) {
    if !state.is_null() {
        // SAFETY: exclusively owned from create, users quiescent, destroyed at most once.
        unsafe { state.drop_in_place() };
        // SAFETY: allocated with this layout by create; its value was just dropped.
        unsafe { dealloc(state.cast(), Layout::new::<LogState>()) };
    }
}

/// Read opened/redirected/backtrace/level/microseconds/rotation, IDs0..5.
/// # Safety
/// State is live; native lock/quiescence and configuration rules are observed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_log_state_scalar_get(
    state: *const LogState,
    field: u32,
) -> i32 {
    // SAFETY: `state` is null or live and aligned for this call.
    let Some(state) = (unsafe { state.as_ref() }) else {
        return 0;
    };
    match field {
        0 => i32::from(state.opened.get()),
        1 => i32::from(state.redirected.get()),
        2 => i32::from(state.backtrace.get()),
        3 => state.level.get(),
        4 => i32::from(state.microseconds.get()),
        5 => state.rotation.get(),
        _ => 0,
    }
}

/// Apply configuration or publication after the required native effect.
/// # Safety
/// State is live; configuration is quiescent. Opened/redirected writes require
/// the matching native resource transition. No native callback is invoked here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_log_state_scalar_set(
    state: *const LogState,
    field: u32,
    value: i32,
) -> u32 {
    // SAFETY: `state` is null or live and aligned for this call.
    let Some(state) = (unsafe { state.as_ref() }) else {
        return INVALID;
    };
    match field {
        0 => state.opened.set(value != 0),
        1 => state.redirected.set(value != 0),
        2 => state.backtrace.set(value != 0),
        3 => state.level.set(super::swr_v1_utils_log_level(value)),
        4 => state.microseconds.set(value != 0),
        5 => state.rotation.set(value),
        _ => return INVALID,
    }
    OK
}

/// Borrow a NUL-terminated logical path/real path/date-format pointer, IDs0..2.
/// # Safety
/// State is live; callers are quiescent against relevant mutations/destruction.
/// The raw pointer's validity ends at that mutation; no Rust reference escapes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_log_state_text_get(
    state: *const LogState,
    field: u32,
) -> *const c_char {
    // SAFETY: `state` is null or live and aligned for this call.
    let Some(state) = (unsafe { state.as_ref() }) else {
        return c"".as_ptr();
    };
    match state.texts.get(field as usize) {
        // SAFETY: callers are quiescent against mutation of this field for this read.
        Some(text) => unsafe { &*text.get() }.pointer(),
        None => c"".as_ptr(),
    }
}

/// Fallible copy before publication; failure leaves the old field untouched.
/// # Safety
/// State is live and the target field is exclusively accessible. Input is
/// initialized/readable for length bytes, null only when empty. Input may alias
/// current stored bytes: copying completes before the old allocation is dropped.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_log_state_text_set(
    state: *const LogState,
    field: u32,
    input: *const u8,
    length: usize,
) -> u32 {
    // SAFETY: `state` is null or live and aligned for this call.
    let Some(state) = (unsafe { state.as_ref() }) else {
        return INVALID;
    };
    let Some(target) = state.texts.get(field as usize) else {
        return INVALID;
    };
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
    match Text::copy(bytes, state.native_max) {
        Ok(text) => {
            // SAFETY: the target field is exclusively accessible; the copy (which may have
            // read it through `input`) is complete, so no borrow of it remains.
            unsafe { *target.get() = text };
            OK
        }
        Err(status) => status,
    }
}

/// Fallible field copy; the target is published only after a completed copy.
/// # Safety
/// State and IDs are valid; source is readable and target exclusively accessible.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_log_state_text_copy(
    state: *const LogState,
    source: u32,
    target: u32,
) -> u32 {
    // SAFETY: `state` is null or live and aligned for this call.
    let Some(state) = (unsafe { state.as_ref() }) else {
        return INVALID;
    };
    let (Some(source_cell), Some(target_cell)) = (
        state.texts.get(source as usize),
        state.texts.get(target as usize),
    ) else {
        return INVALID;
    };
    if source == target {
        return OK;
    }
    // SAFETY: the source field is readable and not mutated during the copy.
    let result = Text::copy(unsafe { &*source_cell.get() }.bytes(), state.native_max);
    match result {
        Ok(text) => {
            // SAFETY: the target (a different field) is exclusively accessible, and the
            // source borrow ended with the copy.
            unsafe { *target_cell.get() = text };
            OK
        }
        Err(status) => status,
    }
}

/// Reset only the original four configuration values, retaining backtrace/IO.
/// # Safety
/// State is live and configuration mutations are quiescent.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_log_state_reset(state: *const LogState) {
    // SAFETY: `state` is null or live and aligned for this call.
    if let Some(state) = unsafe { state.as_ref() } {
        let (date, rotation, level, microseconds) = super::log_defaults();
        // SAFETY: configuration is quiescent; no borrow of the date field is outstanding.
        unsafe { *state.texts[2].get() = Text::Static(date) };
        state.microseconds.set(microseconds != 0);
        state.rotation.set(rotation);
        state.level.set(level);
    }
}

/// Publish native close or failed-open cleanup in the original field order.
/// # Safety
/// State is live/exclusive after native fclose/pointer-reset or warning effects.
/// Failed-open=1 publishes opened=false before paths; close=0 publishes it last.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_log_state_clear(state: *const LogState, failed_open: u8) {
    // SAFETY: `state` is null or live and aligned for this call.
    if let Some(state) = unsafe { state.as_ref() } {
        if failed_open != 0 {
            state.opened.set(false);
        }
        if failed_open != 0 {
            // SAFETY: the state is exclusive after the native effects; no path borrow remains.
            unsafe { state.clear_paths() };
        } else {
            // Native close retains the last resolved filename.
            // SAFETY: as above.
            unsafe { *state.texts[0].get() = Text::Static(c"") };
        }
        if failed_open == 0 {
            state.opened.set(false);
        }
    }
}

/// Native file admission over owned state; frozen v1 controller stays unchanged.
/// # Safety
/// State is live, phase is exclusive aligned initialized u32 storage, and native
/// mutex/quiescence rules hold at each declared field-observation phase.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_log_state_file_step(
    state: *const LogState,
    phase: *mut u32,
    operation: u32,
    descriptor: i32,
) -> u32 {
    // SAFETY: `state` is null or live and aligned for this call.
    let Some(state) = (unsafe { state.as_ref() }) else {
        return 0;
    };
    if phase.is_null() {
        return 0;
    }
    // SAFETY: `phase` is non-null (checked above), aligned, exclusive and initialized.
    let current = unsafe { phase.read() };
    // SAFETY: forwards the caller's exclusive, initialized phase.
    unsafe {
        super::swr_v1_utils_log_file_step(
            phase,
            operation,
            if current == 0 {
                u8::from(state.opened.get())
            } else {
                0
            },
            if operation == 0 && current == 2 {
                state.rotation.get()
            } else {
                0
            },
            if operation == 2 && current == 3 {
                u8::from(state.redirected.get())
            } else {
                0
            },
            descriptor,
        )
    }
}

/// Native redirection admission; descriptor and diagnostic effects remain native.
/// # Safety
/// Live state, exclusive initialized phase and consistent native resource facts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_log_state_redirect_step(
    state: *const LogState,
    phase: *mut u32,
    enable: u8,
    result: i32,
) -> u32 {
    // SAFETY: `state` is null or live and aligned for this call.
    let Some(state) = (unsafe { state.as_ref() }) else {
        return 0;
    };
    if phase.is_null() {
        return 0;
    }
    // SAFETY: `phase` is non-null (checked above), aligned, exclusive and initialized.
    let current = unsafe { phase.read() };
    let opened = current == 0 && enable != 0 && state.opened.get();
    let redirected = current == 0 && (enable == 0 || opened) && state.redirected.get();
    // SAFETY: forwards the caller's exclusive, initialized phase.
    unsafe {
        super::swr_v1_utils_redirect_step(
            phase,
            enable,
            u8::from(opened),
            u8::from(redirected),
            result,
        )
    }
}

/// Stream/date admission, with opened read only for stream entry.
/// # Safety
/// Live state and exclusive initialized phase; native effects follow return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_log_state_config_step(
    state: *const LogState,
    phase: *mut u32,
    operation: u32,
    formatted: usize,
) -> u32 {
    // SAFETY: `state` is null or live and aligned for this call.
    let Some(state) = (unsafe { state.as_ref() }) else {
        return 0;
    };
    if phase.is_null() {
        return 0;
    }
    // SAFETY: `phase` is non-null (checked above), aligned, exclusive and initialized.
    let opened = operation == 2 && unsafe { phase.read() } == 0 && state.opened.get();
    // SAFETY: forwards the caller's exclusive, initialized phase.
    unsafe {
        super::swr_v1_utils_log_config_step(
            phase,
            operation,
            u8::from(opened),
            formatted,
            std::ptr::null_mut(),
        )
    }
}

/// Complete emission over owned facts, refreshed only in their original phases.
/// # Safety
/// Live state and exclusive phase. Native mutex is held during opened reads
/// (phases6/9); configuration is quiescent, and all native effects follow return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_utils_log_state_emit_step(
    state: *const LogState,
    phase: *mut u32,
    level: i32,
    path_changed: u8,
    acquired: u8,
    worker_pid: i32,
) -> u32 {
    // SAFETY: `state` is null or live and aligned for this call.
    let Some(state) = (unsafe { state.as_ref() }) else {
        return 0;
    };
    if phase.is_null() {
        return 0;
    }
    // SAFETY: `phase` is non-null (checked above), aligned, exclusive and initialized.
    let current = unsafe { phase.read() };
    // SAFETY: forwards the caller's exclusive, initialized phase.
    unsafe {
        super::swr_v1_utils_log_emit_step(
            phase,
            level,
            if current == 0 { state.level.get() } else { 0 },
            if current == 1 {
                state.rotation.get()
            } else {
                0
            },
            if current == 2 {
                u8::from(state.microseconds.get())
            } else {
                0
            },
            if current == 6 || current == 9 {
                u8::from(state.opened.get())
            } else {
                0
            },
            if current == 11 {
                u8::from(state.backtrace.get())
            } else {
                0
            },
            path_changed,
            acquired,
            worker_pid,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normal_owned_state_and_atomic_publication() {
        unsafe {
            let mut state = std::ptr::null_mut();
            assert_eq!(swr_v1_utils_log_state_create(32, &raw mut state), OK);
            assert_eq!(swr_v1_utils_log_state_scalar_get(state, 3), 2);
            assert_eq!(swr_v1_utils_log_state_scalar_set(state, 3, 999), OK);
            assert_eq!(swr_v1_utils_log_state_scalar_get(state, 3), 6);
            let path = b"one/owned/path";
            assert_eq!(
                swr_v1_utils_log_state_text_set(state, 0, path.as_ptr(), path.len()),
                OK
            );
            assert_eq!(swr_v1_utils_log_state_text_copy(state, 0, 1), OK);
            assert_eq!(
                CStr::from_ptr(swr_v1_utils_log_state_text_get(state, 1)).to_bytes(),
                path
            );
            let pointer = swr_v1_utils_log_state_text_get(state, 0).cast();
            assert_eq!(
                swr_v1_utils_log_state_text_set(state, 0, pointer, path.len()),
                OK
            );
            assert_eq!(swr_v1_utils_log_state_scalar_set(state, 2, 1), OK);
            swr_v1_utils_log_state_reset(state);
            assert_eq!(swr_v1_utils_log_state_scalar_get(state, 2), 1);
            assert_eq!(
                CStr::from_ptr(swr_v1_utils_log_state_text_get(state, 2)),
                c"%F %T"
            );
            swr_v1_utils_log_state_clear(state, 0);
            assert_eq!(
                CStr::from_ptr(swr_v1_utils_log_state_text_get(state, 0)),
                c""
            );
            assert_eq!(
                CStr::from_ptr(swr_v1_utils_log_state_text_get(state, 1)).to_bytes(),
                path
            );
            swr_v1_utils_log_state_destroy(state);
        }
    }
    #[test]
    fn scalar_capacity_admission_without_large_memory_or_pointer_probe() {
        assert_eq!(admission(8, 7), Err(LENGTH));
        assert_eq!(admission(usize::MAX, usize::MAX), Err(CAPACITY));
        assert_eq!(admission(isize::MAX as usize, usize::MAX), Err(CAPACITY));
        assert_eq!(admission(0, 7), Ok(1));
    }
}
