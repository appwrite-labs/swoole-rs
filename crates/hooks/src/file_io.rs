//! `System::read_file` and `System::write_file`: open, lock, read or write the whole
//! file, sync and unlock, all on the async pool while the coroutine waits, as upstream's
//! lambda over a `File` did. Each warning is logged on the pool thread with the
//! failing call's errno.
use crate::stdio::{Offload, on_pool};
use libc::c_int;
use std::ffi::CStr;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::FileExt;
use swoole_rs_corebase::log;

/// `SW_AIO_WRITE_FSYNC`, tested against the open flags as upstream does.
const WRITE_FSYNC: c_int = 1 << 1;
/// `SW_BUFFER_SIZE_STD`: `File::read_content`'s first buffer.
const READ_CONTENT_SIZE: usize = 8192;

/// Logs a warning: errno, origin, message.
type Warn = fn(c_int, &CStr, &str);

fn sys_warning(error: c_int, origin: &CStr, message: &str) {
    log::sys_warning_errno(error, origin, format_args!("{message}"));
}

fn offset(done: usize) -> u64 {
    u64::try_from(done).unwrap_or(u64::MAX)
}

/// The open file: `flags` as `open(2)` takes them, created 0644.
fn open(file: &CStr, flags: c_int) -> Option<File> {
    let fd = swoole_rs_corebase::sys::open(file, flags, 0o644);
    // SAFETY: a new fd, owned by the `File` from here.
    (fd != -1).then(|| unsafe { File::from_raw_fd(fd) })
}

fn flock(file: &File, operation: c_int) -> bool {
    swoole_rs_corebase::sys::flock(file.as_raw_fd(), operation) == 0
}

/// `file_get_size`: the size of a regular file.
fn regular_size(file: &File) -> Option<usize> {
    let metadata = file.metadata().ok().filter(std::fs::Metadata::is_file)?;
    usize::try_from(metadata.len()).ok()
}

/// `File::read_all` and `File::write_all`'s loop: retry on EINTR/EAGAIN, stop at 0 or
/// an error, which `fail` reports. Returns the bytes done.
fn transfer_all(
    len: usize,
    mut step: impl FnMut(usize) -> std::io::Result<usize>,
    fail: impl FnOnce(c_int, usize),
) -> usize {
    let mut done = 0;
    while done < len {
        match step(done) {
            Ok(0) => break,
            Ok(n) => done += n,
            Err(e) if matches!(e.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) => {}
            Err(e) => {
                fail(e.raw_os_error().unwrap_or(0), done);
                break;
            }
        }
    }
    done
}

/// One whole-file operation, run on the pool thread.
struct Job<'a> {
    file: &'a CStr,
    warn: Warn,
}

impl Job<'_> {
    /// `"{call}({file}{args}) failed"` from `origin`, with the current errno.
    fn warn(&self, origin: &CStr, call: &str, args: &str) {
        let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        let file = self.file.to_string_lossy();
        (self.warn)(error, origin, &format!("{call}({file}{args}) failed"));
    }

    fn read(&self, lock: bool) -> Option<Vec<u8>> {
        let Some(file) = open(self.file, libc::O_RDONLY) else {
            self.warn(c"System::read_file", "open", ", O_RDONLY");
            return None;
        };
        if lock && !flock(&file, libc::LOCK_SH) {
            self.warn(c"System::read_file", "flock", ", LOCK_SH");
            return None;
        }
        let content = match regular_size(&file) {
            Some(size) if size > 0 => self.read_all(&file, size),
            _ => Self::read_content(&file),
        };
        if lock && !flock(&file, libc::LOCK_UN) {
            self.warn(c"System::read_file", "flock", ", LOCK_UN");
        }
        Some(content)
    }

    fn read_all(&self, file: &File, size: usize) -> Vec<u8> {
        let mut buf = vec![0; size];
        let at = buf.as_ptr();
        let read = transfer_all(
            size,
            |done| file.read_at(buf.get_mut(done..).unwrap_or_default(), offset(done)),
            |error, done| {
                let fd = file.as_raw_fd();
                let message = format!("pread({fd}, {at:p}, {}, {done}) failed", size - done);
                (self.warn)(error, c"File::read_all", &message);
            },
        );
        buf.truncate(read);
        buf
    }

    /// `File::read_content`: until end of file or the first error.
    fn read_content(mut file: &File) -> Vec<u8> {
        let mut buf = Vec::with_capacity(READ_CONTENT_SIZE);
        // Upstream stops at the first error, keeping what it read.
        drop(file.read_to_end(&mut buf));
        buf
    }

    fn write(&self, data: &[u8], lock: bool, flags: c_int) -> isize {
        let Some(mut file) = open(self.file, flags) else {
            self.warn(c"System::write_file", "open", &format!(", {flags}"));
            return -1;
        };
        if lock && !flock(&file, libc::LOCK_EX) {
            self.warn(c"System::write_file", "flock", ", LOCK_EX");
            return -1;
        }
        let append = flags & libc::O_APPEND != 0;
        let fd = file.as_raw_fd();
        let written = transfer_all(
            data.len(),
            |done| {
                let rest = data.get(done..).unwrap_or_default();
                if append {
                    file.write(rest)
                } else {
                    file.write_at(rest, offset(done))
                }
            },
            |error, done| {
                let message = format!(
                    "pwrite({fd}, {:p}, {}, {done}) failed",
                    data.as_ptr(),
                    data.len() - done
                );
                (self.warn)(error, c"File::write_all", &message);
            },
        );
        if flags & WRITE_FSYNC != 0 && file.sync_all().is_err() {
            self.warn(c"System::write_file", "fsync", "");
        }
        if lock && !flock(&file, libc::LOCK_UN) {
            self.warn(c"System::write_file", "flock", ", LOCK_UN");
        }
        isize::try_from(written).unwrap_or(isize::MAX)
    }
}

/// `System::read_file`, in a coroutine: the content, or none when the file can't be
/// opened or locked, or the pool refuses the job.
#[must_use]
pub fn read_file(file: &CStr, lock: bool) -> Option<Vec<u8>> {
    read_with(file, lock, sys_warning, Some(crate::offload::async_job))
}

/// `System::write_file`, in a coroutine: bytes written, or -1 when the file can't be
/// opened or locked, or the pool refuses the job. `O_CREAT | O_WRONLY` are added.
#[must_use]
pub fn write_file(file: &CStr, data: &[u8], lock: bool, flags: c_int) -> isize {
    let flags = flags | libc::O_CREAT | libc::O_WRONLY;
    write_with(
        file,
        data,
        lock,
        flags,
        sys_warning,
        Some(crate::offload::async_job),
    )
}

fn read_with(file: &CStr, lock: bool, warn: Warn, offload: Option<Offload>) -> Option<Vec<u8>> {
    let job = Job { file, warn };
    on_pool(offload, None, || job.read(lock))
}

fn write_with(
    file: &CStr,
    data: &[u8],
    lock: bool,
    flags: c_int,
    warn: Warn,
    offload: Option<Offload>,
) -> isize {
    let job = Job { file, warn };
    on_pool(offload, -1, || job.write(data, lock, flags))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::ffi::CString;

    thread_local! {
        static WARNINGS: RefCell<Vec<(String, c_int)>> = const { RefCell::new(Vec::new()) };
    }

    fn warn(error: c_int, origin: &CStr, message: &str) {
        let line = format!("{}(): {message}", origin.to_str().unwrap());
        WARNINGS.with(|warnings| warnings.borrow_mut().push((line, error)));
    }

    fn read(path: &CString, lock: bool) -> Option<Vec<u8>> {
        read_with(path, lock, warn, None)
    }

    fn write(path: &CString, data: &[u8], flags: c_int) -> isize {
        write_with(
            path,
            data,
            true,
            flags | libc::O_CREAT | libc::O_WRONLY,
            warn,
            None,
        )
    }

    fn warnings() -> Vec<(String, c_int)> {
        WARNINGS.with(RefCell::take)
    }

    #[test]
    fn writes_appends_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("swr-file-io-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = CString::new(dir.join("file").to_str().unwrap()).unwrap();
        assert_eq!(write(&path, b"hello", libc::O_TRUNC), 5);
        assert_eq!(write(&path, b" world", libc::O_APPEND), 6);
        assert_eq!(read(&path, true).as_deref(), Some(&b"hello world"[..]));
        // An empty file reads through the growing buffer, as `File::read_content`.
        assert_eq!(write(&path, b"", libc::O_TRUNC), 0);
        assert_eq!(read(&path, false).as_deref(), Some(&b""[..]));
        assert_eq!(warnings(), []);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "Miri's pipes have no /dev/fd entry: the path opens a host descriptor"
    )]
    fn reads_a_stream_past_its_first_buffer() {
        let mut fds = [0; 2];
        // SAFETY: a writable pair.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let data = vec![7u8; READ_CONTENT_SIZE * 3 + 5];
        let writer = {
            let data = data.clone();
            let fd = fds[1];
            std::thread::spawn(move || {
                // SAFETY: the pipe's write end and `data`.
                let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
                // SAFETY: the pipe's write end.
                unsafe { libc::close(fd) };
                n
            })
        };
        let path = CString::new(format!("/dev/fd/{}", fds[0])).unwrap();
        let content = read(&path, false).unwrap();
        assert_eq!(writer.join().unwrap(), isize::try_from(data.len()).unwrap());
        assert_eq!(content, data);
        // SAFETY: the pipe's read end.
        unsafe { libc::close(fds[0]) };
    }

    #[test]
    fn warns_when_it_cannot_open() {
        let path = CString::new("/nonexistent-swr/file").unwrap();
        assert_eq!(read(&path, true), None);
        assert_eq!(write(&path, b"x", 0), -1);
        assert_eq!(
            warnings(),
            [
                (
                    "System::read_file(): open(/nonexistent-swr/file, O_RDONLY) failed".into(),
                    libc::ENOENT
                ),
                (
                    format!(
                        "System::write_file(): open(/nonexistent-swr/file, {}) failed",
                        libc::O_CREAT | libc::O_WRONLY
                    ),
                    libc::ENOENT
                ),
            ]
        );
    }
}
