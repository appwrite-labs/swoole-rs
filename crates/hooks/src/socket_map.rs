//! The hooked fds' coroutine sockets (`swoole_coroutine_socket`, `accept`,
//! `socket_create`): fd to [`Co`], shared by every thread.
//!
//! A lookup lends the handle, no reference count: the socket lives until the
//! fd's hooked `close` (or `socket_unwrap`) takes it out of the map, and native code
//! deletes what the map hands back. `close` takes it out only once
//! `Socket::close()` succeeds, which it does not while a coroutine is bound to the
//! socket, so a coroutine parked in an operation keeps a live pointer.
//!
//! `Socket::close()` cancels the bound coroutines, which resume inside it and may
//! close the fd themselves; the sockets those nested closes remove are deleted only
//! when this thread's outermost close returns.
use crate::co_socket::Co;
use libc::c_int;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use swoole_rs_corebase::set_errno;

/// A live socket the map holds.
struct Socket(Co);

// SAFETY: the map only stores and returns the handle; native code, which owns the
// socket, decides which thread uses it.
unsafe impl Send for Socket {}

static SOCKETS: Mutex<Option<HashMap<c_int, Socket>>> = Mutex::new(None);

fn sockets() -> MutexGuard<'static, Option<HashMap<c_int, Socket>>> {
    // A panic never happens while the lock is held, but stay usable if it did.
    SOCKETS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `fd`'s socket, or none with errno `ENOTSOCK`.
#[must_use]
pub fn get(fd: c_int) -> Option<Co> {
    let socket = sockets()
        .as_ref()
        .and_then(|map| map.get(&fd))
        .map(|socket| socket.0);
    not_socket(socket)
}

/// `socket`, setting errno `ENOTSOCK` when there is none.
fn not_socket(socket: Option<Co>) -> Option<Co> {
    if socket.is_none() {
        set_errno(libc::ENOTSOCK);
    }
    socket
}

/// Whether `fd` has a socket.
#[must_use]
pub fn exists(fd: c_int) -> bool {
    sockets().as_ref().is_some_and(|map| map.contains_key(&fd))
}

/// Map `fd` to the live `socket`. With `replace` an existing socket gives way
/// (`map[fd] =`), else the new one does (`emplace`). Returns the socket the map no
/// longer holds, for native code to delete.
pub fn insert(fd: c_int, socket: Co, replace: bool) -> Option<Co> {
    let mut sockets = sockets();
    let map = sockets.get_or_insert_with(HashMap::new);
    match map.get_mut(&fd) {
        Some(existing) if replace => Some(std::mem::replace(existing, Socket(socket)).0),
        Some(_) => Some(socket),
        None => {
            map.insert(fd, Socket(socket));
            None
        }
    }
}

/// Take `fd`'s socket out of the map, for native code to delete; none with errno
/// `ENOTSOCK` if there is none.
#[must_use]
pub fn remove(fd: c_int) -> Option<Co> {
    not_socket(
        sockets()
            .as_mut()
            .and_then(|map| map.remove(&fd))
            .map(|socket| socket.0),
    )
}

thread_local! {
    /// How many hooked closes are running on this thread, and the sockets they
    /// removed, which wait for the outermost one to return.
    static CLOSING: RefCell<(usize, Vec<Co>)> = const { RefCell::new((0, Vec::new())) };
}

/// `swoole_coroutine_close(fd)` for a coroutine socket: 0 once `close(socket)`
/// succeeds and the socket leaves the map, -1 when it fails, 1 (errno `ENOTSOCK`)
/// when `fd` has no socket. `delete` frees each removed socket after the
/// outermost close on this thread returns, so no close still running uses a
/// freed one: `close` gets the socket while the map holds it, and `delete_socket`
/// once it is removed and no close still running uses it.
pub fn close(
    fd: c_int,
    close: impl FnOnce(Co) -> bool,
    mut delete_socket: impl FnMut(Co),
) -> c_int {
    let Some(socket) = get(fd) else {
        return 1;
    };
    CLOSING.with_borrow_mut(|(depth, _)| *depth += 1);
    let closed = close(socket);
    let removed = if closed { remove(fd) } else { None };
    let doomed = CLOSING.with_borrow_mut(|(depth, doomed)| {
        doomed.extend(removed);
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

    /// A handle the map only stores, never dereferenced.
    fn fake(at: usize) -> Co {
        unsafe { Co::new(core::ptr::without_provenance_mut(at)) }
    }

    fn errno() -> c_int {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }

    #[test]
    fn lends_until_removed() {
        let (a, b) = (fake(8), fake(16));
        let fd = 1_000_001;
        assert!(get(fd).is_none());
        assert_eq!(errno(), libc::ENOTSOCK);
        assert!(!exists(fd));

        assert!(insert(fd, a, true).is_none());
        assert!(exists(fd));
        assert_eq!(get(fd), Some(a));
        // emplace keeps the first; the new one comes back to be deleted.
        assert_eq!(insert(fd, b, false), Some(b));
        assert_eq!(get(fd), Some(a));
        // assignment replaces; the old one comes back.
        assert_eq!(insert(fd, b, true), Some(a));
        assert_eq!(get(fd), Some(b));

        assert_eq!(remove(fd), Some(b));
        assert!(!exists(fd));
        assert!(remove(fd).is_none());
        assert_eq!(errno(), libc::ENOTSOCK);
    }

    thread_local! {
        static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    const NESTED: c_int = 1_000_002;

    fn log(event: &str) {
        LOG.with_borrow_mut(|log| log.push(event.to_owned()));
    }

    fn delete(socket: Co) {
        log(&format!("delete {}", socket.this() as usize));
    }

    /// The cancelled reader's close, which succeeds.
    fn inner_close(_: Co) -> bool {
        log("inner close");
        true
    }

    /// The outer close cancels the reader, which closes the fd from inside it, then
    /// fails with close-wait while still holding the socket.
    fn outer_close(_: Co) -> bool {
        log("outer close");
        assert_eq!(close(NESTED, inner_close, delete), 0);
        assert!(!exists(NESTED));
        log("outer returns");
        false
    }

    #[test]
    fn nested_close_defers_the_delete() {
        assert!(insert(NESTED, fake(8), true).is_none());
        assert_eq!(close(NESTED, outer_close, delete), -1);
        assert_eq!(
            LOG.with_borrow(Clone::clone),
            ["outer close", "inner close", "outer returns", "delete 8"]
        );
        assert_eq!(close(NESTED, outer_close, delete), 1);
        assert_eq!(errno(), libc::ENOTSOCK);
    }
}
