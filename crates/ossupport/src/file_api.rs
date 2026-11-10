// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/os/file.cc (Swoole contributors).
//! The bodies of `src/os/file.cc` that Rust and `native/ossupport/swoole_file.h`
//! still call: `swoole_tmpfile` (under its upstream symbol with `upstream-symbols`),
//! `File::read_content`, `file_get_contents` and `file_put_contents`. Logs name
//! the function as upstream's `__SW_FUNC__` did.
use crate::file::{FileIo, file_open, file_size, tmpfile, write_all};
use crate::file_record::FileRecord;
use libc::{c_char, c_int};
use std::ffi::CStr;
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::{log, sys};

/// `SW_LOG_TRACE`, `SW_LOG_WARNING`, `SW_ERROR_FILE_TOO_LARGE` and `SW_ERROR_FILE_EMPTY`.
const LOG_TRACE: c_int = 1;
const LOG_WARNING: c_int = 4;
const FILE_TOO_LARGE: c_int = 701;
const FILE_EMPTY: c_int = 702;
/// `SW_BUFFER_SIZE_STD`.
const BUFFER_SIZE_STD: usize = 8192;

/// `File::read_all`/`write_all`'s warning on an IO error.
fn warn_io(origin: &CStr, call: &str, fd: c_int, data: *const u8, total: usize, io: FileIo) {
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

/// `File::write_all` on a bare descriptor, opened with `APPEND` or not: the bytes
/// written, with upstream's `pwrite` warning on an IO error.
#[must_use]
pub fn fd_write_all(fd: c_int, append: bool, data: &[u8]) -> usize {
    let io = write_all(fd, append, data);
    warn_io(
        c"File::write_all",
        "pwrite",
        fd,
        data.as_ptr(),
        data.len(),
        io,
    );
    io.done
}

/// `file_get_size(int)`: a regular file's size, or -1 with the last error set.
fn size_of_fd(fd: c_int) -> isize {
    let size = file_size(fd);
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
    let fd = unsafe { tmpfile(filename) };
    if fd < 0 {
        // SAFETY: still the caller's NUL-terminated template.
        let name = unsafe { CStr::from_ptr(filename) }.to_string_lossy();
        log::sys_warning(c"swoole_tmpfile", format_args!("mkstemp('{name}') failed"));
        return -1;
    }
    fd
}

/// `file_get_contents`: true with the file's bytes, NUL-terminated, in `content`
/// (a default-constructed String); false after upstream's log.
pub fn file_get_contents(path: &CStr, content: &mut CppString) -> bool {
    let fd = sys::open(path, libc::O_RDONLY, 0);
    let name = path.to_string_lossy();
    if fd < 0 {
        log::sys_warning(c"file_get_contents", format_args!("open('{name}') failed"));
        return false;
    }
    let record = FileRecord::adopt(fd, libc::O_RDONLY);
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
    content.alloc(size + 1, std::ptr::null());
    let buf = content.spare().get_mut(..size).unwrap_or_default();
    let io = record.read_all(buf);
    warn_io(c"File::read_all", "pread", fd, buf.as_ptr(), size, io);
    content.length = io.done;
    content.set_null_terminated();
    true
}

/// `file_get_contents` for `swoole_file.h`, which the logger originals compile.
///
/// # Safety
/// `path` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_file_get_contents(
    path: *const c_char,
    content: &mut CppString,
) -> bool {
    // SAFETY: as the caller promises.
    let path = unsafe { CStr::from_ptr(path) };
    file_get_contents(path, content)
}

/// `File::read_content`: reads to EOF into `content`, a fresh `String(SW_BUFFER_SIZE_STD)`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_file_read_content(owner: &FileRecord, content: &mut CppString) {
    let fd = owner.descriptor();
    content.alloc(BUFFER_SIZE_STD, std::ptr::null());
    loop {
        let n = sys::read(fd, content.spare());
        if n <= 0 {
            break;
        }
        content.grow(n.unsigned_abs());
    }
}

/// `file_put_contents`.
#[expect(
    clippy::must_use_candidate,
    reason = "upstream's callers ignore file_put_contents's result"
)]
pub fn file_put_contents(path: &CStr, data: &[u8]) -> bool {
    if data.is_empty() {
        log::error_log(
            LOG_WARNING,
            FILE_EMPTY,
            c"file_put_contents",
            format_args!("content is empty"),
        );
        return false;
    }
    if data.len() > swoole_rs_corebase::swr_v1_core_max_file_content() as usize {
        log::error_log(
            LOG_WARNING,
            FILE_TOO_LARGE,
            c"file_put_contents",
            format_args!("content is too large"),
        );
        return false;
    }
    let flags = libc::O_WRONLY | libc::O_TRUNC | libc::O_CREAT;
    let fd = file_open(-1, path, flags, 0o666);
    if fd < 0 {
        let name = path.to_string_lossy();
        log::sys_warning(c"file_put_contents", format_args!("open('{name}') failed"));
        return false;
    }
    let record = FileRecord::adopt(fd, flags);
    let io = record.write_all(data);
    warn_io(
        c"File::write_all",
        "pwrite",
        fd,
        data.as_ptr(),
        data.len(),
        io,
    );
    io.done != 0
}
