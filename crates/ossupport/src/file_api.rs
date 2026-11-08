// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/os/file.cc (Swoole contributors).
//! The bodies of `src/os/file.cc` that Rust and `native/ossupport/swoole_file.h` still
//! call: `swoole_tmpfile` (under its upstream symbol with `upstream-symbols`),
//! `File::read_content`, `file_get_contents` and `file_put_contents`. Logs name the
//! function as upstream's `__SW_FUNC__` did.
use crate::file::{
    FileIo, swr_v1_os_file_open, swr_v1_os_file_size, swr_v1_os_file_write_all, swr_v1_os_tmpfile,
};
use crate::file_record::{FileRecord, swr_v1_os_file_record_all};
use libc::{c_char, c_int, c_void};
use std::ffi::CStr;
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::log;

/// `SW_LOG_TRACE`, `SW_LOG_WARNING`, `SW_ERROR_FILE_TOO_LARGE` and `SW_ERROR_FILE_EMPTY`.
const LOG_TRACE: c_int = 1;
const LOG_WARNING: c_int = 4;
const FILE_TOO_LARGE: c_int = 701;
const FILE_EMPTY: c_int = 702;
/// `SW_BUFFER_SIZE_STD`.
const BUFFER_SIZE_STD: usize = 8192;

/// # Safety
/// `path` is a NUL-terminated string.
unsafe fn text<'a>(path: *const c_char) -> std::borrow::Cow<'a, str> {
    // SAFETY: as the caller promises.
    unsafe { CStr::from_ptr(path) }.to_string_lossy()
}

/// `File::read_all`/`write_all`'s warning on an IO error.
fn warn_io(origin: &CStr, call: &str, fd: c_int, data: *const c_void, total: usize, io: FileIo) {
    if io.failed {
        log::sys_warning(
            origin,
            format_args!(
                "{call}({fd}, {data:p}, {}, {}) failed",
                total - io.done,
                io.done
            ),
        );
    }
}

/// `File::write_all` (or `read_all`, `write` false) on a record and its descriptor.
///
/// # Safety
/// `owner` is live; `data` spans `length` bytes in the direction asked.
unsafe fn transfer(owner: *mut FileRecord, write: bool, data: *mut c_void, length: usize) -> usize {
    // SAFETY: as the caller promises.
    let io = unsafe { swr_v1_os_file_record_all(owner, write, data, length) };
    // SAFETY: as above.
    let fd = unsafe { &*owner }.descriptor();
    if write {
        warn_io(c"File::write_all", "pwrite", fd, data, length, io);
    } else {
        warn_io(c"File::read_all", "pread", fd, data, length, io);
    }
    io.done
}

/// `File::write_all` on a bare descriptor, opened with `APPEND` or not: the bytes
/// written, with upstream's `pwrite` warning on an IO error.
///
/// # Safety
/// `data` spans `length` readable bytes.
#[must_use]
pub unsafe fn fd_write_all(fd: c_int, append: bool, data: *const c_void, length: usize) -> usize {
    // SAFETY: as the caller promises.
    let io = unsafe { swr_v1_os_file_write_all(fd, append, data, length) };
    warn_io(c"File::write_all", "pwrite", fd, data, length, io);
    io.done
}

/// `file_get_size(int)`: a regular file's size, or -1 with the last error set.
fn size_of_fd(fd: c_int) -> isize {
    let size = swr_v1_os_file_size(fd);
    if size.error != 0 {
        log::set_last_error(size.error);
    }
    isize::try_from(size.size).unwrap_or(-1)
}

/// `int swoole_tmpfile(char *filename)`.
///
/// # Safety
/// `filename` is a writable NUL-terminated `XXXXXX` template.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z14swoole_tmpfilePc")
)]
pub unsafe extern "C" fn swoole_tmpfile(filename: *mut c_char) -> c_int {
    // SAFETY: as the caller promises.
    let fd = unsafe { swr_v1_os_tmpfile(filename) };
    if fd < 0 {
        // SAFETY: still the caller's NUL-terminated template.
        let name = unsafe { text(filename) };
        log::sys_warning(c"swoole_tmpfile", format_args!("mkstemp('{name}') failed"));
        return -1;
    }
    fd
}

/// `File::read_content`: reads to EOF into `content`, a fresh `String(SW_BUFFER_SIZE_STD)`.
///
/// # Safety
/// `owner` is live; `content` is a live String no one else uses.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_file_read_content(
    owner: *mut FileRecord,
    content: *mut CppString,
) {
    // SAFETY: as the caller promises.
    let content = unsafe { &mut *content };
    content.alloc(BUFFER_SIZE_STD, std::ptr::null());
    // SAFETY: as above.
    let fd = unsafe { &*owner }.descriptor();
    loop {
        let free = content.str.wrapping_add(content.length);
        // SAFETY: the String has `size - length` free bytes there.
        let n = unsafe { libc::read(fd, free.cast(), content.size - content.length) };
        if n <= 0 {
            break;
        }
        content.grow(n.unsigned_abs());
    }
}

/// `file_get_contents`: true with the file's bytes, NUL-terminated, in `content`
/// (a default-constructed String); false after upstream's log.
///
/// # Safety
/// `path` is NUL-terminated; `content` is a live String no one else uses.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_file_get_contents(
    path: *const c_char,
    content: *mut CppString,
) -> bool {
    // SAFETY: as the caller promises.
    let fd = unsafe { libc::open(path, libc::O_RDONLY) };
    // SAFETY: as above.
    let name = unsafe { text(path) };
    if fd < 0 {
        log::sys_warning(c"file_get_contents", format_args!("open('{name}') failed"));
        return false;
    }
    let mut record = FileRecord::adopt(fd, libc::O_RDONLY);
    let size = size_of_fd(fd);
    if size < 0 {
        return false;
    }
    if size == 0 {
        log::error_log(
            LOG_TRACE,
            FILE_EMPTY,
            c"file_get_contents",
            format_args!("file[{name}] is empty"),
        );
        return false;
    }
    if size.unsigned_abs() > swoole_rs_corebase::swr_v1_core_max_file_content() as usize {
        log::error_log(
            LOG_WARNING,
            FILE_TOO_LARGE,
            c"file_get_contents",
            format_args!("file[{name}] is too large"),
        );
        return false;
    }
    let size = size.unsigned_abs();
    // SAFETY: as the caller promises.
    let content = unsafe { &mut *content };
    content.alloc(size + 1, std::ptr::null());
    // SAFETY: a live record over the open descriptor; the String holds `size + 1` bytes.
    let read = unsafe { transfer(&raw mut record, false, content.str.cast(), size) };
    content.length = read;
    // SAFETY: `read <= size`, inside the String's `size + 1` bytes.
    unsafe { content.str.wrapping_add(read).write(0) };
    true
}

/// `file_put_contents`.
///
/// # Safety
/// `path` is NUL-terminated; `data` holds `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_file_put_contents(
    path: *const c_char,
    data: *const c_void,
    length: usize,
) -> bool {
    if length == 0 {
        log::error_log(
            LOG_WARNING,
            FILE_EMPTY,
            c"file_put_contents",
            format_args!("content is empty"),
        );
        return false;
    }
    if length > swoole_rs_corebase::swr_v1_core_max_file_content() as usize {
        log::error_log(
            LOG_WARNING,
            FILE_TOO_LARGE,
            c"file_put_contents",
            format_args!("content is too large"),
        );
        return false;
    }
    let flags = libc::O_WRONLY | libc::O_TRUNC | libc::O_CREAT;
    // SAFETY: as the caller promises.
    let fd = unsafe { swr_v1_os_file_open(-1, path, flags, 0o666) };
    if fd < 0 {
        // SAFETY: as above.
        let name = unsafe { text(path) };
        log::sys_warning(c"file_put_contents", format_args!("open('{name}') failed"));
        return false;
    }
    let mut record = FileRecord::adopt(fd, flags);
    // SAFETY: a live record over the open descriptor; `data` holds `length` bytes.
    unsafe { transfer(&raw mut record, true, data.cast_mut(), length) != 0 }
}
