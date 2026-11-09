//! `System::exec(command, get_error_stream, buffer, status)`: run `command` under
//! `/bin/sh -c` and read its output into `buffer` in this coroutine, as upstream's
//! coroutine socket over the pipe did, then reap the child.
use core::ffi::{CStr, c_char, c_int};
use swoole_rs_buffer::string::CppString;

// SAFETY: crates/coroutine's and crates/reactor's (upstream symbols).
unsafe extern "C" {
    safe fn swr_v1_coroutine_current() -> *mut core::ffi::c_void;
    #[link_name = "_ZN6swoole9coroutine6System12waitpid_safeEiPii"]
    fn system_waitpid_safe(pid: libc::pid_t, status: *mut c_int, options: c_int) -> libc::pid_t;
}

/// `SW_ERROR_CO_OUT_OF_COROUTINE`.
const OUT_OF_COROUTINE: c_int = 10001;

/// Reads `command`'s output onto `buffer`, doubling it whenever it fills. Each wait
/// for output is bounded by the default socket read timeout, and a read error, a
/// timeout or a cancel ends the output. True when the child was reaped.
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
    if swr_v1_coroutine_current().is_null() {
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
    // SAFETY: fcntl(2) on the pipe this function owns.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    // SAFETY: as above.
    unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    // SAFETY: a plain read of the defaults every new socket copies.
    let timeout = unsafe { swoole_rs_reactor::swr_v1_socket_defaults.timeouts.read };
    loop {
        let spare = buffer.size - buffer.length;
        // SAFETY: the buffer has `spare` writable bytes past its length.
        let read = unsafe { libc::read(fd, buffer.str.wrapping_add(buffer.length).cast(), spare) };
        if read > 0 {
            buffer.length += read.unsigned_abs();
            if buffer.length == buffer.size {
                buffer.extend(buffer.size.wrapping_mul(2));
            }
            continue;
        }
        let waited = match std::io::Error::last_os_error().raw_os_error() {
            _ if read == 0 => break,
            Some(libc::EINTR) => continue,
            // SAFETY: waits on a descriptor this function owns.
            Some(libc::EAGAIN) => unsafe {
                crate::wait_event::system_wait_event(fd, crate::wait_event::READ, timeout)
            },
            _ => break,
        };
        if waited < 0 {
            break;
        }
    }
    // SAFETY: the pipe is this function's.
    unsafe { libc::close(fd) };
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
/// As [`exec`], with `buffer` naming a live String and `status` writable.
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
    buffer: *const SharedPtr<CppString>,
    status: *mut c_int,
) -> bool {
    // SAFETY: a NUL-terminated command, as the caller promises.
    let command = unsafe { CStr::from_ptr(command) };
    // SAFETY: the caller's live shared_ptr.
    let string = unsafe { (*buffer).object };
    // SAFETY: the shared_ptr names a live String.
    let string = unsafe { &mut *string };
    // SAFETY: the caller's writable status.
    let status = unsafe { &mut *status };
    // SAFETY: in a coroutine, as the caller promises.
    unsafe { exec(command, errors, string, status) }
}
