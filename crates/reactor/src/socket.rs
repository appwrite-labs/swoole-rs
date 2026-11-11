//! [`Socket`]: a live `network::Socket`, proven so once where its pointer comes
//! from (a reactor event, a connection, an owner's field), then read and edited
//! without `unsafe`.
use crate::{SocketFields, SocketRecord};
use std::{cell::Cell, ffi::c_int, marker::PhantomData, ptr::NonNull};

/// A live `network::Socket` for `'a`, on this thread. It is `Copy` and holds no
/// Rust reference between calls, so native code and other handles may touch the
/// same record in between; [`read`](Self::read) and [`edit`](Self::edit) borrow its
/// fields only for their closure, and refuse to nest on the same record.
#[derive(Clone, Copy)]
pub struct Socket<'a> {
    record: NonNull<SocketRecord>,
    _live: PhantomData<(&'a mut SocketRecord, *mut ())>,
}

/// Holds a record's borrow flag, and clears it when the closure returns or unwinds.
struct Borrow<'a>(&'a Cell<bool>);

impl<'a> Borrow<'a> {
    fn take(flag: &'a Cell<bool>) -> Self {
        assert!(
            !flag.replace(true),
            "a socket's read or edit runs inside another's on the same record"
        );
        Self(flag)
    }
}

impl Drop for Borrow<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl<'a> Socket<'a> {
    /// The socket `record` points to, or None for null.
    /// # Safety
    /// A non-null `record` is a live `network::Socket` for the handle's lifetime, used on this
    /// thread, which nothing frees or reaches through a Rust reference to its fields meanwhile
    /// (native code and raw pointers may, between this handle's calls).
    #[must_use]
    pub unsafe fn from_ptr(record: *mut SocketRecord) -> Option<Self> {
        Some(Self {
            record: NonNull::new(record)?,
            _live: PhantomData,
        })
    }

    /// The record's borrow flag, which no closure's reference covers.
    fn flag(self) -> &'a Cell<bool> {
        // SAFETY: live for 'a (from_ptr); the reference covers only the flag, which
        // `read` and `edit` never hand out, and a `Cell` may be shared.
        unsafe { &(*self.record.as_ptr()).borrowed }
    }

    /// The record's address, for native calls.
    #[must_use]
    pub fn as_ptr(self) -> *mut SocketRecord {
        self.record.as_ptr()
    }

    /// `read`'s result over the record.
    /// # Panics
    /// Inside a `read` or `edit` closure over the same record.
    pub fn read<R>(self, read: impl FnOnce(&SocketFields) -> R) -> R {
        let _borrow = Borrow::take(self.flag());
        // SAFETY: live for the handle's lifetime (from_ptr), and no other Rust reference to its
        // fields exists: handles form one only here and in `edit`, under the record's flag.
        read(unsafe { &(*self.record.as_ptr()).fields })
    }

    /// `edit`'s result over the record, which it may change.
    /// # Panics
    /// Inside a `read` or `edit` closure over the same record.
    pub fn edit<R>(self, edit: impl FnOnce(&mut SocketFields) -> R) -> R {
        let _borrow = Borrow::take(self.flag());
        // SAFETY: as in `read`; this is the only reference to the fields while `edit` runs.
        edit(unsafe { &mut (*self.record.as_ptr()).fields })
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
    use std::panic::{AssertUnwindSafe, catch_unwind};

    #[test]
    fn a_nested_read_inside_edit_panics_and_the_edit_goes_on() {
        let mut record = SocketRecord::new();
        record.fd = 7;
        let socket = unsafe { Socket::from_ptr(&raw mut record) }.unwrap();
        let other = unsafe { Socket::from_ptr(&raw mut record) }.unwrap();
        socket.edit(|fields| {
            let nested = catch_unwind(AssertUnwindSafe(|| other.read(|fields| fields.fd)));
            assert!(
                nested.is_err(),
                "a read inside an edit of the same record panics"
            );
            // The edit's reference stays valid after the nested call unwound.
            fields.fd = 8;
        });
        assert_eq!(other.fd(), 8, "the flag cleared when the edit returned");
    }

    #[test]
    fn the_flag_clears_on_unwind_and_other_records_nest() {
        let (mut a, mut b) = (SocketRecord::new(), SocketRecord::new());
        b.fd = 9;
        let a = unsafe { Socket::from_ptr(&raw mut a) }.unwrap();
        let b = unsafe { Socket::from_ptr(&raw mut b) }.unwrap();
        assert_eq!(a.read(|_| b.fd()), 9, "another record's read nests");
        let unwound = catch_unwind(AssertUnwindSafe(|| a.edit(|_| panic!("in the closure"))));
        assert!(unwound.is_err());
        a.edit(|fields| fields.fd = 3);
        assert_eq!(a.fd(), 3, "the flag cleared when the closure unwound");
    }
}
