//! `AsyncFile`'s descriptor ownership. Operations call the
//! injected native coroutine effects, which select the configured pool/ring/native lane.
//! Unrelated archive users do not acquire relocations to those optional effects.
//! Native closes before owner release, outside Rust, because a hooked close may
//! destroy an opaque socket payload. Descriptor copies keep the native shallow
//! value semantics and close deliberately does not invalidate the stored value.
use libc::{c_char, c_int, c_void, mode_t, off_t};
use std::cell::Cell;

/// Shared by every operation (a self-assignment and coroutines waiting in effects
/// alias it), so the descriptor is a `Cell`.
pub struct AsyncFileRecord {
    fd: Cell<c_int>,
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_async_file_new() -> *mut AsyncFileRecord {
    Box::into_raw(Box::new(AsyncFileRecord { fd: Cell::new(-1) }))
}
/// Copy the descriptor value into an independent record.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_async_file_clone(source: &AsyncFileRecord) -> *mut AsyncFileRecord {
    Box::into_raw(Box::new(AsyncFileRecord {
        fd: source.fd.clone(),
    }))
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_async_file_assign(
    destination: &AsyncFileRecord,
    source: &AsyncFileRecord,
) {
    destination.fd.set(source.fd.get());
}
/// Released once (or null), after native close and all operations finish.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_async_file_free(owner: Option<Box<AsyncFileRecord>>) {
    drop(owner);
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_async_file_fd(owner: &AsyncFileRecord) -> c_int {
    owner.fd.get()
}
/// Open after native finishes the previous close. fd zero is stored as ready
/// but rejected by open's return value, preserving the original interface.
/// Unused private flags/mode/path storage is eliminated; only the actual open
/// operation consumes these inputs.
/// # Safety
/// path is terminated and remains unchanged through
/// the coroutine open; effect has the actual synchronized coroutine ABI. No owner access runs concurrently while that call waits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_open(
    owner: &AsyncFileRecord,
    path: *const c_char,
    flags: c_int,
    mode: mode_t,
    effect: unsafe extern "C" fn(*const c_char, c_int, mode_t) -> c_int,
) -> bool {
    // SAFETY: caller keeps its terminated path alive through the coroutine effect.
    let fd = unsafe { effect(path, flags, mode) };
    owner.fd.set(fd);
    fd > 0
}
/// # Safety
/// buffer lends count writable bytes through the coroutine call.
/// effect is the configured native read operation, with that same buffer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_read(
    owner: &AsyncFileRecord,
    buffer: *mut c_void,
    count: usize,
    effect: unsafe extern "C" fn(c_int, *mut c_void, usize) -> isize,
) -> isize {
    let fd = owner.fd.get();
    // SAFETY: caller keeps its writable buffer alive through the operation.
    unsafe { effect(fd, buffer, count) }
}
/// # Safety
/// buffer lends count readable bytes through the coroutine call.
/// effect is the configured native write operation, with that same buffer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_write(
    owner: &AsyncFileRecord,
    buffer: *const c_void,
    count: usize,
    effect: unsafe extern "C" fn(c_int, *const c_void, usize) -> isize,
) -> isize {
    let fd = owner.fd.get();
    // SAFETY: caller keeps its readable bytes alive through the operation.
    unsafe { effect(fd, buffer, count) }
}
/// # Safety
/// owner stays live until this coroutine operation returns; effect has the
/// configured native syscall contract and cannot destroy the owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_sync(
    owner: &AsyncFileRecord,
    effect: unsafe extern "C" fn(c_int) -> c_int,
) -> bool {
    let fd = owner.fd.get();
    // SAFETY: the caller keeps the descriptor's lifetime synchronized.
    unsafe { effect(fd) == 0 }
}
/// # Safety
/// owner stays live until this coroutine operation returns; effect has the
/// configured native syscall contract and cannot destroy the owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_truncate(
    owner: &AsyncFileRecord,
    length: off_t,
    effect: unsafe extern "C" fn(c_int, off_t) -> c_int,
) -> bool {
    let fd = owner.fd.get();
    // SAFETY: the caller synchronizes its live descriptor through the operation.
    unsafe { effect(fd, length) == 0 }
}
/// # Safety
/// info lends writable native stat storage through the call;
/// effect has the native stat contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_stat(
    owner: &AsyncFileRecord,
    info: *mut libc::stat,
    effect: unsafe extern "C" fn(c_int, *mut libc::stat) -> c_int,
) -> bool {
    let fd = owner.fd.get();
    // SAFETY: the caller lends correctly aligned writable native stat storage.
    unsafe { effect(fd, info) == 0 }
}
/// # Safety
/// owner remains live and descriptor lifetime synchronized through the call;
/// effect has the configured native lseek contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_offset(
    owner: &AsyncFileRecord,
    offset: off_t,
    whence: c_int,
    effect: unsafe extern "C" fn(c_int, off_t, c_int) -> off_t,
) -> off_t {
    let fd = owner.fd.get();
    // SAFETY: descriptor and offset have the native lseek contract.
    unsafe { effect(fd, offset, whence) }
}
