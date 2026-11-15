//! [`Socket`]: a live `network::Socket`, proven so once where its pointer comes
//! from (a reactor event, a connection, an owner's field), then read and edited
//! without `unsafe`.
use crate::{SocketFields, SocketRecord};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use std::{ffi::c_int, marker::PhantomData, ptr::NonNull};

/// A live `network::Socket` for `'a`. It is `Copy` and holds no Rust reference
/// between calls, so native code and other handles may touch the same record in
/// between; [`read`](Self::read) and [`edit`](Self::edit) borrow its fields only for
/// their closure. Reads share a record, from any thread; an edit has it alone. A read
/// or edit that would overlap another thread's waits for it to end; one that overlaps
/// a borrow still held after a second, as a nested one on its own thread is, panics.
/// Transparent over the record's address, so an export can take one for a C++
/// `Socket *` (`this` included) that is live for the call.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Socket<'a> {
    record: NonNull<SocketRecord>,
    _live: PhantomData<(&'a mut SocketRecord, *mut ())>,
}

/// The borrow count's edit bit; below it, the reads holding the record.
const EDIT: u32 = 1 << 31;

/// How long a borrow waits for an overlapping one to end. Another thread's closure
/// ends within it; a borrow nested on the waiting thread never does, and panics.
/// (Under Miri, whose threads run slowly in real time, a test's wait is longer.)
const PATIENCE: Duration = if cfg!(miri) {
    Duration::from_secs(1)
} else if cfg!(test) {
    Duration::from_millis(10)
} else {
    Duration::from_secs(1)
};

/// A record's borrow, released when the closure returns or unwinds.
struct Borrow<'a> {
    borrows: &'a AtomicU32,
    taken: u32,
}

impl<'a> Borrow<'a> {
    /// One locked add: a read counts itself first, and backs out and waits if an edit
    /// holds the record. (An edit only ever starts from zero, so the brief count blocks none
    /// that wasn't already conflicting.) Inlined: every record access takes one.
    #[inline(always)]
    #[track_caller]
    fn read(borrows: &'a AtomicU32) -> Self {
        if borrows.fetch_add(1, Ordering::Acquire) & EDIT != 0 {
            Self::read_contended(borrows);
        }
        Self { borrows, taken: 1 }
    }

    /// `read` once its first add met an edit: backs out, waits and counts itself again.
    #[cold]
    #[inline(never)]
    #[track_caller]
    fn read_contended(borrows: &AtomicU32) {
        let mut since = None;
        loop {
            borrows.fetch_sub(1, Ordering::Relaxed);
            Self::wait(
                borrows,
                EDIT,
                &mut since,
                "a socket's read overlaps an edit of the same record",
            );
            if borrows.fetch_add(1, Ordering::Acquire) & EDIT == 0 {
                return;
            }
        }
    }

    #[inline(always)]
    #[track_caller]
    fn edit(borrows: &'a AtomicU32) -> Self {
        if borrows
            .compare_exchange(0, EDIT, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            Self::edit_contended(borrows);
        }
        Self {
            borrows,
            taken: EDIT,
        }
    }

    /// `edit` once the record was held: waits until it is free and takes it.
    #[cold]
    #[inline(never)]
    #[track_caller]
    fn edit_contended(borrows: &AtomicU32) {
        let mut since = None;
        loop {
            Self::wait(
                borrows,
                u32::MAX,
                &mut since,
                "a socket's edit overlaps a read or edit of the same record",
            );
            if borrows
                .compare_exchange(0, EDIT, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }

    /// Waits until no bit of `held` is set in the count; panics with `overlap` once it has
    /// waited [`PATIENCE`] since the first wait of this borrow.
    #[cold]
    #[track_caller]
    fn wait(borrows: &AtomicU32, held: u32, since: &mut Option<Instant>, overlap: &str) {
        #[cfg(test)]
        let _waiting = Waiting::enter(borrows);
        let since = *since.get_or_insert_with(Instant::now);
        while borrows.load(Ordering::Relaxed) & held != 0 {
            assert!(since.elapsed() < PATIENCE, "{overlap}");
            std::thread::yield_now();
        }
    }
}

impl Drop for Borrow<'_> {
    fn drop(&mut self) {
        self.borrows.fetch_sub(self.taken, Ordering::Release);
    }
}

/// Test-only: the records a borrow is waiting on, by address, so a test can hold an
/// edit until another thread's borrow is provably waiting for it.
#[cfg(test)]
static WAITING: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());

/// Test-only: a borrow's entry in [`WAITING`] while it waits.
#[cfg(test)]
struct Waiting(usize);

#[cfg(test)]
impl Waiting {
    fn waiting() -> std::sync::MutexGuard<'static, Vec<usize>> {
        WAITING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn enter(borrows: &AtomicU32) -> Self {
        let at = std::ptr::from_ref(borrows).addr();
        Self::waiting().push(at);
        Self(at)
    }

    fn on(borrows: &AtomicU32) -> bool {
        Self::waiting().contains(&std::ptr::from_ref(borrows).addr())
    }
}

#[cfg(test)]
impl Drop for Waiting {
    fn drop(&mut self) {
        let mut waiting = Self::waiting();
        if let Some(index) = waiting.iter().position(|&at| at == self.0) {
            waiting.swap_remove(index);
        }
    }
}

impl<'a> Socket<'a> {
    /// The socket `record` points to, or None for null.
    /// # Safety
    /// A non-null `record` is a live `network::Socket` for the handle's lifetime, which
    /// nothing frees or reaches through a Rust reference to its fields meanwhile (native
    /// code and raw pointers may, between this handle's calls). Its `ssl` is null or a
    /// live `SSL` it owns, as [`SocketFields::set_ssl`] requires.
    #[must_use]
    pub unsafe fn from_ptr(record: *mut SocketRecord) -> Option<Self> {
        Some(Self {
            record: NonNull::new(record)?,
            _live: PhantomData,
        })
    }

    /// The socket of a record this side holds alone for `'a` (one on the caller's
    /// frame, say). Its `ssl` is null or owned, which only the unsafe
    /// [`SocketFields::set_ssl`] can change.
    #[must_use]
    pub fn new(record: &'a mut SocketRecord) -> Self {
        Self {
            record: NonNull::from(record),
            _live: PhantomData,
        }
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
    /// Inside an `edit` closure over the same record, after waiting a second for it to end.
    #[inline(always)]
    #[track_caller]
    pub fn read<R>(self, read: impl FnOnce(&SocketFields) -> R) -> R {
        let _borrow = Borrow::read(self.borrows());
        // SAFETY: live for the handle's lifetime (from_ptr), and no mutable Rust reference to
        // its fields exists: handles form one only in `edit`, which the count excludes.
        read(unsafe { &(*self.record.as_ptr()).fields })
    }

    /// `edit`'s result over the record, which it may change.
    /// # Panics
    /// Inside a `read` or `edit` closure over the same record, after waiting a second for it to end.
    #[inline(always)]
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
    pub fn catch_error(self, error: c_int) -> crate::Caught {
        self.catch(crate::catch_error, error)
    }

    /// `Socket::catch_write_pipe_error(error)`, editing as [`Self::catch_error`] does.
    #[track_caller]
    pub fn catch_pipe_error(self, error: c_int) -> crate::Caught {
        self.catch(crate::catch_pipe_error, error)
    }

    #[track_caller]
    fn catch(self, catch: fn(c_int, &mut bool) -> crate::Caught, error: c_int) -> crate::Caught {
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

    /// `Socket::wait_event`: whether an event came. It waits outside any borrow, since the
    /// wait runs signal handlers that may reach other sockets.
    #[must_use]
    pub fn wait_event(self, timeout_ms: c_int, events: crate::Events) -> bool {
        let (fd, dont_restart) = self.read(|record| (record.fd, record.dont_restart));
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
    fn a_read_from_another_thread_waits_for_an_edit() {
        let mut record = SocketRecord::new();
        let record = (&raw mut record).expose_provenance();
        let handle = || unsafe { Socket::from_ptr(std::ptr::with_exposed_provenance_mut(record)) };
        let editing = std::sync::Barrier::new(2);
        let read = std::thread::scope(|scope| {
            scope.spawn(|| {
                let socket = handle().unwrap();
                socket.edit(|fields| {
                    editing.wait();
                    // Hold the edit until B's read is provably waiting on it; under the
                    // old immediate panic B would never wait, and this would time out.
                    let began = std::time::Instant::now();
                    while !Waiting::on(socket.borrows()) {
                        assert!(began.elapsed() < Duration::from_secs(10), "B never waited");
                        std::thread::yield_now();
                    }
                    fields.fd = 4;
                });
            });
            scope
                .spawn(|| {
                    editing.wait();
                    handle().unwrap().fd()
                })
                .join()
                .unwrap()
        });
        assert_eq!(read, 4, "the read ran after the edit, and saw its write");
        assert_eq!(
            handle().unwrap().borrows().load(Ordering::Relaxed),
            0,
            "nothing left counted"
        );
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
