//! `swoole::File` IO loops, `file_get_size` and the sendfile variants (upstream `src/os/{file,sendfile,base}.cc`). Rust runs the
//! syscalls; native code keeps the descriptors and logs warnings, so every
//! function returns right after the failing call with its errno intact.
use libc::{c_char, c_int, off_t};
use std::ffi::CStr;
use std::marker::PhantomData;
use std::os::fd::OwnedFd;
use std::ptr::NonNull;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::sys::{self, ReadSlot, errno};

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

fn to_off(done: usize) -> off_t {
    done.c_cast::<off_t>()
}

/// `File::open`: closes the open descriptor first; created files default to 0644.
#[must_use]
pub fn file_open(fd: c_int, path: &CStr, flags: c_int, mode: c_int) -> c_int {
    if fd != -1 {
        // The caller's previous descriptor, released here as upstream does.
        sys::close(fd);
    }
    let mode = if flags & libc::O_CREAT == 0 || mode != 0 {
        mode
    } else {
        0o644
    };
    sys::open(path, flags, libc::mode_t::try_from(mode).unwrap_or(0o644))
}

/// `File::close`: false for an already-closed descriptor.
#[expect(
    clippy::must_use_candidate,
    reason = "upstream's callers ignore File::close's result"
)]
pub fn file_close(fd: c_int) -> bool {
    fd != -1 && sys::close(fd) == 0
}

/// A writable file name template, NUL-terminated within the bytes it borrows: what
/// `mkstemp` reads up to the NUL and rewrites (its `XXXXXX`) in place.
#[repr(transparent)]
struct Template<'a>(NonNull<c_char>, PhantomData<&'a mut [u8]>);

impl<'a> Template<'a> {
    /// `None` when `bytes` holds no NUL.
    fn new(bytes: &'a mut [u8]) -> Option<Self> {
        bytes
            .contains(&0)
            .then(|| Self(NonNull::from(bytes).cast(), PhantomData))
    }
}

// libc's, over a template they write only before its NUL; a template without the
// `XXXXXX` suffix is an error they return. The descriptor they return is a new one
// the caller owns, or -1 (`None`: `OwnedFd` never holds -1).
unsafe extern "C" {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    safe fn mkostemp(template: Template<'_>, flags: c_int) -> Option<OwnedFd>;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    safe fn mkstemp(template: Template<'_>) -> Option<OwnedFd>;
}

/// `swoole_tmpfile`: `mkostemp(O_WRONLY | O_CREAT)` on Linux, `mkstemp` elsewhere, over
/// the `XXXXXX` template up to the first NUL in `template`: the new descriptor, or
/// none (errno `EINVAL` when there is no NUL).
pub(crate) fn tmpfile(template: &mut [u8]) -> Option<OwnedFd> {
    let Some(template) = Template::new(template) else {
        sys::set_errno(libc::EINVAL);
        return None;
    };
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let fd = mkostemp(template, libc::O_WRONLY | libc::O_CREAT);
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let fd = mkstemp(template);
    fd
}

/// `File::write_all`: `write` in append mode, otherwise `pwrite` at the progress.
#[must_use]
pub fn write_all(fd: c_int, append: bool, data: &[u8]) -> FileIo {
    transfer(data.len(), |done| {
        if append {
            sys::write(fd, data.get(done..).unwrap_or_default())
        } else {
            sys::pwrite(fd, data.get(done..).unwrap_or_default(), to_off(done))
        }
    })
}

/// `File::read_all`: `pread` at the progress until the buffer is full, EOF or error.
#[must_use]
pub fn read_all(fd: c_int, buf: &mut [impl ReadSlot]) -> FileIo {
    transfer(buf.len(), |done| {
        sys::pread(fd, buf.get_mut(done..).unwrap_or_default(), to_off(done))
    })
}

/// `file_get_size(fd)`: the size of a regular file, or the error to publish.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSize {
    pub size: i64,
    pub error: c_int,
}

#[must_use]
pub fn file_size(fd: c_int) -> FileSize {
    let Some(stat) = swoole_rs_corebase::sys::fstat(fd) else {
        return FileSize {
            size: -1,
            error: errno(),
        };
    };
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

#[cfg(any(target_os = "linux", target_os = "android"))]
unsafe extern "C" {
    /// Linux's `sendfile(2)`: reads its descriptors and count, and reads and advances
    /// the one `off_t` it is lent, or the file's own position without one.
    safe fn sendfile(
        out_fd: c_int,
        in_fd: c_int,
        offset: Option<&mut off_t>,
        count: usize,
    ) -> isize;
}

/// `swoole_sendfile` (upstream `src/os/sendfile.cc`) on Linux: the direct syscall,
/// from `offset` (advanced) or the file's own position, with its errno.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[must_use]
pub fn swr_v1_os_sendfile(
    out_fd: c_int,
    in_fd: c_int,
    offset: Option<&mut off_t>,
    size: usize,
) -> Sendfile {
    let result = sendfile(out_fd, in_fd, offset, size);
    Sendfile { result, warn: 0 }
}

#[cfg(target_vendor = "apple")]
unsafe extern "C" {
    /// Darwin's `sendfile(2)`: reads its descriptors and offset, reads and writes
    /// the one `off_t` length it is lent, and reads the header/trailer vectors only
    /// when given some.
    safe fn sendfile(
        fd: c_int,
        s: c_int,
        offset: off_t,
        len: &mut off_t,
        hdtr: Option<&mut libc::sf_hdtr>,
        flags: c_int,
    ) -> c_int;
}

#[cfg(target_os = "freebsd")]
unsafe extern "C" {
    /// FreeBSD's `sendfile(2)`: reads its descriptors, offset and count, writes the
    /// one `off_t` progress it is lent, and reads the header/trailer vectors only
    /// when given some.
    safe fn sendfile(
        fd: c_int,
        s: c_int,
        offset: off_t,
        nbytes: usize,
        hdtr: Option<&mut libc::sf_hdtr>,
        sbytes: &mut off_t,
        flags: c_int,
    ) -> c_int;
}

/// One Darwin `sendfile`: `length` is in/out, so a retry sends what the
/// interrupted call reported (upstream reuses its `size`). Returns the result and
/// the progress to add to the offset.
#[cfg(target_vendor = "apple")]
fn sendfile_once(out_fd: c_int, in_fd: c_int, at: off_t, length: &mut off_t) -> (c_int, off_t) {
    let ret = sendfile(in_fd, out_fd, at, length, None, 0);
    (ret, *length)
}

/// One FreeBSD `sendfile` of `length` bytes; progress comes back in `sbytes`.
#[cfg(target_os = "freebsd")]
fn sendfile_once(out_fd: c_int, in_fd: c_int, at: off_t, length: &mut off_t) -> (c_int, off_t) {
    let mut sent: off_t = 0;
    let count = length.unsigned_abs().try_into().unwrap_or(usize::MAX);
    let ret = sendfile(in_fd, out_fd, at, count, None, &mut sent, 0);
    (ret, sent)
}

/// `swoole_sendfile` on the kqueue platforms: retried on `EINTR`, `offset` (from 0
/// without one) advanced by every call's progress; success returns the (Darwin:
/// reported) length.
#[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
#[must_use]
pub fn swr_v1_os_sendfile(
    out_fd: c_int,
    in_fd: c_int,
    offset: Option<&mut off_t>,
    size: usize,
) -> Sendfile {
    let mut start = 0;
    let offset = offset.unwrap_or(&mut start);
    let mut length = to_off(size);
    loop {
        let (ret, sent) = sendfile_once(out_fd, in_fd, *offset, &mut length);
        *offset += sent;
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
        let io = read_all(fd, &mut buf);
        assert_eq!(
            io,
            FileIo {
                done: 11,
                failed: false
            },
            "EOF ends the loop quietly"
        );
        assert_eq!(&buf[..11], b"hello world", "content read from offset zero");
        let io = write_all(fd, false, b"HELLO");
        assert_eq!(
            io,
            FileIo {
                done: 5,
                failed: false
            },
            "pwrite writes it all"
        );
        let io = read_all(fd, &mut buf[..11]);
        assert_eq!(
            (io.done, &buf[..11]),
            (11, &b"HELLO world"[..]),
            "pwrite starts at zero"
        );
        assert_eq!(
            file_size(fd),
            FileSize { size: 11, error: 0 },
            "regular size"
        );
        let io = write_all(-1, true, b"x");
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
            file_size(-1),
            FileSize {
                size: -1,
                error: libc::EBADF
            },
            "fstat errno"
        );
        let dir = std::fs::File::open(std::env::temp_dir()).unwrap_or_else(|e| unreachable!("{e}"));
        assert_eq!(
            file_size(dir.as_raw_fd()),
            FileSize {
                size: -1,
                error: libc::EISDIR
            },
            "non-regular files are EISDIR"
        );
    }

    #[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
    #[test]
    fn kqueue_sendfile_sends_the_range_and_advances_the_offset() {
        use std::io::Read;
        let source = scratch(b"0123456789");
        let (sink, mut peer) =
            std::os::unix::net::UnixStream::pair().unwrap_or_else(|e| unreachable!("{e}"));
        let mut offset: off_t = 3;
        let sent = swr_v1_os_sendfile(sink.as_raw_fd(), source.as_raw_fd(), Some(&mut offset), 4);
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
    #[cfg_attr(miri, ignore = "Miri has no mkostemp")]
    fn open_close_and_tmpfile_follow_upstream() {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut template = *b"/tmp/swr-ossupport-XXXXXX\0";
        let fd = tmpfile(&mut template).map_or(-1, std::os::fd::IntoRawFd::into_raw_fd);
        assert!(fd >= 0, "a temporary file");
        let path = std::ffi::CStr::from_bytes_until_nul(&template).unwrap_or_default();
        let reopened = file_open(fd, path, libc::O_RDONLY, 0);
        assert!(reopened >= 0, "reopened after closing the old descriptor");
        assert!(file_close(reopened), "closed");
        assert!(!file_close(-1), "nothing to close");
        let created =
            std::ffi::CString::new(format!("{}.new", path.to_string_lossy())).unwrap_or_default();
        let fd = file_open(-1, &created, libc::O_WRONLY | libc::O_CREAT, 0);
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
        assert!(file_close(fd), "closed");
        std::fs::remove_file(created.to_string_lossy().as_ref()).unwrap_or_default();
        std::fs::remove_file(path.to_string_lossy().as_ref()).unwrap_or_default();
    }
}
