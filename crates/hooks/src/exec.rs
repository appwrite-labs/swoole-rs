//! `System::exec(command, get_error_stream, buffer, status)`: run `command` under
//! `/bin/sh -c` and read its output through a coroutine socket over the
//! pipe, as upstream does, then reap the child.
use core::ffi::{CStr, c_int};
use swoole_rs_corebase::constants::SocketType;
use swoole_rs_corebase::{coroutine, reactor, sys};

/// The most one read takes from the pipe: a Linux pipe's default capacity.
const CHUNK: usize = 1 << 16;

/// Reads `command`'s output until the pipe's end, a read error, the socket's read
/// timeout or a cancel; the output once the child was reaped. The socket's readable
/// event resumes this coroutine at once, as upstream's does, so the reap runs before
/// the same loop pass dispatches the child's SIGCHLD. A deferred resume let the
/// SIGCHLD handler that a coroutine `waitPid` installs reap the child first, and exec
/// then failed.
///
/// Outside a coroutine it raises upstream's fatal error, which does not return.
pub fn exec(command: &CStr, errors: bool, status: &mut c_int) -> Option<Vec<u8>> {
    _ = swoole_rs_corebase::coroutine::current_safe();
    let Some((fd, pid)) = swoole_rs_corebase::swoole_shell_exec(command, errors) else {
        swoole_rs_corebase::log::sys_warning(
            c"exec",
            format_args!("Unable to execute '{}'", command.to_string_lossy()),
        );
        return None;
    };
    // `SocketImpl socket(fd, SW_SOCK_UNIX_STREAM)`, which takes the pipe.
    let owned = crate::co_socket::owned(fd, 0, SocketType::SW_SOCK_UNIX_STREAM as c_int, 0, false);
    let socket = owned.co();
    let mut output = Vec::new();
    let mut chunk = vec![0; CHUNK];
    while let Some(filled) = socket.read(&mut chunk).filter(|&n| n > 0) {
        output.extend_from_slice(chunk.get(..filled).unwrap_or_default());
    }
    socket.close();
    drop(owned);
    (system_waitpid_safe(pid, Some(status), 0) == pid).then_some(output)
}

/// `pid_t System::waitpid_safe(pid_t, int *, int)`: in a coroutine, polls
/// `waitpid(WNOHANG)` between coroutine sleeps that back off, without SIGCHLD.
/// `status` may be null, as `waitpid`'s.
#[unsafe(export_name = "_ZN6swoole9coroutine6System12waitpid_safeEiPii")]
#[must_use]
pub extern "C" fn system_waitpid_safe(
    pid: libc::pid_t,
    status: Option<&mut c_int>,
    options: c_int,
) -> libc::pid_t {
    /// `SW_FILE_LOCK_DEFAULT_SECOND` and `SW_FILE_LOCK_MAX_SECOND`.
    const LOCK_DEFAULT_SECOND: f64 = 0.001;
    const LOCK_MAX_SECOND: f64 = 0.1;
    let mut status = status;
    let outside =
        reactor::event_api::swr_v1_event_current().is_null() || coroutine::current().is_null();
    if outside || options & libc::WNOHANG != 0 {
        return sys::waitpid(pid, status, options).map_or(-1, |pid| pid.unwrap_or(0));
    }
    if let Some(result) =
        reactor::wait_upstream::iouring_waitpid(pid, status.as_deref_mut(), options, -1.0)
    {
        return result;
    }
    let mut second = LOCK_DEFAULT_SECOND;
    loop {
        match sys::waitpid(pid, status.as_deref_mut(), libc::WNOHANG) {
            Ok(Some(child)) => return child,
            Err(_) => return -1,
            // In a coroutine, checked above. A failed sleep keeps its errno and
            // returns the wait's no-child result, as upstream does.
            Ok(None) if !crate::sleep::system_sleep(second) => return 0,
            Ok(None) => {}
        }
        second = reactor::backoff(second, LOCK_DEFAULT_SECOND, LOCK_MAX_SECOND);
    }
}
