//! Upstream's `core-tests/src/os/file.cpp`, one function per TEST with the same inputs
//! and expected values, over crates/ossupport's `file_api` and `file` (the bodies of
//! `src/os/file.cc`). `ASSERT_ERREQ` reads the last error, which the log host
//! `swoole_init` installs sets, so they run here. tests/modules/ossupport/README.md maps
//! each TEST, including the ones with no subject left.

use std::ffi::{CStr, c_char, c_int};

use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::log;
use swoole_rs_corebase::{swr_v1_core_max_file_content, swr_v1_core_set_max_file_content};
use swoole_rs_ossupport::{
    FileRecord, fd_write_all, file_get_contents, file_put_contents, file_size, swoole_tmpfile,
    swr_v1_os_file_record_fd,
};

use super::{EBADF, ENOENT, TEST_STR, last_error};

const EISDIR: c_int = 21;
const ESPIPE: c_int = 29;
const SW_ERROR_FILE_TOO_LARGE: c_int = 701;
const SW_ERROR_FILE_EMPTY: c_int = 702;
/// `SW_MAX_FILE_CONTENT`.
const SW_MAX_FILE_CONTENT: u32 = 64 * 1024 * 1024;

unsafe extern "C" {
    fn pipe(fds: *mut c_int) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn __errno_location() -> *mut c_int;
}

fn set_errno(value: c_int) {
    // SAFETY: this thread's errno.
    unsafe { *__errno_location() = value };
}

fn errno() -> c_int {
    // SAFETY: this thread's errno.
    unsafe { *__errno_location() }
}

fn get_contents(path: &CStr) -> Option<CppString> {
    let mut content = CppString::EMPTY;
    file_get_contents(path, &mut content).then_some(content)
}

fn put_contents() {
    let filename = c"/tmp/not-exists-dir/test.txt";

    assert!(!file_put_contents(filename, &TEST_STR[..0]));
    assert_eq!(last_error(), SW_ERROR_FILE_EMPTY);

    // `file_put_contents(filename, TEST_STR, SwooleG.max_file_content + 1)`: only the
    // length is read before the check fails.
    let too_large = vec![0_u8; swr_v1_core_max_file_content() as usize + 1];
    assert!(!file_put_contents(filename, &too_large));
    assert_eq!(last_error(), SW_ERROR_FILE_TOO_LARGE);

    assert!(!file_put_contents(filename, TEST_STR));
    assert_eq!(last_error(), ENOENT);
}

fn get_contents_() {
    let filename = c"/tmp/not-exists-dir/test.txt";

    assert!(get_contents(filename).is_none());
    assert_eq!(last_error(), ENOENT);

    assert!(get_contents(c"/tmp").is_none());
    assert_eq!(last_error(), EISDIR);

    let empty_file = c"/tmp/empty-file.txt";
    std::fs::File::create("/tmp/empty-file.txt").expect("create the empty file");

    assert!(get_contents(empty_file).is_none());
    assert_eq!(last_error(), SW_ERROR_FILE_EMPTY);
    std::fs::remove_file("/tmp/empty-file.txt").expect("remove the empty file");

    // Upstream's large file is its core-tests binary; this one is this binary.
    let large_file = std::env::current_exe().expect("this binary's path");
    let large_file = std::ffi::CString::new(large_file.into_os_string().into_encoded_bytes())
        .expect("a path without NUL");
    swr_v1_core_set_max_file_content(1024 * 1024);
    assert!(get_contents(&large_file).is_none());
    assert_eq!(last_error(), SW_ERROR_FILE_TOO_LARGE);
    swr_v1_core_set_max_file_content(SW_MAX_FILE_CONTENT);
}

/// `file_get_size(const std::string &)` left the build with its callers, so its
/// ENOENT check has no subject; `file_get_size(int)` is `file::file_size`, whose
/// error the caller publishes.
fn get_size() {
    let size = file_size(9999);
    assert_eq!(size.size, -1);
    assert_eq!(size.error, EBADF);

    let fd = std::fs::File::open("/tmp").expect("open /tmp");
    let size = file_size(std::os::fd::AsRawFd::as_raw_fd(&fd));
    assert_eq!(size.size, -1);
    assert_eq!(size.error, EISDIR);
}

/// `File::read_all` is `FileRecord::read_all`, whose callers warn (and so set the last
/// error): its failure and errno are what the warning would publish. `File::write_all` is
/// `fd_write_all`, which warns itself. `File::stat` is `file_size`'s `fstat`.
fn error() {
    let mut fds = [-1; 2];
    // SAFETY: two writable descriptors.
    assert_eq!(unsafe { pipe(fds.as_mut_ptr()) }, 0);
    let fp = FileRecord::adopt(fds[0], 0);
    let mut buf = vec![0_u8; 65536];
    let io = fp.read_all(&mut buf);
    assert_eq!(io.done, 0);
    assert!(io.failed);
    assert_eq!(errno(), ESPIPE);

    assert_eq!(fd_write_all(fds[0], false, TEST_STR), 0);
    assert_eq!(last_error(), ESPIPE);

    // SAFETY: the pipe's own descriptors, closed once.
    unsafe {
        close(fds[0]);
        close(fds[1]);
    }

    assert_eq!(file_size(fp.descriptor()).error, EBADF);

    let mut fp = fp;
    assert_eq!(swr_v1_os_file_record_fd(&mut fp, true), fds[0]);
}

/// `make_tmpfile` with `task_tmpfile` there is crates/process's
/// `task_file::tests::make_tmpfile_in_a_missing_directory`.
fn tmp_file() {
    let mut buf = [0 as c_char; 128];
    for (slot, &byte) in buf.iter_mut().zip(b"/tmp/not-exists-dir/test.XXXXXX") {
        *slot = byte as c_char;
    }
    // SAFETY: a writable NUL-terminated template.
    assert_eq!(unsafe { swoole_tmpfile(buf.as_mut_ptr()) }, -1);
    assert_eq!(last_error(), ENOENT);
}

fn empty_file() {
    let fname = c"/tmp/swoole_empty_file.txt";
    std::fs::File::create("/tmp/swoole_empty_file.txt").expect("create the file");

    let fp = FileRecord::open_readonly(fname).expect("the path fits");
    let mut buf = [0_u8; 128];
    assert_eq!(fp.read_all(&mut buf).done, 0);
    log::set_last_error(0);
    set_errno(0);
    assert_eq!(last_error(), 0);
    assert_eq!(errno(), 0);
    drop(fp);

    std::fs::remove_file("/tmp/swoole_empty_file.txt").expect("remove the file");
}

/// `file.<name>`, in upstream's order; `file.open_twice` is tests/modules/ossupport's
/// `ossupport-file-owner`.
pub(super) const CASES: super::Cases = &[
    ("file_put_contents", put_contents),
    ("file_get_contents", get_contents_),
    ("file_get_size", get_size),
    ("error", error),
    ("tmp_file", tmp_file),
    ("empty_file", empty_file),
];
