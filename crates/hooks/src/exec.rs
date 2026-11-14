//! `System::exec(command, get_error_stream, buffer, status)`: run `command` under
//! `/bin/sh -c` and read its output through a coroutine socket over the
//! pipe, as upstream does, then reap the child.
use core::ffi::{CStr, c_int};
use swoole_rs_corebase::constants::SocketType;
use swoole_rs_reactor::wait_upstream::system_waitpid_safe;

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
    crate::wait_event::native::current();
    let Some((fd, pid)) = swoole_rs_corebase::swoole_shell_exec(command, errors) else {
        swoole_rs_corebase::log::sys_warning(
            c"exec",
            format_args!("Unable to execute '{}'", command.to_string_lossy()),
        );
        return None;
    };
    // `SocketImpl socket(fd, SW_SOCK_UNIX_STREAM)`, which takes the pipe.
    let socket = crate::co_socket::make(fd, 0, SocketType::SW_SOCK_UNIX_STREAM as c_int, 0, false);
    let mut output = Vec::new();
    let mut chunk = vec![0; CHUNK];
    loop {
        let read = socket.read(&mut chunk);
        let Some(filled) = usize::try_from(read).ok().filter(|&n| n > 0) else {
            break;
        };
        output.extend_from_slice(chunk.get(..filled).unwrap_or_default());
    }
    socket.close();
    // SAFETY: the socket `make` made, which nothing uses afterwards.
    unsafe { crate::co_socket::swr_v1_hook_co_socket_free(socket.this()) };
    (system_waitpid_safe(pid, Some(status), 0) == pid).then_some(output)
}
