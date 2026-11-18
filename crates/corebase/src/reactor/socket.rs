//! [`Socket`]: a live `network::Socket`, proven so once where its pointer comes
//! from (a reactor event, a connection, an owner's field), then read and edited
//! without `unsafe`.
use crate::reactor::{SocketFields, SocketRecord};
use std::cell::Cell;
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
    /// code and raw pointers may, between this handle's calls).
    #[must_use]
    pub unsafe fn from_ptr(record: *mut SocketRecord) -> Option<Self> {
        Some(Self {
            record: NonNull::new(record)?,
            _live: PhantomData,
        })
    }

    /// The socket of a record this side holds alone for `'a` (one on the caller's
    /// frame, say). It has no [`Heap`], so no reactor registers it.
    #[must_use]
    pub fn new(record: &'a mut SocketRecord) -> Self {
        Self {
            record: NonNull::from(record),
            _live: PhantomData,
        }
    }

    /// The record's marks, which no closure's reference covers.
    fn marks(self) -> &'a [AtomicU32; 2] {
        // SAFETY: live for 'a (from_ptr); the reference covers only the marks, which
        // `read` and `edit` never hand out, and atomics may be shared.
        unsafe { &(*self.record.as_ptr()).marks }
    }

    /// The record's borrow count.
    fn borrows(self) -> &'a AtomicU32 {
        &self.marks()[crate::reactor::socket_record::BORROWS]
    }

    /// The serial of the reactor whose registry names the record, or 0.
    pub(crate) fn mark(self) -> &'a AtomicU32 {
        &self.marks()[crate::reactor::socket_record::REACTOR]
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
    pub fn catch_error(self, error: c_int) -> crate::reactor::Caught {
        self.catch(crate::reactor::catch_error, error)
    }

    /// `Socket::catch_write_pipe_error(error)`, editing as [`Self::catch_error`] does.
    #[track_caller]
    pub fn catch_pipe_error(self, error: c_int) -> crate::reactor::Caught {
        self.catch(crate::reactor::catch_pipe_error, error)
    }

    #[track_caller]
    fn catch(
        self,
        catch: fn(c_int, &mut bool) -> crate::reactor::Caught,
        error: c_int,
    ) -> crate::reactor::Caught {
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
    pub fn wait_event(self, timeout_ms: c_int, events: crate::reactor::Events) -> bool {
        let (fd, dont_restart) = self.read(|record| (record.fd, record.dont_restart));
        crate::reactor::socket_upstream::wait_event(fd, dont_restart, timeout_ms, events)
    }
}

/// A [`Socket`] a reactor may register: a heap record whose one free is
/// [`OwnedSocket`]'s, which first deletes it from the reactor that registered it, or a
/// record never freed. So a registry may name it past `'a`: no entry outlives its
/// record. Only an owner ([`OwnedSocket::heap`], [`SocketRecord::into_socket`]), a
/// worker's pipe ([`Heap::lent`]), the registry and C++, whose records `delete` frees
/// through an owner, make one; a record on a frame ([`Socket::new`]) has none.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Heap<'a>(Socket<'a>);

impl<'a> Heap<'a> {
    /// The pipe end `corebase::process` lends for `'a`, a worker's pipe whose writer vouched
    /// for it: a live heap record of this crate's, which its owner frees.
    #[must_use]
    pub fn lent(native: crate::process::worker_record::NativeSocket<'a>) -> Self {
        Self(Socket {
            record: native.as_non_null().cast(),
            _live: PhantomData,
        })
    }

    /// The boundary for holders that still keep a `Socket<'static>` alias rather than
    /// their owner (#4577): a record that stays live until an unsafe free, which goes
    /// through [`OwnedSocket::from_socket`] and so deregisters it, as `'static` states.
    #[must_use]
    pub const fn from_static(socket: Socket<'static>) -> Heap<'static> {
        Heap(socket)
    }

    /// The socket, for `'a`.
    #[must_use]
    pub const fn socket(self) -> Socket<'a> {
        self.0
    }

    /// The registry's copy, which stays valid past `'a` by the type's invariant.
    pub(crate) const fn named(self) -> Heap<'static> {
        Heap(Socket {
            record: self.0.record,
            _live: PhantomData,
        })
    }
}

impl<'a> std::ops::Deref for Heap<'a> {
    type Target = Socket<'a>;
    fn deref(&self) -> &Socket<'a> {
        &self.0
    }
}

/// A heap record and the one right to free it (`delete socket`): neither `Copy` nor
/// `Clone`, so a record is freed once, and it lends [`Socket`] only for a borrow of
/// itself. Transparent over the record's address, so a C++ `Socket *` whose owner
/// gives it up (`Socket::free`, `Reactor::close`) passes as one. Dropping one leaks the
/// record, as a C++ pointer never deleted does.
#[repr(transparent)]
pub struct OwnedSocket(Heap<'static>);

impl OwnedSocket {
    /// `record`, which this owns from here.
    #[must_use]
    pub fn new(record: Box<SocketRecord>) -> Self {
        Self(record.into_socket())
    }

    /// The owner of `socket`'s record, which its handles give up.
    /// # Safety
    /// `socket` is a heap record ([`SocketRecord::into_raw`], [`SocketRecord::into_socket`]
    /// or a call that returns one) that no other owner holds, and no handle to it is used
    /// once this owner frees it.
    #[must_use]
    pub unsafe fn from_socket(socket: Socket<'_>) -> Self {
        Self(Heap(Socket {
            record: socket.record,
            _live: PhantomData,
        }))
    }

    /// The record, for as long as this is borrowed.
    #[must_use]
    pub const fn socket(&self) -> Socket<'_> {
        self.0.0
    }

    /// The record for as long as this is borrowed, which a reactor may register: this
    /// owner's free deletes it from the reactor first.
    #[must_use]
    pub const fn heap(&self) -> Heap<'_> {
        self.0
    }

    /// The record, which nothing frees from here but an owner taken back with
    /// [`Self::from_socket`].
    #[must_use]
    pub const fn leak(self) -> Heap<'static> {
        self.0
    }

    /// The record's box, deleted first from the reactor whose registry names it: no
    /// registry outlives a socket. That reactor must be the thread's; a record freed
    /// while a reactor it can't reach still names it ends the process.
    #[must_use]
    pub fn into_box(self) -> Box<SocketRecord> {
        let socket = self.socket();
        // A `read` or `edit` closure of the record's freed it: it would use the fields
        // after the free.
        if socket.borrows().load(Ordering::Relaxed) != 0 {
            crate::fatal("a socket was freed inside a read or edit of it");
        }
        leave_reactor(socket);
        // SAFETY: the box `new` leaked, or the heap record `from_socket` took back; this
        // owner is its only one and goes here.
        unsafe { Box::from_raw(self.0.as_ptr()) }
    }

    /// `Socket::operator delete`: the record goes, its descriptor stays open.
    pub fn free(self) {
        drop(self.into_box());
    }
}

/// A cell owning at most one heap socket: it lends the record as a [`Heap`] for a
/// borrow of itself, and the unsafe [`Self::take`] gives the owner back. Its field is a
/// cell, so a record that threads share under their own synchronization (a server's
/// connection slot) may hold one. Lending and taking both go through `&self`, so the
/// types can't tell when the last lent handle is gone: only `take`'s caller can.
/// Dropping it, [`Self::forget`] or a second [`Self::put`] leaks the record, as dropping
/// an [`OwnedSocket`] does; all zero bytes are an empty cell.
#[derive(Default)]
#[repr(transparent)]
pub struct OwnerCell(Cell<Option<Heap<'static>>>);

impl OwnerCell {
    /// The record it owns, if any.
    #[must_use]
    #[inline]
    pub fn get(&self) -> Option<Heap<'_>> {
        self.0.get()
    }

    /// Owns `owner` from here. A record it held before leaks: handles it lent may live.
    pub fn put(&self, owner: OwnedSocket) {
        self.0.set(Some(owner.leak()));
    }

    /// Names no record from here; one it held leaks, as in [`Self::put`].
    pub fn forget(&self) {
        self.0.set(None);
    }

    /// The owner it held, which it no longer names.
    /// # Safety
    /// No [`Heap`] or [`Socket`] that [`Self::get`] lent (or a copy of one) is used
    /// once the returned owner frees the record.
    #[must_use]
    pub unsafe fn take(&self) -> Option<OwnedSocket> {
        self.0.take().map(OwnedSocket)
    }
}

/// A heap record its holder owns on one thread and lends counted: a loan goes only to
/// a closure, and a record released while a loan is out is freed when the last loan
/// ends. A loan a Zend bailout skips leaves the count up, so the record leaks rather
/// than being freed early; a loan held across a coroutine yield delays the free.
#[derive(Default)]
pub struct SocketLoans {
    socket: Cell<Option<Heap<'static>>>,
    /// The loans out, with [`PENDING`] set while `retired` holds any: the last loan's
    /// end compares this one word.
    loans: Cell<u32>,
    /// Records released while lent, with how each is freed.
    retired: Cell<Vec<Retired>>,
}

/// A record released while lent, and how it is freed once the loans end.
type Retired = (OwnedSocket, fn(OwnedSocket));

/// [`SocketLoans::loans`]' flag for released records waiting on the loans. Each loan is
/// a frame on this thread's stack, so the count never reaches it.
const PENDING: u32 = 1 << 31;

impl SocketLoans {
    /// Owns `owner` from here; the record it held is released with `free`.
    pub fn put(&self, owner: OwnedSocket, free: fn(OwnedSocket)) {
        if let Some(old) = self.socket.replace(Some(owner.leak())) {
            self.retire(OwnedSocket(old), free);
        }
    }

    /// Releases the record it holds with `free`, now or when the last loan ends:
    /// whether it held one.
    pub fn release(&self, free: fn(OwnedSocket)) -> bool {
        self.socket
            .take()
            .map(|old| self.retire(OwnedSocket(old), free))
            .is_some()
    }

    /// `f` over the record it holds, which stays live until `f` returns, even if `f`
    /// releases it; `None` without one.
    #[inline]
    pub fn lend<T>(&self, f: impl FnOnce(Heap<'_>) -> T) -> Option<T> {
        struct Loan<'a>(&'a SocketLoans);
        impl Drop for Loan<'_> {
            #[inline]
            fn drop(&mut self) {
                let loans = self.0.loans.get() - 1;
                self.0.loans.set(loans);
                if loans == PENDING {
                    self.0.free_retired();
                }
            }
        }
        let socket = self.socket.get()?;
        self.loans.set(self.loans.get() + 1);
        let _loan = Loan(self);
        Some(f(socket))
    }

    /// [`Socket::read`] of the record it holds; `None` without one. Not a counted loan:
    /// the read's own borrow keeps the record, which is not freed while it runs.
    #[inline]
    pub fn read<T>(&self, read: impl FnOnce(&SocketFields) -> T) -> Option<T> {
        Some(self.socket.get()?.read(read))
    }

    /// [`Socket::edit`] of the record it holds; `None` without one. Not counted, as
    /// [`Self::read`].
    #[inline]
    #[track_caller]
    pub fn edit<T>(&self, edit: impl FnOnce(&mut SocketFields) -> T) -> Option<T> {
        Some(self.socket.get()?.edit(edit))
    }

    /// The address of the record it holds, for native code; null without one.
    #[must_use]
    pub fn as_ptr(&self) -> *mut SocketRecord {
        self.socket
            .get()
            .map_or(std::ptr::null_mut(), |socket| socket.as_ptr())
    }

    fn retire(&self, owner: OwnedSocket, free: fn(OwnedSocket)) {
        let loans = self.loans.get();
        if loans == 0 {
            free(owner);
        } else {
            let mut retired = self.retired.take();
            retired.push((owner, free));
            self.retired.set(retired);
            self.loans.set(loans | PENDING);
        }
    }

    /// The last loan ended with records released meanwhile: free them.
    #[cold]
    #[inline(never)]
    fn free_retired(&self) {
        self.loans.set(0);
        for (owner, free) in self.retired.take() {
            free(owner);
        }
    }
}

/// The sockets a holder owns until it is given up by value: it lends the current one
/// for a borrow of itself, and [`Self::retire`] takes that one off its reactor for its
/// descriptor to close but keeps the record. Nothing frees a record through `&self`, so
/// no handle it lent outlives its record; the owners leave only through
/// [`Self::take_all`], which needs the holder alone. Dropping it leaks the records, as
/// dropping an [`OwnedSocket`] does.
#[derive(Default)]
pub struct KeptSockets {
    current: Cell<Option<Heap<'static>>>,
    owners: Cell<Vec<OwnedSocket>>,
}

impl KeptSockets {
    /// The current socket, if any.
    #[must_use]
    #[inline]
    pub fn get(&self) -> Option<Heap<'_>> {
        self.current.get()
    }

    /// Keeps `owner`, which is the current socket from here. One current before stays
    /// kept as it is, no longer lent.
    pub fn put(&self, owner: OwnedSocket) {
        self.current.set(Some(owner.0));
        let mut owners = self.owners.take();
        owners.push(owner);
        self.owners.set(owners);
    }

    /// The current socket, which no longer is: deleted from the reactor whose registry
    /// names it, as a free deletes it first, and kept for the rest of this holder's life.
    /// The caller closes its descriptor.
    pub fn retire(&self) -> Option<Socket<'_>> {
        let socket = self.current.take()?.socket();
        leave_reactor(socket);
        Some(socket)
    }

    /// Every record this kept, current or retired, for the caller to free: holding this
    /// alone, it has no lend left.
    #[must_use]
    pub fn take_all(&mut self) -> Vec<OwnedSocket> {
        self.current.set(None);
        std::mem::take(self.owners.get_mut())
    }
}

/// Deletes `socket` from the reactor whose registry names it, if one does.
fn leave_reactor(socket: Socket<'_>) {
    let mark = socket.mark().load(Ordering::Relaxed);
    if mark != 0 {
        deregister(socket, mark);
    }
}

/// Deletes `socket` from the thread's reactor, whose serial is `mark`, as it is freed:
/// from the kernel too while its descriptor is still the one registered, quietly, since
/// it may be closed already; then from the registry wherever it names the record.
fn deregister(socket: Socket<'_>, mark: u32) {
    let Some(reactor) =
        crate::reactor::event_api::running().filter(|reactor| reactor.serial() == mark)
    else {
        crate::fatal("a socket was freed while registered with a reactor other than this thread's");
    };
    let (fd, removed) = socket.read(|record| (record.fd, record.removed));
    let registered = reactor.registry(|registry| registry.get(fd));
    if !removed && registered.is_some_and(|entry| entry.as_ptr() == socket.as_ptr()) {
        socket.edit(|record| record.silent_remove = true);
        _ = crate::reactor::reactor_upstream::reactor_del(reactor, socket);
    }
    if socket.mark().load(Ordering::Relaxed) != 0 {
        reactor.with(|state| state.forget(socket));
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
    fn kept_sockets_keep_a_retired_record_live_until_taken_by_value() {
        let mut kept = KeptSockets::default();
        assert!(kept.get().is_none(), "none at first");
        let mut record = SocketRecord::new();
        record.fd = -1;
        kept.put(OwnedSocket::new(Box::new(record)));
        let lent = kept.get().expect("the current socket");
        let retired = kept.retire().expect("the current socket retires");
        assert!(kept.get().is_none(), "no current socket once retired");
        assert!(kept.retire().is_none(), "nothing more to retire");
        // Both handles still reach the record, which the holder keeps.
        retired.edit(|record| record.fd = 3);
        assert_eq!(lent.fd(), 3);
        kept.put(OwnedSocket::new(Box::default()));
        let owners = kept.take_all();
        assert_eq!(owners.len(), 2, "the retired record and the current one");
        assert!(kept.get().is_none(), "nothing current once taken");
        for owner in owners {
            owner.free();
        }
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
                crate::reactor::catch_error(libc::EAGAIN, &mut false)
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
