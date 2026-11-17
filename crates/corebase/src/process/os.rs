//! The process system calls that take only integers, as safe functions.

/// getpid(2).
#[must_use]
pub fn getpid() -> libc::pid_t {
    // A pid fits pid_t; std calls getpid(2), which has no preconditions.
    std::process::id().cast_signed()
}

pub use crate::sys::kill;
