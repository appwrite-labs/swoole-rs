//! `swoole::File` IO loops, `file_get_size` and the sendfile variants (upstream `src/os/{file,sendfile,base}.cc`). Rust runs the
//! syscalls; native code keeps the descriptors and logs warnings, so every
//! function returns right after the failing call with its errno intact.
use libc::{c_char, c_int, c_void, off_t};

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Bytes moved by a `File::read_all`/`write_all` loop, and whether it stopped on
/// an error native must warn about (errno is still the failing call's).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIo {
    pub done: usize,
    pub failed: bool,
}

/// Upstream `catch_fs_error`: zero stops, transient errors retry, others warn.
fn transfer(length: usize, mut io: impl FnMut(usize) -> isize) -> FileIo {
    let mut done = 0;
    while done < length {
        let n = io(done);
        if n > 0 {
            done += n.unsigned_abs();
        } else if n == 0 {
            break;
        } else if !matches!(errno(), libc::EINTR | libc::EAGAIN) {
            return FileIo { done, failed: true };
        }
    }
    FileIo {
        done,
        failed: false,
    }
}

#[expect(
    clippy::cast_possible_wrap,
    reason = "a file offset below the requested length fits off_t, as in upstream's implicit conversion"
)]
const fn to_off(done: usize) -> off_t {
    done as off_t
}

/// `File::open`: closes the open descriptor first; created files default to 0644.
/// # Safety
/// `path` is a valid NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_file_open(
    fd: c_int,
    path: *const c_char,
    flags: c_int,
    mode: c_int,
) -> c_int {
    if fd != -1 {
        // SAFETY: the caller's previous descriptor, released here as upstream does.
        unsafe { libc::close(fd) };
    }
    if flags & libc::O_CREAT == 0 {
        // SAFETY: valid C string per the contract.
        return unsafe { libc::open(path, flags) };
    }
    let mode = if mode == 0 { 0o644 } else { mode };
    // SAFETY: valid C string per the contract; the mode is promoted as C does.
    unsafe { libc::open(path, flags, mode.unsigned_abs()) }
}

/// `File::close`: false for an already-closed descriptor.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_file_close(fd: c_int) -> bool {
    // SAFETY: the caller hands over its descriptor.
    fd != -1 && unsafe { libc::close(fd) } == 0
}

/// `swoole_tmpfile`: `mkostemp(O_WRONLY | O_CREAT)` on Linux, `mkstemp` elsewhere.
/// # Safety
/// `filename` is a writable NUL-terminated `XXXXXX` template.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_tmpfile(filename: *mut c_char) -> c_int {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: a writable template per the contract.
    unsafe {
        libc::mkostemp(filename, libc::O_WRONLY | libc::O_CREAT)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    // SAFETY: a writable template per the contract.
    unsafe {
        libc::mkstemp(filename)
    }
}

/// `File::write_all`: `write` in append mode, otherwise `pwrite` at the progress.
/// # Safety
/// `data` points to `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_file_write_all(
    fd: c_int,
    append: bool,
    data: *const c_void,
    length: usize,
) -> FileIo {
    transfer(length, |done| {
        // SAFETY: `done < length`, so the range stays inside the caller's buffer.
        let at = unsafe { data.byte_add(done) };
        if append {
            // SAFETY: `at` has `length - done` readable bytes.
            unsafe { libc::write(fd, at, length - done) }
        } else {
            // SAFETY: `at` has `length - done` readable bytes.
            unsafe { libc::pwrite(fd, at, length - done, to_off(done)) }
        }
    })
}

/// `File::read_all`: `pread` at the progress until `length` bytes, EOF or error.
/// # Safety
/// `buf` points to `length` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_file_read_all(
    fd: c_int,
    buf: *mut c_void,
    length: usize,
) -> FileIo {
    transfer(length, |done| {
        // SAFETY: `done < length`, so the range stays inside the caller's buffer.
        let at = unsafe { buf.byte_add(done) };
        // SAFETY: `at` has `length - done` writable bytes.
        unsafe { libc::pread(fd, at, length - done, to_off(done)) }
    })
}

/// `File::read_line`: reads up to `n - 1` bytes, cuts after the first NUL or
/// newline, NUL-terminates and seeks just past the line. Without a terminator it
/// seeks one byte past the data read, as upstream does.
/// # Safety
/// `buf` points to `n` writable bytes and `n` is at least one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_file_read_line(fd: c_int, buf: *mut c_void, n: usize) -> isize {
    // SAFETY: plain syscall on a caller-owned descriptor.
    let start = unsafe { libc::lseek(fd, 0, libc::SEEK_CUR) };
    // SAFETY: `buf` has `n` writable bytes, so `n - 1` leaves room for the NUL.
    let read = unsafe { libc::read(fd, buf, n.wrapping_sub(1)) };
    if read <= 0 {
        return read;
    }
    let read = read.unsigned_abs();
    // SAFETY: the read initialised `read` bytes at `buf`.
    let bytes = unsafe { std::slice::from_raw_parts_mut(buf.cast::<u8>(), n) };
    let (length, skip) = bytes
        .iter()
        .take(read)
        .position(|&b| b == 0 || b == b'\n')
        .map_or((read, 1), |at| (at + 1, 0));
    if let Some(terminator) = bytes.get_mut(length) {
        *terminator = 0;
    }
    // SAFETY: plain syscall on a caller-owned descriptor.
    unsafe { libc::lseek(fd, start + to_off(length + skip), libc::SEEK_SET) };
    length.cast_signed()
}

/// `file_get_size(fd)`: the size of a regular file, or the error to publish.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSize {
    pub size: i64,
    pub error: c_int,
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_file_size(fd: c_int) -> FileSize {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `stat` is writable storage for one `struct stat`.
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } < 0 {
        return FileSize {
            size: -1,
            error: errno(),
        };
    }
    // SAFETY: fstat succeeded, so it initialised `stat`.
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
        return FileSize {
            size: -1,
            error: libc::EISDIR,
        };
    }
    FileSize {
        size: stat.st_size,
        error: 0,
    }
}

/// A sendfile result and the warning native logs: 0 none; for the copy fallback
/// 1 `pread() failed` and 2 `write() failed`; for kqueue `sendfile` 1 `sendfile failed`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sendfile {
    pub result: isize,
    pub warn: u32,
}

/// Linux's direct sendfile syscall, preserving a nullable offset and its errno.
/// # Safety
/// offset is null or an exclusively writable `off_t`; descriptors follow sendfile(2).
#[cfg(any(target_os = "linux", target_os = "android"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_sendfile(
    out_fd: c_int,
    in_fd: c_int,
    offset: *mut off_t,
    size: usize,
) -> Sendfile {
    // SAFETY: the caller forwards the syscall's descriptor/offset contract.
    let result = unsafe { libc::sendfile(out_fd, in_fd, offset, size) };
    Sendfile { result, warn: 0 }
}

/// The `!HAVE_SENDFILE` fallback: one `pread` of at most `SW_BUFFER_SIZE_BIG`
/// bytes at `*offset`, written once; the offset advances by what was written.
/// # Safety
/// `offset` points to a valid, exclusively borrowed `off_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_sendfile_copy(
    out_fd: c_int,
    in_fd: c_int,
    offset: *mut off_t,
    size: usize,
) -> Sendfile {
    #[expect(
        clippy::large_stack_arrays,
        reason = "upstream's SW_BUFFER_SIZE_BIG stack buffer"
    )]
    let mut buf = [0_u8; 65536];
    let amount = size.min(buf.len());
    // SAFETY: `offset` is valid per the contract.
    let at = unsafe { *offset };
    // SAFETY: `buf` has `amount` writable bytes.
    let n = unsafe { libc::pread(in_fd, buf.as_mut_ptr().cast(), amount, at) };
    if n <= 0 {
        return Sendfile {
            result: -1,
            warn: 1,
        };
    }
    // SAFETY: `buf` holds `n` bytes read above.
    let written = unsafe { libc::write(out_fd, buf.as_ptr().cast(), n.unsigned_abs()) };
    if written < 0 {
        return Sendfile {
            result: written,
            warn: 2,
        };
    }
    // SAFETY: `offset` is valid per the contract.
    unsafe { *offset += isize_to_off(written) };
    Sendfile {
        result: written,
        warn: 0,
    }
}

const fn isize_to_off(value: isize) -> off_t {
    value as off_t
}

/// One Darwin `sendfile`: `length` is in/out, so a retry sends what the
/// interrupted call reported (upstream reuses its `size`). Returns the result and
/// the progress to add to the offset.
#[cfg(target_vendor = "apple")]
fn sendfile_once(out_fd: c_int, in_fd: c_int, at: off_t, length: &mut off_t) -> (c_int, off_t) {
    // SAFETY: `length` is a live in/out length; no header/trailer vectors.
    let ret = unsafe { libc::sendfile(in_fd, out_fd, at, length, std::ptr::null_mut(), 0) };
    (ret, *length)
}

/// One FreeBSD `sendfile` of `length` bytes; progress comes back in `sbytes`.
#[cfg(target_os = "freebsd")]
fn sendfile_once(out_fd: c_int, in_fd: c_int, at: off_t, length: &mut off_t) -> (c_int, off_t) {
    let mut sent: off_t = 0;
    // SAFETY: `sent` is a live out-parameter; no header/trailer vectors.
    let ret = unsafe {
        libc::sendfile(
            in_fd,
            out_fd,
            at,
            length.unsigned_abs().try_into().unwrap_or(usize::MAX),
            std::ptr::null_mut(),
            &raw mut sent,
            0,
        )
    };
    (ret, sent)
}

/// The kqueue platforms' `sendfile`: retried on `EINTR`, the offset advanced by
/// every call's progress; success returns the (Darwin: reported) length.
/// # Safety
/// `offset` points to a valid, exclusively borrowed `off_t`.
#[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_sendfile(
    out_fd: c_int,
    in_fd: c_int,
    offset: *mut off_t,
    size: usize,
) -> Sendfile {
    let mut length = to_off(size);
    loop {
        // SAFETY: `offset` is valid per the contract.
        let at = unsafe { *offset };
        let (ret, sent) = sendfile_once(out_fd, in_fd, at, &mut length);
        // SAFETY: `offset` is valid per the contract.
        unsafe { *offset += sent };
        match ret {
            -1 if errno() == libc::EINTR => {}
            -1 => {
                return Sendfile {
                    result: -1,
                    warn: 0,
                };
            }
            0 => {
                return Sendfile {
                    result: isize::try_from(length).unwrap_or(-1),
                    warn: 0,
                };
            }
            _ => {
                return Sendfile {
                    result: -1,
                    warn: 1,
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};
    use std::os::fd::AsRawFd;

    fn scratch(content: &[u8]) -> std::fs::File {
        let path = std::env::temp_dir().join(format!(
            "swr-ossupport-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap_or_else(|e| unreachable!("{e}"));
        std::fs::remove_file(&path).unwrap_or_else(|e| unreachable!("{e}"));
        file.write_all(content)
            .unwrap_or_else(|e| unreachable!("{e}"));
        file.seek(SeekFrom::Start(0))
            .unwrap_or_else(|e| unreachable!("{e}"));
        file
    }

    #[test]
    fn read_and_write_all_use_positional_io_and_stop_at_eof() {
        let file = scratch(b"hello world");
        let fd = file.as_raw_fd();
        let mut buf = [0_u8; 32];
        let io = unsafe { swr_v1_os_file_read_all(fd, buf.as_mut_ptr().cast(), buf.len()) };
        assert_eq!(
            io,
            FileIo {
                done: 11,
                failed: false
            },
            "EOF ends the loop quietly"
        );
        assert_eq!(&buf[..11], b"hello world", "content read from offset zero");
        let io = unsafe { swr_v1_os_file_write_all(fd, false, b"HELLO".as_ptr().cast(), 5) };
        assert_eq!(
            io,
            FileIo {
                done: 5,
                failed: false
            },
            "pwrite writes it all"
        );
        let io = unsafe { swr_v1_os_file_read_all(fd, buf.as_mut_ptr().cast(), 11) };
        assert_eq!(
            (io.done, &buf[..11]),
            (11, &b"HELLO world"[..]),
            "pwrite starts at zero"
        );
        assert_eq!(
            swr_v1_os_file_size(fd),
            FileSize { size: 11, error: 0 },
            "regular size"
        );
        let io = unsafe { swr_v1_os_file_write_all(-1, true, b"x".as_ptr().cast(), 1) };
        assert_eq!(
            io,
            FileIo {
                done: 0,
                failed: true
            },
            "EBADF warns"
        );
        assert_eq!(
            errno(),
            libc::EBADF,
            "errno survives for the native warning"
        );
    }

    #[test]
    fn size_rejects_failures_and_non_regular_files() {
        assert_eq!(
            swr_v1_os_file_size(-1),
            FileSize {
                size: -1,
                error: libc::EBADF
            },
            "fstat errno"
        );
        let dir = std::fs::File::open(std::env::temp_dir()).unwrap_or_else(|e| unreachable!("{e}"));
        assert_eq!(
            swr_v1_os_file_size(dir.as_raw_fd()),
            FileSize {
                size: -1,
                error: libc::EISDIR
            },
            "non-regular files are EISDIR"
        );
    }

    #[test]
    fn read_line_cuts_after_the_terminator_and_seeks_past_it() {
        let mut file = scratch(b"ab\ncd\0ef");
        let fd = file.as_raw_fd();
        let mut buf = [0xff_u8; 16];
        assert_eq!(
            unsafe { swr_v1_os_file_read_line(fd, buf.as_mut_ptr().cast(), 16) },
            3,
            "newline kept"
        );
        assert_eq!(&buf[..4], b"ab\n\0", "NUL after the line");
        assert_eq!(
            unsafe { swr_v1_os_file_read_line(fd, buf.as_mut_ptr().cast(), 16) },
            3,
            "NUL ends a line"
        );
        assert_eq!(&buf[..4], b"cd\0\0", "NUL kept and terminated");
        assert_eq!(
            unsafe { swr_v1_os_file_read_line(fd, buf.as_mut_ptr().cast(), 16) },
            2,
            "no terminator"
        );
        assert_eq!(&buf[..3], b"ef\0", "terminated");
        assert_eq!(
            file.stream_position().unwrap_or_default(),
            9,
            "one byte past the data"
        );
        assert_eq!(
            unsafe { swr_v1_os_file_read_line(fd, buf.as_mut_ptr().cast(), 16) },
            0,
            "EOF"
        );
        file.seek(SeekFrom::Start(0))
            .unwrap_or_else(|e| unreachable!("{e}"));
        assert_eq!(
            unsafe { swr_v1_os_file_read_line(fd, buf.as_mut_ptr().cast(), 3) },
            2,
            "n - 1 bytes"
        );
        assert_eq!(&buf[..3], b"ab\0", "a full chunk is terminated");
        assert_eq!(
            file.stream_position().unwrap_or_default(),
            3,
            "and skips one byte"
        );
    }

    #[test]
    fn sendfile_copy_moves_one_chunk_and_advances_the_offset() {
        let source = scratch(b"0123456789");
        let sink = scratch(b"");
        let mut offset: off_t = 2;
        let sent = unsafe {
            swr_v1_os_sendfile_copy(sink.as_raw_fd(), source.as_raw_fd(), &raw mut offset, 5)
        };
        assert_eq!(
            (sent, offset),
            (Sendfile { result: 5, warn: 0 }, 7),
            "five bytes from offset two"
        );
        let sent = unsafe {
            swr_v1_os_sendfile_copy(sink.as_raw_fd(), source.as_raw_fd(), &raw mut offset, 0)
        };
        assert_eq!(
            sent,
            Sendfile {
                result: -1,
                warn: 1
            },
            "an empty read warns"
        );
        let mut buf = [0_u8; 8];
        let io = unsafe { swr_v1_os_file_read_all(sink.as_raw_fd(), buf.as_mut_ptr().cast(), 8) };
        assert_eq!(&buf[..io.done], b"23456", "written to the sink");
    }

    #[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
    #[test]
    fn kqueue_sendfile_sends_the_range_and_advances_the_offset() {
        use std::io::Read;
        let source = scratch(b"0123456789");
        let (sink, mut peer) =
            std::os::unix::net::UnixStream::pair().unwrap_or_else(|e| unreachable!("{e}"));
        let mut offset: off_t = 3;
        let sent =
            unsafe { swr_v1_os_sendfile(sink.as_raw_fd(), source.as_raw_fd(), &raw mut offset, 4) };
        assert_eq!(
            (sent, offset),
            (Sendfile { result: 4, warn: 0 }, 7),
            "four bytes sent"
        );
        let mut buf = [0_u8; 4];
        peer.read_exact(&mut buf)
            .unwrap_or_else(|e| unreachable!("{e}"));
        assert_eq!(&buf, b"3456", "the requested range");
    }

    #[test]
    fn open_close_and_tmpfile_follow_upstream() {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut template = *b"/tmp/swr-ossupport-XXXXXX\0";
        let fd = unsafe { swr_v1_os_tmpfile(template.as_mut_ptr().cast()) };
        assert!(fd >= 0, "a temporary file");
        let path = std::ffi::CStr::from_bytes_until_nul(&template).unwrap_or_default();
        let reopened = unsafe { swr_v1_os_file_open(fd, path.as_ptr(), libc::O_RDONLY, 0) };
        assert!(reopened >= 0, "reopened after closing the old descriptor");
        assert!(swr_v1_os_file_close(reopened), "closed");
        assert!(!swr_v1_os_file_close(-1), "nothing to close");
        let created =
            std::ffi::CString::new(format!("{}.new", path.to_string_lossy())).unwrap_or_default();
        let fd =
            unsafe { swr_v1_os_file_open(-1, created.as_ptr(), libc::O_WRONLY | libc::O_CREAT, 0) };
        let mode =
            std::fs::metadata(created.to_string_lossy().as_ref()).map(|m| m.permissions().mode());
        let reference = format!("{}.ref", path.to_string_lossy());
        let expected = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o644)
            .open(&reference)
            .and_then(|file| file.metadata())
            .map(|m| m.permissions().mode());
        std::fs::remove_file(&reference).unwrap_or_default();
        assert_eq!(
            mode.unwrap_or_default() & 0o777,
            expected.unwrap_or_default() & 0o777,
            "created with 0644 under the umask"
        );
        assert!(swr_v1_os_file_close(fd), "closed");
        std::fs::remove_file(created.to_string_lossy().as_ref()).unwrap_or_default();
        std::fs::remove_file(path.to_string_lossy().as_ref()).unwrap_or_default();
    }
}
