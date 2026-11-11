//! [`Socket`]: a live `network::Socket`, proven so once where its pointer comes
//! from (a reactor event, a connection, an owner's field), then read and edited
//! without `unsafe`.
use crate::{SocketFields, SocketRecord};
use std::sync::atomic::{AtomicU32, Ordering};
use std::{ffi::c_int, marker::PhantomData, ptr::NonNull};

/// A live `network::Socket` for `'a`. It is `Copy` and holds no Rust reference
/// between calls, so native code and other handles may touch the same record in
/// between; [`read`](Self::read) and [`edit`](Self::edit) borrow its fields only for
/// their closure. Reads share a record, from any thread; an edit has it alone, and a
/// read or edit that would overlap one panics.
#[derive(Clone, Copy)]
pub struct Socket<'a> {
    record: NonNull<SocketRecord>,
    _live: PhantomData<(&'a mut SocketRecord, *mut ())>,
}

/// The borrow count's edit bit; below it, the reads holding the record.
const EDIT: u32 = 1 << 31;

/// A record's borrow, released when the closure returns or unwinds.
struct Borrow<'a> {
    borrows: &'a AtomicU32,
    taken: u32,
}

impl<'a> Borrow<'a> {
    #[track_caller]
    fn read(borrows: &'a AtomicU32) -> Self {
        // One locked add: a read counts itself first, and backs out if an edit holds the
        // record. (An edit only ever starts from zero, so the brief count blocks none
        // that wasn't already conflicting.)
        let borrow = Self { borrows, taken: 1 };
        // On failure the assert unwinds through `borrow`, which takes the count back.
        assert!(
            borrows.fetch_add(1, Ordering::Acquire) & EDIT == 0,
            "a socket's read runs inside an edit of the same record"
        );
        borrow
    }

    #[track_caller]
    fn edit(borrows: &'a AtomicU32) -> Self {
        let taken = borrows.compare_exchange(0, EDIT, Ordering::Acquire, Ordering::Relaxed);
        assert!(
            taken.is_ok(),
            "a socket's edit runs inside a read or edit of the same record"
        );
        Self {
            borrows,
            taken: EDIT,
        }
    }
}

impl Drop for Borrow<'_> {
    fn drop(&mut self) {
        self.borrows.fetch_sub(self.taken, Ordering::Release);
    }
}

impl<'a> Socket<'a> {
    /// The socket `record` points to, or None for null.
    /// # Safety
    /// A non-null `record` is a live `network::Socket` for the handle's lifetime, which
    /// nothing frees or reaches through a Rust reference to its fields meanwhile (native
    /// code and raw pointers may, between this handle's calls).
    #[must_use]
    pub unsafe fn from_ptr(record: *mut SocketRecord) -> Option<Self> {
        Some(Self {
            record: NonNull::new(record)?,
            _live: PhantomData,
        })
    }

    /// The record's borrow count, which no closure's reference covers.
    fn borrows(self) -> &'a AtomicU32 {
        // SAFETY: live for 'a (from_ptr); the reference covers only the count, which
        // `read` and `edit` never hand out, and an atomic may be shared.
        unsafe { &(*self.record.as_ptr()).borrows }
    }

    /// The record's address, for native calls.
    #[must_use]
    pub fn as_ptr(self) -> *mut SocketRecord {
        self.record.as_ptr()
    }

    /// `read`'s result over the record.
    /// # Panics
    /// Inside an `edit` closure over the same record.
    #[track_caller]
    pub fn read<R>(self, read: impl FnOnce(&SocketFields) -> R) -> R {
        let _borrow = Borrow::read(self.borrows());
        // SAFETY: live for the handle's lifetime (from_ptr), and no mutable Rust reference to
        // its fields exists: handles form one only in `edit`, which the count excludes.
        read(unsafe { &(*self.record.as_ptr()).fields })
    }

    /// `edit`'s result over the record, which it may change.
    /// # Panics
    /// Inside a `read` or `edit` closure over the same record.
    #[track_caller]
    pub fn edit<R>(self, edit: impl FnOnce(&mut SocketFields) -> R) -> R {
        let _borrow = Borrow::edit(self.borrows());
        // SAFETY: as in `read`; the count makes this the only reference to the fields.
        edit(unsafe { &mut (*self.record.as_ptr()).fields })
    }

    /// `fd`.
    #[must_use]
    #[track_caller]
    pub fn fd(self) -> c_int {
        self.read(|record| record.fd)
    }

    /// `Socket::catch_error(error)`. It edits only on `ENOBUFS`, the one error that sets
    /// `kernel_nobufs`, so threads that share a record (a pipe) and hit `EAGAIN` only read it.
    #[track_caller]
    pub fn catch_error(self, error: c_int) -> c_int {
        self.catch(crate::catch_error, error)
    }

    /// `Socket::catch_write_pipe_error(error)`, editing as [`Self::catch_error`] does.
    #[track_caller]
    pub fn catch_pipe_error(self, error: c_int) -> c_int {
        self.catch(crate::catch_pipe_error, error)
    }

    #[track_caller]
    fn catch(self, catch: fn(c_int, &mut bool) -> c_int, error: c_int) -> c_int {
        if error == libc::ENOBUFS {
            self.edit(|record| catch(error, &mut record.kernel_nobufs))
        } else {
            catch(error, &mut false)
        }
    }

    /// `Socket::has_kernel_nobufs`: whether a send hit `ENOBUFS` since the last call,
    /// which it clears. Only a set flag needs the edit.
    #[must_use]
    #[track_caller]
    pub fn take_kernel_nobufs(self) -> bool {
        self.read(|record| record.kernel_nobufs)
            && self.edit(|record| std::mem::take(&mut record.kernel_nobufs))
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
        assert_eq!(other.fd(), 8, "the count cleared when the edit returned");
    }

    #[test]
    fn an_edit_inside_a_read_panics_and_reads_share() {
        let mut record = SocketRecord::new();
        let socket = unsafe { Socket::from_ptr(&raw mut record) }.unwrap();
        let fd = socket.read(|outer| {
            let nested = catch_unwind(AssertUnwindSafe(|| socket.edit(|fields| fields.fd = 1)));
            assert!(
                nested.is_err(),
                "an edit inside a read of the same record panics"
            );
            socket.read(|inner| inner.fd) + outer.fd
        });
        assert_eq!(fd, 0, "reads of one record nest");
        socket.edit(|fields| fields.fd = 2);
        assert_eq!(socket.fd(), 2, "the count cleared after the reads");
    }

    #[test]
    fn threads_read_one_record_together() {
        let mut record = SocketRecord::new();
        record.fd = 5;
        let record = (&raw mut record).expose_provenance();
        // Each thread waits inside its read for the other's, so the two reads overlap.
        let both_reading = std::sync::Barrier::new(2);
        let after = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..2)
                .map(|_| {
                    let both_reading = &both_reading;
                    scope.spawn(move || {
                        let record = std::ptr::with_exposed_provenance_mut(record);
                        let socket = unsafe { Socket::from_ptr(record) }.unwrap();
                        socket.read(|fields| {
                            both_reading.wait();
                            let readers = socket.borrows().load(Ordering::Relaxed);
                            // Neither leaves its read until both have counted.
                            both_reading.wait();
                            assert_eq!(readers, 2, "the two reads hold the record at once");
                            fields.fd
                        })
                    })
                })
                .collect();
            threads
                .into_iter()
                .map(|t| t.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(after, [5, 5], "both reads saw the record");
        let socket = unsafe { Socket::from_ptr(std::ptr::with_exposed_provenance_mut(record)) };
        socket.unwrap().edit(|fields| fields.fd = 6);
    }

    #[test]
    fn error_catching_edits_only_for_enobufs() {
        let mut record = SocketRecord::new();
        let socket = unsafe { Socket::from_ptr(&raw mut record) }.unwrap();
        // Inside a read, so an edit would panic: EAGAIN and a clear flag only read.
        socket.read(|_| {
            assert_eq!(
                socket.catch_error(libc::EAGAIN),
                crate::catch_error(libc::EAGAIN, &mut false)
            );
            assert!(!socket.take_kernel_nobufs());
        });
        socket.catch_error(libc::ENOBUFS);
        assert!(socket.take_kernel_nobufs(), "ENOBUFS sets the flag");
        assert!(!socket.take_kernel_nobufs(), "taking it clears it");
    }

    #[test]
    fn the_count_clears_on_unwind_and_other_records_nest() {
        let (mut a, mut b) = (SocketRecord::new(), SocketRecord::new());
        b.fd = 9;
        let a = unsafe { Socket::from_ptr(&raw mut a) }.unwrap();
        let b = unsafe { Socket::from_ptr(&raw mut b) }.unwrap();
        assert_eq!(a.edit(|_| b.fd()), 9, "another record's read nests");
        let unwound = catch_unwind(AssertUnwindSafe(|| a.edit(|_| panic!("in the closure"))));
        assert!(unwound.is_err());
        a.edit(|fields| fields.fd = 3);
        assert_eq!(a.fd(), 3, "the count cleared when the closure unwound");
    }
}
