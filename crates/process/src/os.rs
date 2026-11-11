//! The process system calls that take only integers, as safe functions.
use std::ffi::c_int;

/// getpid(2).
#[must_use]
pub fn getpid() -> libc::pid_t {
    // A pid fits pid_t; std calls getpid(2), which has no preconditions.
    std::process::id().cast_signed()
}

/// kill(2): 0, or -1 with `errno` set.
#[expect(
    clippy::must_use_candidate,
    reason = "upstream ignores kill's result where it only sends a signal"
)]
pub fn kill(pid: libc::pid_t, signo: c_int) -> c_int {
    // SAFETY: kill(2) takes plain integers.
    unsafe { libc::kill(pid, signo) }
}
