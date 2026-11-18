//! The hooked fds' coroutine sockets (`swoole_coroutine_socket`, `accept`,
//! `socket_create`): fd to the [`OwnedCo`] it owns, shared by every thread.
//!
//! A lookup lends the handle, no reference count: the socket lives until the
//! fd's hooked `close` (or `socket_unwrap`) takes it out of the map, and is deleted
//! as what the map hands back drops. `close` takes it out only once
//! `Socket::close()` succeeds, which it does not while a coroutine is bound to the
//! socket, so a coroutine parked in an operation keeps a live pointer.
//!
//! `Socket::close()` cancels the bound coroutines, which resume inside it and may
//! close the fd themselves; the sockets those nested closes remove are deleted only
//! when this thread's outermost close returns.
use crate::co_socket::{Co, OwnedCo};
use libc::c_int;
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use swoole_rs_corebase::set_errno;

/// A live socket the map owns.
struct Socket(OwnedCo);

#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "OwnedCo is not Send so that only the map moves a socket between threads"
)]
// SAFETY: the map only stores and returns the socket; native code decides which
// thread uses it.
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
        .map(|socket| socket.0.co());
    not_socket(socket)
}

/// `socket`, setting errno `ENOTSOCK` when there is none.
fn not_socket<T>(socket: Option<T>) -> Option<T> {
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

/// Map `fd` to `socket`. With `replace` an existing socket gives way (`map[fd] =`),
/// else the new one does (`emplace`). Returns the socket the map does not hold,
/// deleted as it drops, which is after the lock is released.
pub(crate) fn insert(fd: c_int, socket: OwnedCo, replace: bool) -> Option<OwnedCo> {
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

/// Take `fd`'s socket out of the map, deleted as it drops; none with errno
/// `ENOTSOCK` if there is none.
#[must_use]
pub(crate) fn remove(fd: c_int) -> Option<OwnedCo> {
    not_socket(
        sockets()
            .as_mut()
            .and_then(|map| map.remove(&fd))
            .map(|socket| socket.0),
    )
}

thread_local! {
    /// How many hooked closes are running on this thread.
    static CLOSING: Cell<usize> = const { Cell::new(0) };
    /// The sockets they removed, which wait for the outermost one to return.
    /// Taken out and put back around one extend, which runs no close.
    static DOOMED: Cell<Vec<OwnedCo>> = const { Cell::new(Vec::new()) };
}

/// Remove the mapping before close(2) lets another socket reuse this number.
pub(crate) fn retire(fd: c_int) {
    let owned = remove(fd);
    let mut doomed = DOOMED.take();
    doomed.extend(owned);
    DOOMED.set(doomed);
}

/// `swoole_coroutine_close(fd)` for a coroutine socket: its close outcome, or
/// `None` (errno `ENOTSOCK`) when `fd` has no socket. A successful close removes
/// the socket from the map. Each removed socket is deleted after the outermost
/// close on this thread returns, so no close still running uses a freed one:
/// `close` gets the socket while the map holds it.
pub(crate) fn close(fd: c_int, close: impl FnOnce(Co) -> bool) -> Option<bool> {
    let socket = get(fd)?;
    CLOSING.set(CLOSING.get() + 1);
    let closed = close(socket);
    let depth = CLOSING.get() - 1;
    CLOSING.set(depth);
    if depth == 0 {
        drop(DOOMED.take());
    }
    Some(closed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::co_socket::unopened;
    use std::cell::RefCell;

    /// A socket that logs `delete <name>` as it is deleted.
    fn named(name: &'static str) -> OwnedCo {
        let socket = unopened();
        crate::socket_extra::set_close_hook(
            socket.co(),
            Box::new(move |_| log(&format!("delete {name}"))),
        );
        socket
    }

    use swoole_rs_corebase::sys::errno;

    #[test]
    fn lends_until_removed() {
        let (a, b) = (named("a"), named("b"));
        let (ca, cb) = (a.co(), b.co());
        let fd = 1_000_001;
        assert!(get(fd).is_none());
        assert_eq!(errno(), libc::ENOTSOCK);
        assert!(!exists(fd));

        assert!(insert(fd, a, true).is_none());
        assert!(exists(fd));
        assert_eq!(get(fd), Some(ca));
        // emplace keeps the first; the new one comes back to be deleted.
        let b = insert(fd, b, false).unwrap();
        assert_eq!(b.co(), cb);
        assert_eq!(get(fd), Some(ca));
        // assignment replaces; the old one comes back.
        let a = insert(fd, b, true).unwrap();
        assert_eq!(a.co(), ca);
        assert_eq!(get(fd), Some(cb));
        drop(a);
        assert_eq!(LOG.take(), ["delete a"]);

        assert_eq!(remove(fd).map(|b| b.co()), Some(cb));
        assert_eq!(LOG.take(), ["delete b"]);
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

    /// The cancelled reader's close, which succeeds.
    fn inner_close(_: Co) -> bool {
        log("inner close");
        retire(NESTED);
        true
    }

    /// The outer close cancels the reader, which closes the fd from inside it, then
    /// fails with close-wait while still holding the socket.
    fn outer_close(_: Co) -> bool {
        log("outer close");
        assert_eq!(close(NESTED, inner_close), Some(true));
        assert!(!exists(NESTED));
        log("outer returns");
        false
    }

    #[test]
    fn nested_close_defers_the_delete() {
        assert!(insert(NESTED, named("nested"), true).is_none());
        assert_eq!(close(NESTED, outer_close), Some(false));
        assert_eq!(
            LOG.take(),
            [
                "outer close",
                "inner close",
                "outer returns",
                "delete nested"
            ]
        );
        assert_eq!(close(NESTED, outer_close), None);
        assert_eq!(errno(), libc::ENOTSOCK);
    }
}
