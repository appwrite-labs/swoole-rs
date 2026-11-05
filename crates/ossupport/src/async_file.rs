//! `AsyncFile`'s descriptor ownership. Operations call the
//! existing coroutine C ABI, which selects its configured pool/ring/native lane.
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
/// the coroutine open. No owner access runs concurrently while that call waits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_open(
    owner: *mut AsyncFileRecord,
    path: *const c_char,
    flags: c_int,
    mode: mode_t,
) -> bool {
    // SAFETY: caller keeps its terminated path alive through the coroutine effect.
    let fd = unsafe { coroutine::swoole_coroutine_open(path, flags, mode) };
    // SAFETY: no borrow spans the effect; caller still lends the live owner exclusively.
    unsafe { (*owner).fd = fd };
    fd > 0
}
/// # Safety
/// owner is live; buffer lends count writable bytes through the coroutine call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_read(
    owner: *const AsyncFileRecord,
    buffer: *mut c_void,
    count: usize,
) -> isize {
    // SAFETY: snapshot before calling the existing coroutine ABI.
    let fd = unsafe { (*owner).fd };
    // SAFETY: caller keeps its writable buffer alive through the operation.
    unsafe { coroutine::swoole_coroutine_read(fd, buffer, count) }
}
/// # Safety
/// owner is live; buffer lends count readable bytes through the coroutine call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_write(
    owner: *const AsyncFileRecord,
    buffer: *const c_void,
    count: usize,
) -> isize {
    // SAFETY: copy the descriptor without retaining a record borrow across IO.
    let fd = unsafe { (*owner).fd };
    // SAFETY: caller keeps its readable bytes alive through the operation.
    unsafe { coroutine::swoole_coroutine_write(fd, buffer, count) }
}
/// # Safety
/// owner is live/stable until this coroutine operation returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_sync(owner: *const AsyncFileRecord) -> bool {
    // SAFETY: copy the descriptor before any coroutine effect.
    let fd = unsafe { (*owner).fd };
    // SAFETY: the caller keeps the descriptor's lifetime synchronized.
    unsafe { coroutine::swoole_coroutine_fsync(fd) == 0 }
}
/// # Safety
/// owner is live/stable until this coroutine operation returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_truncate(
    owner: *const AsyncFileRecord,
    length: off_t,
) -> bool {
    // SAFETY: copy before entering the coroutine ABI.
    let fd = unsafe { (*owner).fd };
    // SAFETY: the caller synchronizes its live descriptor through the operation.
    unsafe { coroutine::swoole_coroutine_ftruncate(fd, length) == 0 }
}
/// # Safety
/// owner is live and info lends writable native stat storage through the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_stat(
    owner: *const AsyncFileRecord,
    info: *mut libc::stat,
) -> bool {
    // SAFETY: descriptor snapshot ends before the coroutine effect.
    let fd = unsafe { (*owner).fd };
    // SAFETY: the caller lends correctly aligned writable native stat storage.
    unsafe { coroutine::swoole_coroutine_fstat(fd, info) == 0 }
}
/// # Safety
/// owner remains live and descriptor lifetime synchronized through the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_async_file_offset(
    owner: *const AsyncFileRecord,
    offset: off_t,
    whence: c_int,
) -> off_t {
    // SAFETY: copy the descriptor, retaining no borrow during the coroutine call.
    let fd = unsafe { (*owner).fd };
    // SAFETY: descriptor and offset have the native lseek contract.
    unsafe { coroutine::swoole_coroutine_lseek(fd, offset, whence) }
}

#[cfg(not(test))]
mod coroutine {
    use libc::{c_char, c_int, c_void, mode_t, off_t, stat};
    unsafe extern "C" {
        pub(super) fn swoole_coroutine_open(
            path: *const c_char,
            flags: c_int,
            mode: mode_t,
        ) -> c_int;
        pub(super) fn swoole_coroutine_read(fd: c_int, buffer: *mut c_void, count: usize) -> isize;
        pub(super) fn swoole_coroutine_write(
            fd: c_int,
            buffer: *const c_void,
            count: usize,
        ) -> isize;
        pub(super) fn swoole_coroutine_fsync(fd: c_int) -> c_int;
        pub(super) fn swoole_coroutine_ftruncate(fd: c_int, length: off_t) -> c_int;
        pub(super) fn swoole_coroutine_fstat(fd: c_int, info: *mut stat) -> c_int;
        pub(super) fn swoole_coroutine_lseek(fd: c_int, offset: off_t, whence: c_int) -> off_t;
    }
}

#[cfg(test)]
mod coroutine {
    // Unit tests exercise owned records in the direct-file lane. Integration
    // runs the unchanged original AsyncFile test through the actual hook ABI.
    pub(super) unsafe fn swoole_coroutine_open(
        path: *const libc::c_char,
        flags: libc::c_int,
        mode: libc::mode_t,
    ) -> libc::c_int {
        unsafe { libc::open(path, flags, libc::c_uint::from(mode)) }
    }
    pub(super) use libc::{
        fstat as swoole_coroutine_fstat, fsync as swoole_coroutine_fsync,
        ftruncate as swoole_coroutine_ftruncate, lseek as swoole_coroutine_lseek,
        read as swoole_coroutine_read, write as swoole_coroutine_write,
    };
}
