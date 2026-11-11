//! The hooked fds' coroutine sockets (`swoole_coroutine_socket`, `accept`,
//! `socket_create`): fd to native `SocketImpl *`, shared by every thread.
//!
//! A lookup lends the raw pointer, no reference count: the socket lives until the
//! fd's hooked `close` (or `socket_unwrap`) takes it out of the map, and native code
//! deletes what the map hands back. `close` takes it out only once
//! `Socket::close()` succeeds, which it does not while a coroutine is bound to the
//! socket, so a coroutine parked in an operation keeps a live pointer.
//!
//! `Socket::close()` cancels the bound coroutines, which resume inside it and may
//! close the fd themselves; the sockets those nested closes remove are deleted only
//! when this thread's outermost close returns.
use libc::{c_int, c_void};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use swoole_rs_corebase::set_errno;

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

/// `fd`'s socket, or null with errno `ENOTSOCK`.
#[must_use]
pub fn get(fd: c_int) -> *mut c_void {
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
#[must_use]
pub fn exists(fd: c_int) -> bool {
    sockets().as_ref().is_some_and(|map| map.contains_key(&fd))
}

/// Map `fd` to `socket`. With `replace` an existing socket gives way (`map[fd] =`),
/// else the new one does (`emplace`). Returns the socket the map no longer holds,
/// for native code to delete, or null.
pub fn insert(fd: c_int, socket: *mut c_void, replace: bool) -> *mut c_void {
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
#[must_use]
pub fn remove(fd: c_int) -> *mut c_void {
    let socket = sockets()
        .as_mut()
        .and_then(|map| map.remove(&fd))
        .map_or(std::ptr::null_mut(), |socket| socket.0);
    if socket.is_null() {
        set_errno(libc::ENOTSOCK);
    }
    socket
}

thread_local! {
    /// How many hooked closes are running on this thread, and the sockets they
    /// removed, which wait for the outermost one to return.
    static CLOSING: RefCell<(usize, Vec<*mut c_void>)> = const { RefCell::new((0, Vec::new())) };
}

/// `swoole_coroutine_close(fd)` for a coroutine socket: 0 once `close(socket)`
/// succeeds and the socket leaves the map, -1 when it fails, 1 (errno `ENOTSOCK`)
/// when `fd` has no socket. `delete` frees each removed socket after the
/// outermost close on this thread returns, so no close still running uses a
/// freed one: `close` gets the socket while the map holds it, and `delete_socket`
/// once it is removed and no close still running uses it.
pub fn close(
    fd: c_int,
    close: impl FnOnce(*mut c_void) -> bool,
    mut delete_socket: impl FnMut(*mut c_void),
) -> c_int {
    let socket = get(fd);
    if socket.is_null() {
        return 1;
    }
    CLOSING.with_borrow_mut(|(depth, _)| *depth += 1);
    let closed = close(socket);
    let removed = if closed {
        remove(fd)
    } else {
        std::ptr::null_mut()
    };
    let doomed = CLOSING.with_borrow_mut(|(depth, doomed)| {
        if !removed.is_null() {
            doomed.push(removed);
        }
        *depth -= 1;
        if *depth == 0 {
            std::mem::take(doomed)
        } else {
            Vec::new()
        }
    });
    for socket in doomed {
        delete_socket(socket);
    }
    if closed { 0 } else { -1 }
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
        assert!(get(fd).is_null());
        assert_eq!(errno(), libc::ENOTSOCK);
        assert!(!exists(fd));

        assert!(insert(fd, a, true).is_null());
        assert!(exists(fd));
        assert_eq!(get(fd), a);
        // emplace keeps the first; the new one comes back to be deleted.
        assert_eq!(insert(fd, b, false), b);
        assert_eq!(get(fd), a);
        // assignment replaces; the old one comes back.
        assert_eq!(insert(fd, b, true), a);
        assert_eq!(get(fd), b);

        assert_eq!(remove(fd), b);
        assert!(!exists(fd));
        assert!(remove(fd).is_null());
        assert_eq!(errno(), libc::ENOTSOCK);
    }

    thread_local! {
        static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    const NESTED: c_int = 1_000_002;

    fn log(event: &str) {
        LOG.with_borrow_mut(|log| log.push(event.to_owned()));
    }

    fn delete(socket: *mut c_void) {
        log(&format!("delete {}", socket as usize));
    }

    /// The cancelled reader's close, which succeeds.
    fn inner_close(_: *mut c_void) -> bool {
        log("inner close");
        true
    }

    /// The outer close cancels the reader, which closes the fd from inside it, then
    /// fails with close-wait while still holding the socket.
    fn outer_close(_: *mut c_void) -> bool {
        log("outer close");
        assert_eq!(close(NESTED, inner_close, delete), 0);
        assert!(!exists(NESTED));
        log("outer returns");
        false
    }

    #[test]
    fn nested_close_defers_the_delete() {
        assert!(insert(NESTED, 8usize as *mut c_void, true).is_null());
        assert_eq!(close(NESTED, outer_close, delete), -1);
        assert_eq!(
            LOG.with_borrow(Clone::clone),
            ["outer close", "inner close", "outer returns", "delete 8"]
        );
        assert_eq!(close(NESTED, outer_close, delete), 1);
        assert_eq!(errno(), libc::ENOTSOCK);
    }
}
