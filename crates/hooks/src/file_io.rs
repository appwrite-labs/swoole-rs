//! `System::read_file` and `System::write_file`: open, lock, read or write the whole
//! file, sync and unlock, all on the async pool while the coroutine waits, as upstream's
//! lambda over a `File` did. Native code owns the `String` the content lands in and
//! the log; each warning reaches it on the pool thread with the failing call's errno.
use crate::stdio::{Offload, offloaded};
use libc::{c_char, c_int, c_void, size_t};
use std::ffi::CStr;

/// Native code's side of a whole-file operation.
#[repr(C)]
pub struct FileOps {
    /// `swoole_sys_warning("%s", message)` with errno `error`, logged as from `origin`.
    pub warn: unsafe extern "C" fn(error: c_int, origin: *const c_char, message: *const c_char),
    /// Give `out`'s `String` (created on first use) `length` bytes of content and at
    /// least `size` bytes of room; returns its buffer.
    pub buffer: unsafe extern "C" fn(out: *mut c_void, length: size_t, size: size_t) -> *mut c_char,
}

/// `SW_AIO_WRITE_FSYNC`, tested against the open flags as upstream does.
const WRITE_FSYNC: c_int = 1 << 1;
/// `SW_BUFFER_SIZE_STD`: `File::read_content`'s first buffer.
const READ_CONTENT_SIZE: usize = 8192;

fn offset(done: usize) -> libc::off_t {
    libc::off_t::try_from(done).unwrap_or(libc::off_t::MAX)
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// An open fd, closed on drop like `File`.
struct Fd(c_int);

impl Drop for Fd {
    fn drop(&mut self) {
        // SAFETY: the fd this operation opened.
        unsafe { libc::close(self.0) };
    }
}

impl Fd {
    fn open(file: *const c_char, flags: c_int) -> Option<Self> {
        let fd = if flags & libc::O_CREAT != 0 {
            let mode: libc::c_uint = 0o644;
            // SAFETY: the caller passes a C string.
            unsafe { libc::open(file, flags, mode) }
        } else {
            // SAFETY: as above.
            unsafe { libc::open(file, flags) }
        };
        if fd == -1 { None } else { Some(Self(fd)) }
    }

    fn flock(&self, operation: c_int) -> bool {
        // SAFETY: flock has no memory preconditions.
        unsafe { libc::flock(self.0, operation) == 0 }
    }

    /// `file_get_size`: the size of a regular file, else -1.
    fn size(&self) -> i64 {
        let mut status = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: a writable stat buffer.
        if unsafe { libc::fstat(self.0, status.as_mut_ptr()) } < 0 {
            return -1;
        }
        // SAFETY: fstat filled it.
        let status = unsafe { status.assume_init() };
        if status.st_mode & libc::S_IFMT != libc::S_IFREG {
            return -1;
        }
        status.st_size
    }
}

/// `File::read_all` and `File::write_all`'s loop: retry on EINTR/EAGAIN, stop at 0 or
/// an error, which `fail` reports. Returns the bytes done.
fn transfer_all(
    len: usize,
    mut step: impl FnMut(usize) -> isize,
    fail: impl FnOnce(c_int, usize),
) -> usize {
    let mut done = 0;
    while done < len {
        let n = step(done);
        if n > 0 {
            done += n.unsigned_abs();
            continue;
        }
        let error = errno();
        if n == 0 {
            break;
        }
        if error == libc::EINTR || error == libc::EAGAIN || error == libc::EWOULDBLOCK {
            continue;
        }
        fail(error, done);
        break;
    }
    done
}

/// What a whole-file operation needs from its caller, lent to the pool thread.
struct Job<'a> {
    file: *const c_char,
    ops: &'a FileOps,
    out: *mut c_void,
}

impl Job<'_> {
    /// Log `message` with errno `error` as from `origin`.
    fn log(&self, origin: &CStr, error: c_int, message: impl Into<Vec<u8>>) {
        let mut message = message.into();
        message.push(0);
        // SAFETY: native code's logger with two C strings.
        unsafe { (self.ops.warn)(error, origin.as_ptr(), message.as_ptr().cast()) };
    }

    /// `"{call}({file}{args}) failed"` from `origin`, with the current errno.
    fn warn(&self, origin: &CStr, call: &str, args: &str) {
        // SAFETY: the caller passes a C string.
        let file = unsafe { CStr::from_ptr(self.file) }.to_bytes();
        let message = [call.as_bytes(), b"(", file, args.as_bytes(), b") failed"].concat();
        self.log(origin, errno(), message);
    }

    fn buffer(&self, length: usize, size: usize) -> *mut c_char {
        // SAFETY: the caller's `String` slot.
        unsafe { (self.ops.buffer)(self.out, length, size) }
    }

    fn read(&self, lock: bool) {
        let Some(fd) = Fd::open(self.file, libc::O_RDONLY) else {
            return self.warn(c"System::read_file", "open", ", O_RDONLY");
        };
        if lock && !fd.flock(libc::LOCK_SH) {
            return self.warn(c"System::read_file", "flock", ", LOCK_SH");
        }
        match usize::try_from(fd.size()) {
            Ok(size) if size > 0 => self.read_all(&fd, size),
            _ => self.read_content(&fd),
        }
        if lock && !fd.flock(libc::LOCK_UN) {
            self.warn(c"System::read_file", "flock", ", LOCK_UN");
        }
    }

    fn read_all(&self, fd: &Fd, size: usize) {
        let buf = self.buffer(0, size + 1);
        let read = transfer_all(
            size,
            // SAFETY: `buf` has room for `size` bytes.
            |done| unsafe {
                libc::pread(
                    fd.0,
                    buf.wrapping_add(done).cast(),
                    size - done,
                    offset(done),
                )
            },
            |error, done| {
                let message = format!("pread({}, {buf:p}, {}, {done}) failed", fd.0, size - done);
                self.log(c"File::read_all", error, message);
            },
        );
        self.terminate(read, size + 1);
    }

    fn read_content(&self, fd: &Fd) {
        let mut size = READ_CONTENT_SIZE;
        let mut length = 0;
        let mut buf = self.buffer(0, size);
        loop {
            // SAFETY: `buf` has room for `size` bytes, `length` of them used.
            let n = unsafe { libc::read(fd.0, buf.wrapping_add(length).cast(), size - length) };
            if n <= 0 {
                break;
            }
            length += n.unsigned_abs();
            if length == size {
                size *= 2;
            }
            buf = self.buffer(length, size);
        }
        self.terminate(length, size);
    }

    /// `content->length = length; content->str[length] = 0`.
    fn terminate(&self, length: usize, size: usize) {
        let buf = self.buffer(length, size);
        // SAFETY: `length < size`, inside the buffer.
        unsafe { buf.wrapping_add(length).write(0) };
    }

    fn write(&self, data: *const c_void, length: usize, lock: bool, flags: c_int) -> isize {
        let Some(fd) = Fd::open(self.file, flags) else {
            self.warn(c"System::write_file", "open", &format!(", {flags}"));
            return -1;
        };
        if lock && !fd.flock(libc::LOCK_EX) {
            self.warn(c"System::write_file", "flock", ", LOCK_EX");
            return -1;
        }
        let append = flags & libc::O_APPEND != 0;
        let written = transfer_all(
            length,
            |done| {
                // SAFETY: `data` holds `length` bytes.
                let from = unsafe { data.cast::<u8>().add(done) }.cast();
                if append {
                    // SAFETY: as above.
                    unsafe { libc::write(fd.0, from, length - done) }
                } else {
                    // SAFETY: as above.
                    unsafe { libc::pwrite(fd.0, from, length - done, offset(done)) }
                }
            },
            |error, done| {
                let message = format!(
                    "pwrite({}, {data:p}, {}, {done}) failed",
                    fd.0,
                    length - done
                );
                self.log(c"File::write_all", error, message);
            },
        );
        // SAFETY: fsync has no memory preconditions.
        if flags & WRITE_FSYNC != 0 && unsafe { libc::fsync(fd.0) } != 0 {
            self.warn(c"System::write_file", "fsync", "");
        }
        if lock && !fd.flock(libc::LOCK_UN) {
            self.warn(c"System::write_file", "flock", ", LOCK_UN");
        }
        isize::try_from(written).unwrap_or(isize::MAX)
    }
}

/// `System::read_file`: the content lands in `out`'s `String`, which stays unset when
/// the file can't be opened or locked, or the pool refuses the job.
///
/// # Safety
/// `file` is a C string; `ops` and `out` are valid for the call; `offload` is native
/// `async`, valid in the current coroutine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_read_file(
    file: *const c_char,
    lock: bool,
    out: *mut c_void,
    ops: *const FileOps,
    offload: Option<Offload>,
) {
    let job = Job {
        file,
        // SAFETY: the caller's ops, valid for the call.
        ops: unsafe { &*ops },
        out,
    };
    // SAFETY: the job only touches what the caller lent until `offload` returns.
    unsafe { offloaded(offload, (), || job.read(lock)) };
}

/// `System::write_file`: bytes written, or -1 when the file can't be opened or
/// locked, or the pool refuses the job. `flags` already has `O_CREAT | O_WRONLY`.
///
/// # Safety
/// As [`swr_v1_hook_read_file`]; `data` holds `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_write_file(
    file: *const c_char,
    data: *const c_void,
    length: size_t,
    lock: bool,
    flags: c_int,
    ops: *const FileOps,
    offload: Option<Offload>,
) -> isize {
    let job = Job {
        file,
        // SAFETY: the caller's ops, valid for the call.
        ops: unsafe { &*ops },
        out: std::ptr::null_mut(),
    };
    // SAFETY: as in `swr_v1_hook_read_file`.
    unsafe { offloaded(offload, -1, || job.write(data, length, lock, flags)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::ffi::CString;

    thread_local! {
        static WARNINGS: RefCell<Vec<(String, c_int)>> = const { RefCell::new(Vec::new()) };
    }

    unsafe extern "C" fn warn(error: c_int, origin: *const c_char, message: *const c_char) {
        // SAFETY: two C strings from `Job::log`.
        let (origin, message) = unsafe { (CStr::from_ptr(origin), CStr::from_ptr(message)) };
        let line = format!(
            "{}(): {}",
            origin.to_str().unwrap(),
            message.to_str().unwrap()
        );
        WARNINGS.with(|warnings| warnings.borrow_mut().push((line, error)));
    }

    /// A `String` stand-in: `(content, set)`.
    unsafe extern "C" fn buffer(out: *mut c_void, length: size_t, size: size_t) -> *mut c_char {
        // SAFETY: the test's own slot.
        let out = unsafe { &mut *out.cast::<Option<(Vec<u8>, usize)>>() };
        let (bytes, used) = out.get_or_insert_with(|| (Vec::new(), 0));
        if bytes.len() < size {
            bytes.resize(size, 0);
        }
        *used = length;
        bytes.as_mut_ptr().cast()
    }

    const OPS: FileOps = FileOps { warn, buffer };

    fn read(path: &CString, lock: bool) -> Option<Vec<u8>> {
        let mut out: Option<(Vec<u8>, usize)> = None;
        // SAFETY: a C string and the test's ops and slot; no pool.
        unsafe { swr_v1_hook_read_file(path.as_ptr(), lock, (&raw mut out).cast(), &OPS, None) };
        out.map(|(mut bytes, used)| {
            assert_eq!(bytes[used], 0);
            bytes.truncate(used);
            bytes
        })
    }

    fn write(path: &CString, data: &[u8], flags: c_int) -> isize {
        // SAFETY: a C string, `data` and the test's ops; no pool.
        unsafe {
            swr_v1_hook_write_file(
                path.as_ptr(),
                data.as_ptr().cast(),
                data.len(),
                true,
                flags | libc::O_CREAT | libc::O_WRONLY,
                &OPS,
                None,
            )
        }
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
