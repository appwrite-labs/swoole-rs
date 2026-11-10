//! File descriptor, flags and path ownership. Native keeps only a `std::string`
//! view for its reference-returning API; guarded view updates never unwind into Rust.
use crate::file::{FileIo, file_close, file_open, read_all, write_all};
use libc::{c_char, c_int, c_void, off_t};
use std::ffi::CStr;
use std::{ptr, slice};
use swoole_rs_corebase::sys;

pub struct FileRecord {
    fd: c_int,
    flags: c_int,
    path: Vec<u8>,
}
impl FileRecord {
    /// Empty owner used before publishing a copied request descriptor.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            fd: -1,
            flags: 0,
            path: Vec::new(),
        }
    }

    /// Own `fd`, opened with `flags`, with no path (a File whose path native keeps).
    #[must_use]
    pub const fn adopt(fd: c_int, flags: c_int) -> Self {
        Self {
            fd,
            flags,
            path: Vec::new(),
        }
    }

    /// Open the request's file and own its exact path bytes.
    #[must_use]
    pub fn open_readonly(path: &CStr) -> Option<Self> {
        let path_bytes = path_copy(path.to_bytes())?;
        let fd = file_open(-1, path, libc::O_RDONLY, 0);
        let file = Self {
            fd,
            flags: 0,
            path: path_bytes,
        };
        Some(file)
    }

    /// Value-copy without duplicating the descriptor, as the native interface does.
    #[must_use]
    pub fn try_copy(&self) -> Option<Self> {
        Some(Self {
            fd: self.fd,
            flags: self.flags,
            path: path_copy(&self.path)?,
        })
    }

    #[must_use]
    pub const fn descriptor(&self) -> c_int {
        self.fd
    }

    /// The path it was opened with (`File::get_path()`).
    #[must_use]
    pub fn path(&self) -> &[u8] {
        &self.path
    }

    /// `File::write_all`, appending when opened with `O_APPEND`.
    #[must_use]
    pub fn write_all(&self, data: &[u8]) -> FileIo {
        write_all(self.fd, self.flags & libc::O_APPEND != 0, data)
    }

    /// `File::read_all` from offset zero.
    #[must_use]
    pub fn read_all(&self, buf: &mut [u8]) -> FileIo {
        read_all(self.fd, buf)
    }
}
impl Drop for FileRecord {
    fn drop(&mut self) {
        if self.fd >= 0 {
            // This record owns the descriptor until close/release.
            sys::close(self.fd);
        }
    }
}
fn path_copy(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut path = Vec::new();
    path.try_reserve_exact(bytes.len()).ok()?;
    path.extend_from_slice(bytes);
    Some(path)
}
/// Native `std::string` assignment, catching and retaining its exception outside Rust.
pub type PathUpdate = unsafe extern "C" fn(*mut c_void) -> c_int;

/// Adopt a descriptor, with zero flags and a path (which may contain embedded NULs).
/// # Safety
/// `path` is readable for `length` bytes, or null when length is zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_file_record_new(
    fd: c_int,
    path: *const c_char,
    length: usize,
) -> *mut FileRecord {
    let bytes = if length == 0 {
        &[]
    } else {
        // SAFETY: the caller lends the complete path bytes.
        unsafe { slice::from_raw_parts(path.cast::<u8>(), length) }
    };
    let Some(path) = path_copy(bytes) else {
        return ptr::null_mut();
    };
    Box::into_raw(Box::new(FileRecord { fd, flags: 0, path }))
}
/// Value-copy the descriptor/flags/path, matching File's value-copy interface.
/// Each resulting record must be released once.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_file_record_clone(source: &FileRecord) -> *mut FileRecord {
    let Some(path) = path_copy(&source.path) else {
        return ptr::null_mut();
    };
    Box::into_raw(Box::new(FileRecord {
        fd: source.fd,
        flags: source.flags,
        path,
    }))
}
/// Destroy the record (null or new/clone's allocation, released once) and close a
/// still-owned nonnegative descriptor.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_file_record_free(owner: Option<Box<FileRecord>>) {
    drop(owner);
}
/// Current descriptor; ready is fd != -1, and release writes -1.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_file_record_fd(owner: &mut FileRecord, release: bool) -> c_int {
    let fd = owner.fd;
    if release {
        owner.fd = -1;
    }
    fd
}
/// Close invalidates the record before the syscall, including its failure path.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_file_record_close(owner: &mut FileRecord) -> bool {
    file_close(swr_v1_os_file_record_fd(owner, true))
}
/// Open closes the old descriptor, assigns the native path view, then commits flags.
/// -2 is allocation refusal; a native path exception is retained by `update`.
/// # Safety
/// Path has length+1 readable bytes ending in NUL. `update` only updates native path
/// storage (never the record), catches all exceptions, and never reenters Rust.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_file_record_open(
    owner: &mut FileRecord,
    path: *const c_char,
    length: usize,
    flags: c_int,
    mode: c_int,
    update: PathUpdate,
    context: *mut c_void,
) -> c_int {
    // SAFETY: a NUL-terminated path, as the caller promises.
    let fd = file_open(owner.fd, unsafe { CStr::from_ptr(path) }, flags, mode);
    owner.fd = fd;
    // SAFETY: the complete caller-owned path bytes are readable.
    let bytes = unsafe { slice::from_raw_parts(path.cast::<u8>(), length) };
    let Some(path) = path_copy(bytes) else {
        return -2;
    };
    // SAFETY: native guard neither reenters Rust nor unwinds; no record borrow is held.
    if unsafe { update(context) } != 0 {
        return -1;
    }
    owner.path = path;
    owner.flags = flags;
    c_int::from(fd != -1)
}
/// Copy assignment publishes fd/flags before the native string assignment.
/// # Safety
/// Distinct live records, exclusive target; update obeys the open contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_file_record_assign(
    owner: *mut FileRecord,
    source: *const FileRecord,
    update: PathUpdate,
    context: *mut c_void,
) -> c_int {
    if ptr::eq(owner, source) {
        return 0;
    }
    let (fd, flags, path) = {
        // SAFETY: source remains live and distinct from owner.
        let source = unsafe { &*source };
        (source.fd, source.flags, path_copy(&source.path))
    };
    // SAFETY: exclusive target; descriptor value-copy does not close its old value.
    let record = unsafe { &mut *owner };
    record.fd = fd;
    record.flags = flags;
    let Some(path) = path else {
        return -2;
    };
    // SAFETY: native update catches all exceptions and cannot reenter this owner.
    if unsafe { update(context) } != 0 {
        return -1;
    }
    // SAFETY: exclusive target, observed only after the native effect returned.
    unsafe { (*owner).path = path };
    0
}
/// File read/write/pread/pwrite. Flags and descriptor are read directly from Rust state.
/// # Safety
/// Buffer denotes length writable bytes for reads/readable bytes for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_file_record_io(
    owner: &FileRecord,
    operation: u32,
    buffer: *mut c_void,
    length: usize,
    offset: off_t,
) -> isize {
    let fd = owner.fd;
    match operation {
        // SAFETY: caller supplies the writable span.
        0 => unsafe { libc::read(fd, buffer, length) },
        // SAFETY: caller supplies the readable span.
        1 => unsafe { libc::write(fd, buffer.cast_const(), length) },
        // SAFETY: caller supplies the writable span and offset.
        2 => unsafe { libc::pread(fd, buffer, length, offset) },
        // SAFETY: caller supplies the readable span and offset.
        3 => unsafe { libc::pwrite(fd, buffer.cast_const(), length, offset) },
        _ => -1,
    }
}
/// fsync, truncate, seek/set, seek/current and flock control operations.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_file_record_control(
    owner: &FileRecord,
    operation: u32,
    value: off_t,
) -> off_t {
    let fd = owner.fd;
    match operation {
        // SAFETY: syscall on the record's descriptor.
        0 => off_t::from(unsafe { libc::fsync(fd) }),
        // SAFETY: syscall on the record's descriptor.
        1 => off_t::from(unsafe { libc::ftruncate(fd, value) }),
        // SAFETY: syscall on the record's descriptor.
        2 => unsafe { libc::lseek(fd, value, libc::SEEK_SET) },
        // SAFETY: syscall on the record's descriptor.
        3 => unsafe { libc::lseek(fd, 0, libc::SEEK_CUR) },
        // SAFETY: syscall on the record's descriptor, native operation is an int.
        4 => off_t::from(unsafe { libc::flock(fd, c_int::try_from(value).unwrap_or(-1)) }),
        _ => -1,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::os::fd::IntoRawFd;

    unsafe extern "C" fn accept(_: *mut c_void) -> c_int {
        0
    }
    unsafe extern "C" fn refuse(_: *mut c_void) -> c_int {
        1
    }

    fn temporary() -> (CString, c_int) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let name = format!(
            "{}/swoole-rs-file-record-{}-{}",
            std::env::temp_dir().display(),
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&name)
            .unwrap();
        (CString::new(name).unwrap(), file.into_raw_fd())
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri can't spawn a process")]
    fn owner_io_copy_release_and_close() {
        if crate::isolated_process::run_in_child(
            "file_record::tests::owner_io_copy_release_and_close",
        ) {
            return;
        }
        let (path, fd) = temporary();
        unsafe {
            let owner = swr_v1_os_file_record_new(fd, path.as_ptr(), path.as_bytes().len());
            assert!(!owner.is_null());
            let data = b"record-owned IO";
            assert_eq!(
                swr_v1_os_file_record_io(
                    &*owner,
                    1,
                    data.as_ptr().cast_mut().cast(),
                    data.len(),
                    0
                ),
                15
            );
            assert_eq!(swr_v1_os_file_record_control(&*owner, 3, 0), 15);
            let mut received = [0; 15];
            assert_eq!(
                (*owner).read_all(&mut received),
                FileIo {
                    done: 15,
                    failed: false
                }
            );
            assert_eq!(&received, data);
            assert_eq!(swr_v1_os_file_record_control(&*owner, 3, 0), 15);
            let copy = swr_v1_os_file_record_clone(&*owner);
            assert_eq!((*copy).path, path.as_bytes());
            assert_eq!(swr_v1_os_file_record_fd(&mut *copy, true), fd);
            swr_v1_os_file_record_free(Some(Box::from_raw(copy)));
            assert_ne!(libc::fcntl(fd, libc::F_GETFD), -1);
            assert!(swr_v1_os_file_record_close(&mut *owner));
            assert!(!swr_v1_os_file_record_close(&mut *owner));
            assert_eq!(swr_v1_os_file_record_fd(&mut *owner, false), -1);
            swr_v1_os_file_record_free(Some(Box::from_raw(owner)));
            assert_eq!(libc::fcntl(fd, libc::F_GETFD), -1);
            libc::unlink(path.as_ptr());
        }
    }

    #[test]
    fn open_commits_path_and_flags_after_native_view_succeeds() {
        let (path, fd) = temporary();
        unsafe {
            let owner = swr_v1_os_file_record_new(fd, c"old".as_ptr(), 3);
            assert_eq!(
                swr_v1_os_file_record_open(
                    &mut *owner,
                    path.as_ptr(),
                    path.as_bytes().len(),
                    libc::O_RDWR | libc::O_APPEND,
                    0,
                    refuse,
                    ptr::null_mut()
                ),
                -1
            );
            assert_eq!((*owner).path, b"old");
            assert_eq!((*owner).flags, 0);
            assert_ne!((*owner).fd, -1);
            assert_eq!(
                swr_v1_os_file_record_open(
                    &mut *owner,
                    path.as_ptr(),
                    path.as_bytes().len(),
                    libc::O_RDWR | libc::O_APPEND,
                    0,
                    accept,
                    ptr::null_mut()
                ),
                1
            );
            assert_eq!((*owner).path, path.as_bytes());
            assert_eq!((*owner).flags, libc::O_RDWR | libc::O_APPEND);
            assert_eq!((*owner).write_all(b"ab").done, 2);
            assert_eq!(swr_v1_os_file_record_control(&*owner, 3, 0), 2);
            swr_v1_os_file_record_free(Some(Box::from_raw(owner)));
            libc::unlink(path.as_ptr());
        }
    }

    #[test]
    fn assignment_preserves_value_copy_and_native_exception_order() {
        let (first_path, first_fd) = temporary();
        let (second_path, second_fd) = temporary();
        unsafe {
            let source = swr_v1_os_file_record_new(
                first_fd,
                first_path.as_ptr(),
                first_path.as_bytes().len(),
            );
            let target = swr_v1_os_file_record_new(
                second_fd,
                second_path.as_ptr(),
                second_path.as_bytes().len(),
            );
            assert_eq!(
                swr_v1_os_file_record_assign(target, source, refuse, ptr::null_mut()),
                -1
            );
            assert_eq!((*target).fd, first_fd);
            assert_eq!((*target).path, second_path.as_bytes());
            assert_ne!(libc::fcntl(second_fd, libc::F_GETFD), -1);
            assert_eq!(
                swr_v1_os_file_record_assign(target, source, accept, ptr::null_mut()),
                0
            );
            assert_eq!((*target).path, first_path.as_bytes());
            swr_v1_os_file_record_fd(&mut *target, true);
            swr_v1_os_file_record_free(Some(Box::from_raw(target)));
            swr_v1_os_file_record_free(Some(Box::from_raw(source)));
            libc::close(second_fd);
            libc::unlink(first_path.as_ptr());
            libc::unlink(second_path.as_ptr());
        }
    }
    #[test]
    #[cfg_attr(miri, ignore = "Miri can't spawn a process")]
    fn owner_drop_preserves_a_reopened_file_descriptor_number() {
        if crate::isolated_process::run_in_child(
            "file_record::tests::owner_drop_preserves_a_reopened_file_descriptor_number",
        ) {
            return;
        }
        unsafe {
            let fd = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
            assert!(fd >= 0);
            let owner = swr_v1_os_file_record_new(fd, ptr::null(), 0);
            assert!(swr_v1_os_file_record_close(&mut *owner));
            assert_eq!(swr_v1_os_file_record_fd(&mut *owner, false), -1);
            crate::isolated_process::replacement_survives(fd, || {
                swr_v1_os_file_record_free(Some(Box::from_raw(owner)));
            });
        }
    }
}
