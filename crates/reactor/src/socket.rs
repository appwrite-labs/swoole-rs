//! [`Socket`]: a live `network::Socket`, proven so once where its pointer comes
//! from (a reactor event, a connection, an owner's field), then read and edited
//! without `unsafe`.
use crate::SocketRecord;
use std::{cell::Cell, ffi::c_int, marker::PhantomData, ptr::NonNull};

/// A live `network::Socket` for `'a`, on this thread. It is `Copy` and holds no
/// Rust reference between calls, so native code and other handles may touch the
/// same record in between; [`read`](Self::read) and [`edit`](Self::edit) borrow it
/// only for their closure, and refuse to nest.
#[derive(Clone, Copy)]
pub struct Socket<'a> {
    record: NonNull<SocketRecord>,
    /// This thread's [`BORROWED`], looked up once per handle: a thread-local in a
    /// `dlopen`ed library costs a TLS descriptor call per lookup.
    borrowed: &'a Cell<bool>,
    _live: PhantomData<(&'a mut SocketRecord, *mut ())>,
}

thread_local! {
    /// Whether a `read` or `edit` closure is running on this thread.
    static BORROWED: Cell<bool> = const { Cell::new(false) };
}

/// This thread's [`BORROWED`], looked up once for any number of handles: an op
/// runner that builds several keeps one, where [`Socket::from_ptr`] looks it up
/// for each (#2964).
#[derive(Clone, Copy)]
pub struct SocketGuard(&'static Cell<bool>);

impl SocketGuard {
    #[must_use]
    pub fn current() -> Self {
        let flag = BORROWED.with(std::ptr::from_ref);
        // SAFETY: a const thread-local without a destructor lives as long as its
        // thread, and the guard is `!Send`, so it never leaves this one.
        Self(unsafe { &*flag })
    }

    /// [`Socket::from_ptr`] with this guard.
    /// # Safety
    /// As [`Socket::from_ptr`], on the guard's thread.
    #[must_use]
    pub unsafe fn socket<'a>(self, record: *mut SocketRecord) -> Option<Socket<'a>> {
        Some(Socket {
            record: NonNull::new(record)?,
            borrowed: self.0,
            _live: PhantomData,
        })
    }
}

/// Clears [`BORROWED`] when the closure returns or unwinds.
struct Borrow<'a>(&'a Cell<bool>);

impl<'a> Borrow<'a> {
    fn take(flag: &'a Cell<bool>) -> Self {
        assert!(
            !flag.replace(true),
            "a socket's read or edit runs inside another's"
        );
        Self(flag)
    }
}

impl Drop for Borrow<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl Socket<'_> {
    /// The socket `record` points to, or None for null.
    /// # Safety
    /// A non-null `record` is a live `network::Socket` for the handle's lifetime, used on this
    /// thread, which nothing frees or reaches through a Rust reference meanwhile
    /// (native code and raw pointers may, between this handle's calls).
    #[must_use]
    pub unsafe fn from_ptr(record: *mut SocketRecord) -> Option<Self> {
        let record = NonNull::new(record)?;
        Some(Self {
            record,
            borrowed: SocketGuard::current().0,
            _live: PhantomData,
        })
    }

    /// The record's address, for native calls.
    #[must_use]
    pub fn as_ptr(self) -> *mut SocketRecord {
        self.record.as_ptr()
    }

    /// `read`'s result over the record.
    /// # Panics
    /// Inside another socket's `read` or `edit` closure.
    pub fn read<R>(self, read: impl FnOnce(&SocketRecord) -> R) -> R {
        let _borrow = Borrow::take(self.borrowed);
        // SAFETY: live for the handle's lifetime (from_ptr), and no other Rust reference to it exists:
        // handles form one only here and in `edit`, which cannot nest.
        read(unsafe { self.record.as_ref() })
    }

    /// `edit`'s result over the record, which it may change.
    /// # Panics
    /// Inside another socket's `read` or `edit` closure.
    pub fn edit<R>(self, edit: impl FnOnce(&mut SocketRecord) -> R) -> R {
        let _borrow = Borrow::take(self.borrowed);
        let mut record = self.record;
        // SAFETY: as in `read`; this is the only reference while `edit` runs.
        edit(unsafe { record.as_mut() })
    }

    /// `fd`.
    #[must_use]
    pub fn fd(self) -> c_int {
        self.read(|record| record.fd)
    }

    /// `Socket::wait_event`: waits outside any borrow, since the wait runs signal handlers that
    /// may reach other sockets.
    #[must_use]
    pub fn wait_event(self, timeout_ms: c_int, events: c_int) -> c_int {
        let (fd, dont_restart) = self.read(|record| (record.fd, record.dont_restart != 0));
        crate::socket_upstream::wait_event(fd, dont_restart, timeout_ms, events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nested_borrow_panics_and_the_guard_clears_on_unwind() {
        let mut record = SocketRecord::new();
        record.fd = 7;
        let socket = unsafe { Socket::from_ptr(&raw mut record) }.unwrap();
        // A guard's handle shares the thread's flag with `from_ptr`'s.
        let other = unsafe { SocketGuard::current().socket(&raw mut record) }.unwrap();
        let nested =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| socket.read(|_| other.fd())));
        assert!(
            nested.is_err(),
            "a read inside another handle's read panics"
        );
        socket.edit(|record| record.fd = 8);
        assert_eq!(other.fd(), 8, "the guard cleared when the panic unwound");
    }
}
