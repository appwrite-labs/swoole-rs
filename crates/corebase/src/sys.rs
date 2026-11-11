//! Safe wrappers for the libc calls the crates make on plain descriptors and
//! process state. Buffers are Rust slices, so the calls have no memory
//! preconditions and callers need no `unsafe`. Results are libc's, with errno
//! left as the call set it.
#![expect(
    clippy::must_use_candidate,
    reason = "callers ignore a close or write result where the C they replace did"
)]
use libc::{c_int, off_t, pid_t};
use std::ffi::CStr;
use std::mem::MaybeUninit;

/// A byte a read may overwrite with any value: `u8`, or one not yet written
/// (`CppString::spare`). Readers only write their buffer, so they take either.
pub trait ReadSlot: sealed::Sealed {}
impl ReadSlot for u8 {}
impl ReadSlot for MaybeUninit<u8> {}

mod sealed {
    /// Only one-byte types every bit pattern initializes.
    pub trait Sealed {}
    impl Sealed for u8 {}
    impl Sealed for std::mem::MaybeUninit<u8> {}
}

/// This thread's errno.
#[must_use]
pub fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Sets this thread's errno.
pub fn set_errno(value: c_int) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: returns this thread's errno slot, live for the thread.
    let slot = unsafe { libc::__errno_location() };
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    // SAFETY: returns this thread's errno slot, live for the thread.
    let slot = unsafe { libc::__error() };
    // SAFETY: `slot` is this thread's errno.
    unsafe { slot.write(value) };
}

/// `strerror(3)`: libc's message for `code`, as std's `strerror_r` reads it.
#[must_use]
pub fn strerror(code: c_int) -> String {
    let mut text = std::io::Error::from_raw_os_error(code).to_string();
    // std appends the code; strip that, leaving libc's message.
    let message = text
        .strip_suffix(&format!(" (os error {code})"))
        .map_or(text.len(), str::len);
    text.truncate(message);
    text
}

/// `read(2)` into `buf`.
pub fn read(fd: c_int, buf: &mut [impl ReadSlot]) -> isize {
    // SAFETY: `buf` is writable for its length.
    unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) }
}

/// `write(2)` of `buf`.
pub fn write(fd: c_int, buf: &[u8]) -> isize {
    // SAFETY: `buf` is readable for its length.
    unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) }
}

/// `pread(2)` into `buf` at `offset`.
pub fn pread(fd: c_int, buf: &mut [impl ReadSlot], offset: off_t) -> isize {
    // SAFETY: `buf` is writable for its length.
    unsafe { libc::pread(fd, buf.as_mut_ptr().cast(), buf.len(), offset) }
}

/// `pwrite(2)` of `buf` at `offset`.
pub fn pwrite(fd: c_int, buf: &[u8], offset: off_t) -> isize {
    // SAFETY: `buf` is readable for its length.
    unsafe { libc::pwrite(fd, buf.as_ptr().cast(), buf.len(), offset) }
}

/// `close(2)`.
pub fn close(fd: c_int) -> c_int {
    // SAFETY: close(2) has no memory preconditions.
    unsafe { libc::close(fd) }
}

/// `dup(2)`.
#[must_use]
pub fn dup(fd: c_int) -> c_int {
    // SAFETY: dup(2) has no memory preconditions.
    unsafe { libc::dup(fd) }
}

/// `dup2(2)`.
pub fn dup2(from: c_int, to: c_int) -> c_int {
    // SAFETY: dup2(2) has no memory preconditions.
    unsafe { libc::dup2(from, to) }
}

/// `fcntl(2)` with an integer argument (`F_GETFL`, `F_SETFL`, `F_GETFD`,
/// `F_SETFD`, `F_DUPFD`, ...); commands that take no argument ignore it.
pub fn fcntl(fd: c_int, cmd: c_int, arg: c_int) -> c_int {
    // SAFETY: an integer argument has no memory preconditions.
    unsafe { libc::fcntl(fd, cmd, arg) }
}

/// `flock(2)`.
pub fn flock(fd: c_int, operation: c_int) -> c_int {
    // SAFETY: flock(2) has no memory preconditions.
    unsafe { libc::flock(fd, operation) }
}

/// `poll(2)` of `fds`.
pub fn poll(fds: &mut [libc::pollfd], timeout: c_int) -> c_int {
    let count = libc::nfds_t::try_from(fds.len()).unwrap_or(libc::nfds_t::MAX);
    // SAFETY: `fds` holds `count` writable pollfds.
    unsafe { libc::poll(fds.as_mut_ptr(), count, timeout) }
}

/// `pipe(2)`: the read and write ends, or `None` with errno set.
#[must_use]
pub fn pipe() -> Option<[c_int; 2]> {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for two descriptors.
    (unsafe { libc::pipe(fds.as_mut_ptr()) } == 0).then_some(fds)
}

/// `fstat(2)`: the descriptor's status, or `None` with errno set.
#[must_use]
pub fn fstat(fd: c_int) -> Option<libc::stat> {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `stat` is writable storage for one stat.
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: fstat succeeded, so it filled `stat`.
    Some(unsafe { stat.assume_init() })
}

/// `sleep(3)`: the seconds left when a signal cut it short.
pub fn sleep(seconds: libc::c_uint) -> libc::c_uint {
    // SAFETY: sleep(3) has no memory preconditions.
    unsafe { libc::sleep(seconds) }
}

/// `usleep(3)`.
pub fn usleep(microseconds: libc::useconds_t) -> c_int {
    // SAFETY: usleep(3) has no memory preconditions.
    unsafe { libc::usleep(microseconds) }
}

/// `nanosleep(2)` for `request`, the time left going to `remaining`.
pub fn nanosleep(request: &libc::timespec, remaining: &mut libc::timespec) -> c_int {
    // SAFETY: both are live timespecs; nanosleep reads one and writes the other.
    unsafe { libc::nanosleep(request, remaining) }
}

/// `open(2)` of `path`; `mode` applies when `flags` has `O_CREAT`.
pub fn open(path: &CStr, flags: c_int, mode: libc::mode_t) -> c_int {
    // SAFETY: `path` is NUL-terminated; the mode is promoted as C does.
    unsafe { libc::open(path.as_ptr(), flags, libc::c_uint::from(mode)) }
}

/// `unlink(2)`.
pub fn unlink(path: &CStr) -> c_int {
    // SAFETY: `path` is NUL-terminated.
    unsafe { libc::unlink(path.as_ptr()) }
}

/// `getpid(2)`.
#[must_use]
pub fn getpid() -> pid_t {
    // A pid fits pid_t; std calls getpid(2), which has no preconditions.
    std::process::id().cast_signed()
}

/// `clock_gettime(2)`: the clock's time, or `None` with errno set.
#[must_use]
pub fn clock_gettime(clock: libc::clockid_t) -> Option<libc::timespec> {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is writable storage for one timespec.
    (unsafe { libc::clock_gettime(clock, &raw mut now) } == 0).then_some(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_calls_round_trip_through_a_pipe() {
        let mut fds = [0; 2];
        // SAFETY: `fds` holds two descriptors.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let [reader, writer] = fds;
        assert_eq!(write(writer, b"abc"), 3);
        let copy = dup(reader);
        assert!(copy >= 0);
        assert_eq!(fcntl(copy, libc::F_SETFL, libc::O_NONBLOCK), 0);
        assert_ne!(fcntl(copy, libc::F_GETFL, 0) & libc::O_NONBLOCK, 0);
        let mut buf = [0; 8];
        assert_eq!(read(copy, &mut buf), 3);
        assert_eq!(&buf[..3], b"abc");
        assert_eq!(read(copy, &mut buf), -1);
        assert_eq!(errno(), libc::EAGAIN);
        set_errno(0);
        assert_eq!(errno(), 0);
        for fd in [reader, writer, copy] {
            assert_eq!(close(fd), 0);
        }
        assert_eq!(close(copy), -1);
        assert_eq!(errno(), libc::EBADF);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no strerror")]
    fn strerror_is_libc_message() {
        for code in [libc::ENOENT, libc::EAGAIN, libc::EBADF, 0, 9999] {
            // SAFETY: strerror returns a NUL-terminated message for any code.
            let libc = unsafe { CStr::from_ptr(libc::strerror(code)) };
            assert_eq!(strerror(code), libc.to_str().unwrap(), "code {code}");
        }
    }

    #[test]
    fn process_and_clock() {
        assert_eq!(getpid().unsigned_abs(), std::process::id());
        assert!(clock_gettime(libc::CLOCK_MONOTONIC).is_some());
        assert!(clock_gettime(libc::clockid_t::MAX).is_none());
    }
}
