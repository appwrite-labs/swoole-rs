//! `System::exec(command, get_error_stream, buffer, status)`: run `command` under
//! `/bin/sh -c` and read its output through a coroutine socket over the
//! pipe, as upstream does, then reap the child.
use core::ffi::{CStr, c_char, c_int};
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::constants::SocketType;
use swoole_rs_reactor::wait_upstream::system_waitpid_safe;

/// `SW_ERROR_CO_OUT_OF_COROUTINE`.
const OUT_OF_COROUTINE: c_int = 10001;

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
    if swoole_rs_coroutine::current().is_null() {
        // SAFETY: Swoole's fatal error hook; this frame owns nothing.
        unsafe {
            swoole_rs_corebase::fatal_error(
                OUT_OF_COROUTINE,
                c"API must be called in the coroutine",
            );
        }
    }
    let mut pid = 0;
    // SAFETY: a NUL-terminated command and a writable pid.
    let fd =
        unsafe { swoole_rs_corebase::swoole_shell_exec(command.as_ptr(), &raw mut pid, errors) };
    if fd < 0 {
        swoole_rs_corebase::log::sys_warning(
            c"exec",
            format_args!("Unable to execute '{}'", command.to_string_lossy()),
        );
        return None;
    }
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

/// `std::shared_ptr<T>`'s layout in libc++ and libstdc++: the object, then the
/// control block.
#[repr(C)]
pub struct SharedPtr<T> {
    pub object: *mut T,
    pub control: *mut core::ffi::c_void,
}

/// `bool System::exec(const char *, bool, std::shared_ptr<String>, int *)`; the
/// Itanium ABI passes the `shared_ptr` by address and the caller destroys it.
///
/// # Safety
/// `command` is NUL-terminated and `buffer` names a live String.
#[cfg_attr(
    all(feature = "upstream-symbols", target_os = "macos"),
    unsafe(
        export_name = "_ZN6swoole9coroutine6System4execEPKcbNSt3__110shared_ptrINS_6StringEEEPi"
    )
)]
#[cfg_attr(
    all(feature = "upstream-symbols", not(target_os = "macos")),
    unsafe(export_name = "_ZN6swoole9coroutine6System4execEPKcbSt10shared_ptrINS_6StringEEPi")
)]
pub unsafe extern "C" fn system_exec(
    command: *const c_char,
    errors: bool,
    buffer: &SharedPtr<CppString>,
    status: &mut c_int,
) -> bool {
    // SAFETY: a NUL-terminated command, as the caller promises.
    let command = unsafe { CStr::from_ptr(command) };
    // SAFETY: the shared_ptr names a live String.
    let string = unsafe { &mut *buffer.object };
    exec(command, errors, status)
        .map(|output| string.append(&output))
        .is_some()
}
