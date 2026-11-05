// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/os/file.cc (Swoole contributors).
//! The bodies of `src/os/file.cc`: `swoole_tmpfile`, `file_get_size`, `File::stat`,
//! `File::write_all` and `File::read_all` under their upstream symbols (with
//! `upstream-symbols`), and the `std::string` helpers, which the header calls with
//! the string's `c_str()`. Logs name the function as upstream's `__SW_FUNC__` did.
use crate::file::{FileIo, swr_v1_os_file_open, swr_v1_os_file_size, swr_v1_os_tmpfile};
use crate::file_record::{FileRecord, swr_v1_os_file_record_all, swr_v1_os_file_record_stat};
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

/// `swoole::File`, whose first member is its Rust record (the header keeps it first).
#[repr(C)]
pub struct CppFile {
    pub owner: *mut FileRecord,
}

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

/// `ssize_t swoole::file_get_size(int fd)`.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole13file_get_sizeEi")
)]
pub extern "C" fn file_get_size(fd: c_int) -> isize {
    size_of_fd(fd)
}

/// `ssize_t swoole::file_get_size(FILE *fp)`: flushed first.
///
/// # Safety
/// `fp` is an open stream.
#[cfg_attr(
    all(feature = "upstream-symbols", target_os = "macos"),
    unsafe(export_name = "_ZN6swoole13file_get_sizeEP7__sFILE")
)]
#[cfg_attr(
    all(feature = "upstream-symbols", not(target_os = "macos")),
    unsafe(export_name = "_ZN6swoole13file_get_sizeEP8_IO_FILE")
)]
pub unsafe extern "C" fn file_get_size_stream(fp: *mut libc::FILE) -> isize {
    // SAFETY: as the caller promises.
    unsafe { libc::fflush(fp) };
    // SAFETY: as above.
    size_of_fd(unsafe { libc::fileno(fp) })
}

/// `bool File::stat(FileStatus *) const`.
///
/// # Safety
/// `this` is a live File; `output` is writable.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole4File4statEP4stat")
)]
pub unsafe extern "C" fn file_stat(this: *const CppFile, output: *mut libc::stat) -> bool {
    // SAFETY: as the caller promises.
    let owner = unsafe { &*this }.owner;
    // SAFETY: as above.
    if unsafe { swr_v1_os_file_record_stat(owner, output) } {
        return true;
    }
    log::sys_warning(c"File::stat", format_args!("fstat() failed"));
    false
}

/// `size_t File::write_all(const void *, size_t) const`.
///
/// # Safety
/// `this` is a live File; `data` holds `length` bytes.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole4File9write_allEPKvm")
)]
pub unsafe extern "C" fn file_write_all(
    this: *const CppFile,
    data: *const c_void,
    length: usize,
) -> usize {
    // SAFETY: as the caller promises.
    let owner = unsafe { &*this }.owner;
    // SAFETY: as above.
    unsafe { transfer(owner, true, data.cast_mut(), length) }
}

/// `size_t File::read_all(void *, size_t) const`.
///
/// # Safety
/// `this` is a live File; `data` has `length` writable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole4File8read_allEPvm")
)]
pub unsafe extern "C" fn file_read_all(
    this: *const CppFile,
    data: *mut c_void,
    length: usize,
) -> usize {
    // SAFETY: as the caller promises.
    let owner = unsafe { &*this }.owner;
    // SAFETY: as above.
    unsafe { transfer(owner, false, data, length) }
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

/// `file_get_size(const std::string &)`.
///
/// # Safety
/// `path` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_file_get_size_path(path: *const c_char) -> isize {
    // SAFETY: as the caller promises.
    let fd = unsafe { libc::open(path, libc::O_RDONLY) };
    if fd < 0 {
        log::set_last_error(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
        return -1;
    }
    let size = size_of_fd(fd);
    // SAFETY: the descriptor opened above.
    unsafe { libc::close(fd) };
    size
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

/// `file_exists`.
///
/// # Safety
/// `path` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_file_exists(path: *const c_char) -> bool {
    // SAFETY: as the caller promises.
    unsafe { libc::access(path, libc::F_OK) == 0 }
}
