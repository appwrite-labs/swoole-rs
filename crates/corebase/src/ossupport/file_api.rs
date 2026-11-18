// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/os/file.cc (Swoole contributors).
//! The bodies of `src/os/file.cc` that Rust still calls: `swoole_tmpfile` (under its
//! upstream symbol), `file_get_contents` and
//! `file_put_contents`. Logs name the function as upstream's `__SW_FUNC__` did.
use crate::buffer::string::StringBuffer;
use crate::log::LogLevel::{SW_LOG_TRACE as LOG_TRACE, SW_LOG_WARNING as LOG_WARNING};
use crate::ossupport::file::{FileIo, file_open, file_size, write_all};
use crate::ossupport::file_record::FileRecord;
use crate::{log, sys};
use libc::c_int;
use std::ffi::CStr;

/// `SW_LOG_TRACE`, `SW_LOG_WARNING`, `SW_ERROR_FILE_TOO_LARGE` and `SW_ERROR_FILE_EMPTY`.
const FILE_TOO_LARGE: c_int = swoole_rs_error::ErrorCode::SW_ERROR_FILE_TOO_LARGE as c_int;
const FILE_EMPTY: c_int = swoole_rs_error::ErrorCode::SW_ERROR_FILE_EMPTY as c_int;

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

/// `file_get_size(int)`: a regular file's size, or `None` with the last error set.
fn size_of_fd(fd: c_int) -> Option<usize> {
    match file_size(fd) {
        Ok(size) => usize::try_from(size).ok(),
        Err(error) => {
            log::set_last_error(error.raw_os_error().unwrap_or_default());
            None
        }
    }
}

/// `file_get_contents`: true with the file's bytes, NUL-terminated, in `content`
/// (a default-constructed String); false after upstream's log.
pub fn file_get_contents(path: &CStr, content: &mut StringBuffer) -> bool {
    let name = path.to_string_lossy();
    let Ok(fd) = sys::open(path, libc::O_RDONLY, 0) else {
        log::sys_warning(c"file_get_contents", format_args!("open('{name}') failed"));
        return false;
    };
    let record = FileRecord::adopt(fd, libc::O_RDONLY);
    let Some(size) = size_of_fd(fd) else {
        return false;
    };
    if size == 0 {
        log::error_log(
            LOG_TRACE,
            FILE_EMPTY,
            c"file_get_contents",
            format_args!("file[{name}] is empty"),
        );
        return false;
    }
    if size > crate::swr_v1_core_max_file_content() as usize {
        log::error_log(
            LOG_WARNING,
            FILE_TOO_LARGE,
            c"file_get_contents",
            format_args!("file[{name}] is too large"),
        );
        return false;
    }
    content.alloc(size + 1, None);
    content.read_zeroed(size, |buf| {
        let io = record.read_all(buf);
        warn_io(
            c"File::read_all",
            "pread",
            fd,
            buf.as_ptr().cast(),
            size,
            io,
        );
        io.done
    });
    content.set_null_terminated();
    true
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
    if data.len() > crate::swr_v1_core_max_file_content() as usize {
        log::error_log(
            LOG_WARNING,
            FILE_TOO_LARGE,
            c"file_put_contents",
            format_args!("content is too large"),
        );
        return false;
    }
    let flags = libc::O_WRONLY | libc::O_TRUNC | libc::O_CREAT;
    let Ok(fd) = file_open(-1, path, flags, 0o666) else {
        let name = path.to_string_lossy();
        log::sys_warning(c"file_put_contents", format_args!("open('{name}') failed"));
        return false;
    };
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
