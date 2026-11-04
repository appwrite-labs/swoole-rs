//! The hooked fds' coroutine sockets (`swoole_coroutine_socket`, `accept`,
//! `socket_create`): fd to native `SocketImpl *`, shared by every thread.
//!
//! A lookup lends the raw pointer, no reference count: the socket lives until the
//! fd's hooked `close` (or `socket_unwrap`) takes it out of the map, and native code
//! deletes what the map hands back. `close` takes it out only once
//! `Socket::close()` succeeds, which it does not while a coroutine is bound to the
//! socket, so a coroutine parked in an operation keeps a live pointer.
use libc::{c_int, c_void};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// A native `SocketImpl *`, only dereferenced by native code.
struct Socket(*mut c_void);

// SAFETY: the map only stores and returns the pointer; native code, which owns the
// socket, decides which thread uses it.
unsafe impl Send for Socket {}

static SOCKETS: Mutex<Option<HashMap<c_int, Socket>>> = Mutex::new(None);

fn sockets() -> MutexGuard<'static, Option<HashMap<c_int, Socket>>> {
    // A panic never happens while the lock is held, but stay usable if it did.
    SOCKETS.lock().unwrap_or_else(PoisonError::into_inner)
}

fn set_errno(code: c_int) {
    // SAFETY: this thread's errno slot.
    #[cfg(target_os = "linux")]
    let slot = unsafe { libc::__errno_location() };
    // SAFETY: as above.
    #[cfg(not(target_os = "linux"))]
    let slot = unsafe { libc::__error() };
    // SAFETY: the slot is this thread's and live.
    unsafe { *slot = code };
}

/// `fd`'s socket, or null with errno `ENOTSOCK`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_socket_get(fd: c_int) -> *mut c_void {
    let socket = sockets()
        .as_ref()
        .and_then(|map| map.get(&fd))
        .map_or(std::ptr::null_mut(), |socket| socket.0);
    if socket.is_null() {
        set_errno(libc::ENOTSOCK);
    }
    socket
}

/// Whether `fd` has a socket.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_socket_exists(fd: c_int) -> bool {
    sockets().as_ref().is_some_and(|map| map.contains_key(&fd))
}

/// Map `fd` to `socket`. With `replace` an existing socket gives way (`map[fd] =`),
/// else the new one does (`emplace`). Returns the socket the map no longer holds,
/// for native code to delete, or null.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_socket_insert(
    fd: c_int,
    socket: *mut c_void,
    replace: bool,
) -> *mut c_void {
    let mut sockets = sockets();
    let map = sockets.get_or_insert_with(HashMap::new);
    match map.get_mut(&fd) {
        Some(existing) if replace => std::mem::replace(existing, Socket(socket)).0,
        Some(_) => socket,
        None => {
            map.insert(fd, Socket(socket));
            std::ptr::null_mut()
        }
    }
}

/// Take `fd`'s socket out of the map, for native code to delete; null with errno
/// `ENOTSOCK` if there is none.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_socket_remove(fd: c_int) -> *mut c_void {
    let socket = sockets()
        .as_mut()
        .and_then(|map| map.remove(&fd))
        .map_or(std::ptr::null_mut(), |socket| socket.0);
    if socket.is_null() {
        set_errno(libc::ENOTSOCK);
    }
    socket
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errno() -> c_int {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }

    #[test]
    fn lends_until_removed() {
        let (a, b) = (8usize as *mut c_void, 16usize as *mut c_void);
        let fd = 1_000_001;
        assert!(swr_v1_hook_socket_get(fd).is_null());
        assert_eq!(errno(), libc::ENOTSOCK);
        assert!(!swr_v1_hook_socket_exists(fd));

        assert!(swr_v1_hook_socket_insert(fd, a, true).is_null());
        assert!(swr_v1_hook_socket_exists(fd));
        assert_eq!(swr_v1_hook_socket_get(fd), a);
        // emplace keeps the first; the new one comes back to be deleted.
        assert_eq!(swr_v1_hook_socket_insert(fd, b, false), b);
        assert_eq!(swr_v1_hook_socket_get(fd), a);
        // assignment replaces; the old one comes back.
        assert_eq!(swr_v1_hook_socket_insert(fd, b, true), a);
        assert_eq!(swr_v1_hook_socket_get(fd), b);

        assert_eq!(swr_v1_hook_socket_remove(fd), b);
        assert!(!swr_v1_hook_socket_exists(fd));
        assert!(swr_v1_hook_socket_remove(fd).is_null());
        assert_eq!(errno(), libc::ENOTSOCK);
    }
}
