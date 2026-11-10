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
    _live: PhantomData<(&'a mut SocketRecord, *mut ())>,
}

thread_local! {
    /// Whether a `read` or `edit` closure is running on this thread.
    static BORROWED: Cell<bool> = const { Cell::new(false) };
}

/// `body`'s result with [`BORROWED`] set, which it clears when `body` returns or unwinds.
/// One thread-local lookup a call: in the extension, each is a `__tls_get_addr` (#2808).
fn borrowed<R>(body: impl FnOnce() -> R) -> R {
    /// Clears the flag on return or unwind.
    struct Clear<'a>(&'a Cell<bool>);
    impl Drop for Clear<'_> {
        fn drop(&mut self) {
            self.0.set(false);
        }
    }
    BORROWED.with(|flag| {
        assert!(
            !flag.replace(true),
            "a socket's read or edit runs inside another's"
        );
        let _clear = Clear(flag);
        body()
    })
}

impl Socket<'_> {
    /// The socket `record` points to, or None for null.
    /// # Safety
    /// A non-null `record` is a live `network::Socket` for the handle's lifetime, used on this
    /// thread, which nothing frees or reaches through a Rust reference meanwhile
    /// (native code and raw pointers may, between this handle's calls).
    #[must_use]
    pub unsafe fn from_ptr(record: *mut SocketRecord) -> Option<Self> {
        Some(Self {
            record: NonNull::new(record)?,
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
        // SAFETY: live for the handle's lifetime (from_ptr), and no other Rust reference to it exists:
        // handles form one only here and in `edit`, which cannot nest.
        borrowed(|| read(unsafe { self.record.as_ref() }))
    }

    /// `edit`'s result over the record, which it may change.
    /// # Panics
    /// Inside another socket's `read` or `edit` closure.
    pub fn edit<R>(self, edit: impl FnOnce(&mut SocketRecord) -> R) -> R {
        let mut record = self.record;
        // SAFETY: as in `read`; this is the only reference while `edit` runs.
        borrowed(|| edit(unsafe { record.as_mut() }))
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
