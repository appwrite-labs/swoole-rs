//! `AsyncFile`'s descriptor ownership. Operations call the
//! injected native coroutine effects, which select the configured pool/ring/native lane.
//! Unrelated archive users do not acquire relocations to those optional effects.
//! Native closes before owner release, outside Rust, because a hooked close may
//! destroy an opaque socket payload. Descriptor copies keep the native shallow
//! value semantics and close deliberately does not invalidate the stored value.
use libc::{c_char, c_int, c_void, mode_t, off_t};

pub struct AsyncFileRecord {
    fd: c_int,
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_async_file_new() -> *mut AsyncFileRecord {
    Box::into_raw(Box::new(AsyncFileRecord { fd: -1 }))
}
/// # Safety
/// source is stable/live. Copy its descriptor value into an independent record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_clone(
    source: *const AsyncFileRecord,
) -> *mut AsyncFileRecord {
    // SAFETY: the live record is stable throughout this scalar snapshot.
    let fd = unsafe { (*source).fd };
    Box::into_raw(Box::new(AsyncFileRecord { fd }))
}
/// # Safety
/// destination is live/exclusive and source live/stable during this leaf copy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_assign(
    destination: *mut AsyncFileRecord,
    source: *const AsyncFileRecord,
) {
    // SAFETY: snapshot before publication, including a self-assignment.
    let fd = unsafe { (*source).fd };
    // SAFETY: the destination remains live and exclusive through publication.
    unsafe { (*destination).fd = fd };
}
/// # Safety
/// owner is released once after native close and all operations/borrows finish.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_free(owner: *mut AsyncFileRecord) {
    if !owner.is_null() {
        // SAFETY: native returns its unique owner after its close effect ends.
        unsafe { drop(Box::from_raw(owner)) };
    }
}
/// # Safety
/// owner is live and stable during this descriptor snapshot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_fd(owner: *const AsyncFileRecord) -> c_int {
    // SAFETY: this is a leaf copy; no reference spans a coroutine effect.
    unsafe { (*owner).fd }
}
/// Open after native finishes the previous close. fd zero is stored as ready
/// but rejected by open's return value, preserving the original interface.
/// Unused private flags/mode/path storage is eliminated; only the actual open
/// operation consumes these inputs.
/// # Safety
/// owner is live/exclusive; path is terminated and remains unchanged through
/// the coroutine open; effect has the actual synchronized coroutine ABI. No owner access runs concurrently while that call waits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_open(
    owner: *mut AsyncFileRecord,
    path: *const c_char,
    flags: c_int,
    mode: mode_t,
    effect: unsafe extern "C" fn(*const c_char, c_int, mode_t) -> c_int,
) -> bool {
    // SAFETY: caller keeps its terminated path alive through the coroutine effect.
    let fd = unsafe { effect(path, flags, mode) };
    // SAFETY: no borrow spans the effect; caller still lends the live owner exclusively.
    unsafe { (*owner).fd = fd };
    fd > 0
}
/// # Safety
/// owner is live; buffer lends count writable bytes through the coroutine call.
/// effect is the configured native read operation, with that same buffer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_read(
    owner: *const AsyncFileRecord,
    buffer: *mut c_void,
    count: usize,
    effect: unsafe extern "C" fn(c_int, *mut c_void, usize) -> isize,
) -> isize {
    // SAFETY: snapshot before calling the existing coroutine ABI.
    let fd = unsafe { (*owner).fd };
    // SAFETY: caller keeps its writable buffer alive through the operation.
    unsafe { effect(fd, buffer, count) }
}
/// # Safety
/// owner is live; buffer lends count readable bytes through the coroutine call.
/// effect is the configured native write operation, with that same buffer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_write(
    owner: *const AsyncFileRecord,
    buffer: *const c_void,
    count: usize,
    effect: unsafe extern "C" fn(c_int, *const c_void, usize) -> isize,
) -> isize {
    // SAFETY: copy the descriptor without retaining a record borrow across IO.
    let fd = unsafe { (*owner).fd };
    // SAFETY: caller keeps its readable bytes alive through the operation.
    unsafe { effect(fd, buffer, count) }
}
/// # Safety
/// owner is live/stable until this coroutine operation returns; effect has the
/// configured native syscall contract and cannot destroy the owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_sync(
    owner: *const AsyncFileRecord,
    effect: unsafe extern "C" fn(c_int) -> c_int,
) -> bool {
    // SAFETY: copy the descriptor before any coroutine effect.
    let fd = unsafe { (*owner).fd };
    // SAFETY: the caller keeps the descriptor's lifetime synchronized.
    unsafe { effect(fd) == 0 }
}
/// # Safety
/// owner is live/stable until this coroutine operation returns; effect has the
/// configured native syscall contract and cannot destroy the owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_truncate(
    owner: *const AsyncFileRecord,
    length: off_t,
    effect: unsafe extern "C" fn(c_int, off_t) -> c_int,
) -> bool {
    // SAFETY: copy before entering the coroutine ABI.
    let fd = unsafe { (*owner).fd };
    // SAFETY: the caller synchronizes its live descriptor through the operation.
    unsafe { effect(fd, length) == 0 }
}
/// # Safety
/// owner is live and info lends writable native stat storage through the call;
/// effect has the native stat contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_stat(
    owner: *const AsyncFileRecord,
    info: *mut libc::stat,
    effect: unsafe extern "C" fn(c_int, *mut libc::stat) -> c_int,
) -> bool {
    // SAFETY: descriptor snapshot ends before the coroutine effect.
    let fd = unsafe { (*owner).fd };
    // SAFETY: the caller lends correctly aligned writable native stat storage.
    unsafe { effect(fd, info) == 0 }
}
/// # Safety
/// owner remains live and descriptor lifetime synchronized through the call;
/// effect has the configured native lseek contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_offset(
    owner: *const AsyncFileRecord,
    offset: off_t,
    whence: c_int,
    effect: unsafe extern "C" fn(c_int, off_t, c_int) -> off_t,
) -> off_t {
    // SAFETY: copy the descriptor, retaining no borrow during the coroutine call.
    let fd = unsafe { (*owner).fd };
    // SAFETY: descriptor and offset have the native lseek contract.
    unsafe { effect(fd, offset, whence) }
}
