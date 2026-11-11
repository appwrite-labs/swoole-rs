//! `System::exec(command, get_error_stream, buffer, status)`: run `command` under
//! `/bin/sh -c` and read its output into `buffer` through a coroutine socket over the
//! pipe, as upstream does, then reap the child.
use core::ffi::{CStr, c_char, c_int};
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::constants::SocketType;
use swoole_rs_reactor::wait_upstream::system_waitpid_safe;

/// `SW_ERROR_CO_OUT_OF_COROUTINE`.
const OUT_OF_COROUTINE: c_int = 10001;

/// Reads `command`'s output onto `buffer`, doubling it whenever it fills, until the
/// pipe's end, a read error, the socket's read timeout or a cancel. True when the
/// child was reaped. The socket's readable event resumes this coroutine at once, as
/// upstream's does, so the reap runs before the same loop pass dispatches the child's
/// SIGCHLD. A deferred resume let the SIGCHLD handler that a coroutine `waitPid`
/// installs reap the child first, and exec then failed.
///
/// # Safety
/// In a coroutine (outside one it raises upstream's fatal error, which does not
/// return); `command` is NUL-terminated and `buffer` has a buffer.
pub unsafe fn exec(
    command: &CStr,
    errors: bool,
    buffer: &mut CppString,
    status: &mut c_int,
) -> bool {
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
        return false;
    }
    // `SocketImpl socket(fd, SW_SOCK_UNIX_STREAM)`, which takes the pipe.
    let socket = crate::co_socket::make(fd, 0, SocketType::SW_SOCK_UNIX_STREAM as c_int, 0, false);
    loop {
        // SAFETY: the buffer has `size - length` writable bytes past its length.
        let spare = unsafe {
            core::slice::from_raw_parts_mut(
                buffer.str.wrapping_add(buffer.length).cast::<u8>(),
                buffer.size - buffer.length,
            )
        };
        let read = socket.read(spare);
        if read <= 0 {
            break;
        }
        buffer.length += read.unsigned_abs();
        if buffer.length == buffer.size {
            buffer.extend(buffer.size.wrapping_mul(2));
        }
    }
    socket.close();
    // SAFETY: the socket `make` made, which nothing uses afterwards.
    unsafe { crate::co_socket::swr_v1_hook_co_socket_free(socket.this()) };
    // SAFETY: `status` is writable.
    unsafe { system_waitpid_safe(pid, status, 0) == pid }
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
/// As [`exec`], with `buffer` naming a live String.
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
    // SAFETY: in a coroutine, as the caller promises.
    unsafe { exec(command, errors, string, status) }
}
