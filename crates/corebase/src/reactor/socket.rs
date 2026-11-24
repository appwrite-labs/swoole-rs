//! [`Socket`]: a live `network::Socket`, proven so once where its pointer comes
//! from (a reactor event, a connection, an owner's field), then read and edited
//! without `unsafe`.
use crate::reactor::{SocketFields, SocketRecord};
use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::{ffi::c_int, marker::PhantomData, ptr::NonNull};

/// A live `network::Socket` for `'a`. It is `Copy` and holds no Rust reference
/// between calls, so native code and other handles may touch the same record in
/// between; [`read`](Self::read) and [`edit`](Self::edit) borrow its fields only for
/// their closure. Reads share a record, from any thread; an edit has it alone. A read
/// or edit that conflicts with another thread waits. A conflict on this thread panics.
/// Transparent over the record's address, so an export can take one for a C++
/// `Socket *` (`this` included) that is live for the call.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Socket<'a> {
    record: NonNull<SocketRecord>,
    _live: PhantomData<(&'a mut SocketRecord, *mut ())>,
}

const EDIT: usize = 1;
const SHARED: usize = 2;

/// The reactor registration and active borrows, outside the borrowed socket fields.
#[derive(Default, Debug)]
pub(super) struct SocketMarks {
    pub(super) reactor: AtomicU32,
    // Primary thread address, tagged with EDIT for an exclusive borrow. SHARED
    // keeps the record occupied as long as any secondary reader remains.
    owner: AtomicUsize,
    readers: Mutex<Vec<usize>>,
}

struct Borrow<'a, const MODE: usize> {
    state: &'a SocketMarks,
    secondary: bool,
    _thread: PhantomData<*mut ()>,
}

impl SocketMarks {
    #[inline(always)]
    #[track_caller]
    fn take<const MODE: usize>(&self) -> Borrow<'_, MODE> {
        let thread = crate::sys::thread_token();
        let token = thread | MODE;
        let secondary = self
            .owner
            .compare_exchange(0, token, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
            && self.contended(token);
        Borrow {
            state: self,
            secondary,
            _thread: PhantomData,
        }
    }

    fn readers(&self) -> std::sync::MutexGuard<'_, Vec<usize>> {
        self.readers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Take an idle record or join its primary reader. Only secondary readers
    /// register in the list, where nested reads have one entry per borrow.
    #[cold]
    #[track_caller]
    fn contended(&self, token: usize) -> bool {
        #[cfg(test)]
        let _waiting = Waiting::enter(self);
        let thread = token & !EDIT;
        loop {
            let owner = self.owner.load(Ordering::Acquire);
            if owner == 0 {
                if self
                    .owner
                    .compare_exchange(0, token, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return false;
                }
                continue;
            }
            if owner & EDIT != 0 {
                assert!(
                    owner & !EDIT != thread,
                    "a socket borrow overlaps an edit of the same record"
                );
            } else {
                let mut readers = self.readers();
                if self.owner.load(Ordering::Acquire) != owner {
                    continue;
                }
                if token & EDIT == 0 {
                    // The primary can leave without the mutex. Pin the record
                    // before sharing it with the first secondary reader.
                    if owner & SHARED == 0
                        && self
                            .owner
                            .compare_exchange(
                                owner,
                                owner | SHARED,
                                Ordering::AcqRel,
                                Ordering::Relaxed,
                            )
                            .is_err()
                    {
                        continue;
                    }
                    readers.push(thread);
                    return true;
                }
                let overlaps = owner & !SHARED == thread || readers.contains(&thread);
                drop(readers);
                assert!(
                    !overlaps,
                    "a socket's edit overlaps a read of the same record"
                );
            }
            std::thread::yield_now();
        }
    }

    #[cold]
    fn release_shared(&self) {
        let token = crate::sys::thread_token();
        let mut readers = self.readers();
        let Some(index) = readers.iter().rposition(|&reader| reader == token) else {
            unreachable!("a shared socket reader is registered")
        };
        readers.swap_remove(index);
        if readers.is_empty() {
            self.owner.fetch_sub(SHARED, Ordering::Release);
        }
    }
}

impl<const MODE: usize> Drop for Borrow<'_, MODE> {
    #[inline(always)]
    fn drop(&mut self) {
        if MODE == EDIT {
            self.state.owner.store(0, Ordering::Release);
        } else if self.secondary {
            self.state.release_shared();
        } else {
            self.state.owner.fetch_and(SHARED, Ordering::Release);
        }
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

    fn enter(borrows: &SocketMarks) -> Self {
        let at = std::ptr::from_ref(borrows).addr();
        Self::waiting().push(at);
        Self(at)
    }

    fn on(borrows: &SocketMarks) -> bool {
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
    fn marks(self) -> &'a SocketMarks {
        // SAFETY: live for 'a (from_ptr); the reference covers only the marks, which
        // `read` and `edit` never hand out, and atomics may be shared.
        unsafe { &(*self.record.as_ptr()).marks }
    }

    /// The serial of the reactor whose registry names the record, or 0.
    pub(crate) fn mark(self) -> &'a AtomicU32 {
        &self.marks().reactor
    }

    /// The record's address, for native calls.
    #[must_use]
    pub fn as_ptr(self) -> *mut SocketRecord {
        self.record.as_ptr()
    }

    /// `read`'s result over the record.
    /// # Panics
    /// Inside an `edit` closure over the same record on this thread.
    #[inline(always)]
    #[track_caller]
    pub fn read<R>(self, read: impl FnOnce(&SocketFields) -> R) -> R {
        let _borrow = self.marks().take::<0>();
        // SAFETY: live for the handle's lifetime (from_ptr), and no mutable Rust reference to
        // its fields exists: handles form one only in `edit`, which the read guard excludes.
        read(unsafe { &(*self.record.as_ptr()).fields })
    }

    /// `edit`'s result over the record, which it may change.
    /// # Panics
    /// Inside a `read` or `edit` closure over the same record on this thread.
    #[inline(always)]
    #[track_caller]
    pub fn edit<R>(self, edit: impl FnOnce(&mut SocketFields) -> R) -> R {
        let _borrow = self.marks().take::<EDIT>();
        // SAFETY: as in `read`; the edit guard makes this the only reference to the fields.
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
        self.catch(
            SocketFields::catch_error,
            crate::reactor::catch_error,
            error,
        )
    }

    /// `Socket::catch_write_pipe_error(error)`, editing as [`Self::catch_error`] does.
    #[track_caller]
    pub fn catch_pipe_error(self, error: c_int) -> crate::reactor::Caught {
        self.catch(
            SocketFields::catch_pipe_error,
            crate::reactor::catch_pipe_error,
            error,
        )
    }

    /// `on_record` over the record on `ENOBUFS`; otherwise `catch`, which sets nothing.
    #[track_caller]
    fn catch(
        self,
        on_record: fn(&mut SocketFields, c_int) -> crate::reactor::Caught,
        catch: fn(c_int, &mut bool) -> crate::reactor::Caught,
        error: c_int,
    ) -> crate::reactor::Caught {
        if error == libc::ENOBUFS {
            self.edit(|record| on_record(record, error))
        } else {
            catch(error, &mut false)
        }
    }

    /// `Socket::has_kernel_nobufs`: whether a send hit `ENOBUFS` since the last call,
    /// which it clears. Only a set flag needs the edit.
    #[must_use]
    #[track_caller]
    pub fn take_kernel_nobufs(self) -> bool {
        self.read(SocketFields::kernel_nobufs_pending) && self.edit(SocketFields::has_kernel_nobufs)
    }

    /// `Socket::wait_event`: whether an event came. It waits outside any borrow, since the
    /// wait runs signal handlers that may reach other sockets.
    #[must_use]
    pub fn wait_event(self, timeout_ms: c_int, events: crate::reactor::Events) -> bool {
        self.read(SocketFields::io).wait(timeout_ms, events).is_ok()
    }
}

/// A [`Socket`] a reactor may register: a heap record whose one free is
/// [`OwnedSocket`]'s, which first deletes it from the reactor that registered it, or a
/// record never freed. So a registry may name it past `'a`: no entry outlives its
/// record. Only an owner ([`OwnedSocket::heap`]), a worker's pipe ([`Heap::lent`]), the
/// registry and C++, whose records `delete` frees through an owner, make one, and tests'
/// fixtures (feature `test-records`); a record on a frame ([`Socket::new`]) has none.
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

    /// A test fixture's record with no owner: live until the test frees it, through
    /// [`OwnedSocket::from_socket`] if a reactor registered it. Every production holder
    /// owns its [`OwnedSocket`] (#4577), so only tests (feature `test-records`) make one.
    #[cfg(any(test, feature = "test-records"))]
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
/// gives it up (`Socket::free`, `Reactor::close`) passes as one. Dropping it deletes the
/// record. Its descriptor has a separate owner.
#[repr(transparent)]
pub struct OwnedSocket(Heap<'static>);

impl OwnedSocket {
    /// `record`, which this owns from here.
    #[must_use]
    pub fn new(record: Box<SocketRecord>) -> Self {
        Self(Heap(Socket::new(Box::leak(record))))
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
    pub fn leak(self) -> Heap<'static> {
        core::mem::ManuallyDrop::new(self).0
    }

    /// The record's box, deleted first from the reactor whose registry names it: no
    /// registry outlives a socket. That reactor must be the thread's; a record freed
    /// while a reactor it can't reach still names it ends the process.
    #[must_use]
    pub fn into_box(self) -> Box<SocketRecord> {
        let heap = self.leak();
        let socket = heap.socket();
        // A `read` or `edit` closure of the record's freed it: it would use the fields
        // after the free.
        if socket.marks().owner.load(Ordering::Relaxed) != 0 {
            crate::fatal("a socket was freed inside a read or edit of it");
        }
        leave_reactor(socket);
        // SAFETY: the box `new` leaked, or the heap record `from_socket` took back; this
        // owner is its only one and goes here.
        unsafe { Box::from_raw(heap.as_ptr()) }
    }

    /// `Socket::operator delete`: the record goes, its descriptor stays open.
    pub fn free(self) {
        drop(self.into_box());
    }
}

impl Drop for OwnedSocket {
    fn drop(&mut self) {
        // Transfer this final owner's reclamation right to into_box.
        drop(Self(self.0).into_box());
    }
}

/// A heap record its holder owns on one thread and lends counted: a loan goes only to
/// a closure, and a record released while a loan is out is freed when the last loan
/// ends. A loan a Zend bailout skips leaves the count up, so the record leaks rather
/// than being freed early; a loan held across a coroutine yield delays the free.
pub struct SocketLoans {
    on_drop: fn(OwnedSocket),
    socket: Cell<Option<Heap<'static>>>,
    /// The loans out, with [`PENDING`] set while `retired` holds any: the last loan's
    /// end compares this one word.
    loans: Cell<u32>,
    /// Records released while lent, with how each is freed.
    retired: Cell<Vec<Retired>>,
}

/// A record released while lent, and how it is freed once the loans end.
type Retired = (Heap<'static>, fn(OwnedSocket));

/// [`SocketLoans::loans`]' flag for released records waiting on the loans. Each loan is
/// a frame on this thread's stack, so the count never reaches it.
const PENDING: u32 = 1 << 31;

impl Drop for SocketLoans {
    fn drop(&mut self) {
        if self.loans.get() == 0 {
            self.release(self.on_drop);
        }
    }
}

impl SocketLoans {
    /// An empty holder. On drop, release its current record when every loan has returned.
    #[must_use]
    pub const fn new(on_drop: fn(OwnedSocket)) -> Self {
        Self {
            on_drop,
            socket: Cell::new(None),
            loans: Cell::new(0),
            retired: Cell::new(Vec::new()),
        }
    }

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
            // Only the last loan may reclaim this record. A skipped loan
            // leaves the handle here even if the holder itself is discarded.
            retired.push((owner.leak(), free));
            self.retired.set(retired);
            self.loans.set(loans | PENDING);
        }
    }

    /// The last loan ended with records released meanwhile: free them.
    #[cold]
    #[inline(never)]
    fn free_retired(&self) {
        self.loans.set(0);
        for (heap, free) in self.retired.take() {
            free(OwnedSocket(heap));
        }
    }
}

/// The sockets a holder owns until it is given up by value: it lends the current one
/// for a borrow of itself, and [`Self::retire`] takes that one off its reactor for its
/// descriptor to close but keeps the record. Nothing frees a record through `&self`, so
/// no handle it lent outlives its record; the owners leave only through
/// [`Self::take_all`], which needs the holder alone. Its owners otherwise drop with it.
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

/// Deletes `socket` from the thread's reactor (or the one `swoole_event_free` is
/// deleting, which the thread no longer names), whose serial is `mark`, as it is freed:
/// from the kernel too while its descriptor is still the one registered, quietly, since
/// it may be closed already; then from the registry wherever it names the record.
fn deregister(socket: Socket<'_>, mark: u32) {
    let reactor = crate::reactor::event_api::running().or_else(crate::reactor::deleting);
    let Some(reactor) = reactor.filter(|reactor| reactor.serial() == mark) else {
        crate::fatal("a socket was freed while registered with a reactor other than this thread's");
    };
    let (key, removed) = socket.read(|record| (record.registry_key(), record.removed));
    let registered = reactor.registry(|registry| registry.entry(key));
    if !removed && registered.is_some_and(|entry| entry.as_ptr() == socket.as_ptr()) {
        socket.edit(SocketFields::quiet_next_removal);
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
    use std::time::Duration;

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
    fn an_inner_records_borrow_is_tracked_through_unwind() {
        let (mut first, mut second) = (SocketRecord::new(), SocketRecord::new());
        let (a, b) = (Socket::new(&mut first), Socket::new(&mut second));
        a.read(|_| {
            let nested = catch_unwind(AssertUnwindSafe(|| b.edit(|_| b.fd())));
            assert!(
                nested.is_err(),
                "the inner edit excludes a read of its record"
            );
            b.edit(|fields| fields.fd = 9);
            assert_eq!(b.fd(), 9, "unwinding released the inner edit");
        });
    }

    #[test]
    fn dropping_a_socket_releases_the_record_and_preserves_the_descriptor() {
        use std::io::{Read, Write};
        use std::os::fd::AsRawFd;
        use std::rc::Rc;
        let (mut reader, mut writer) = std::io::pipe().unwrap();
        let mut record = SocketRecord::with_fd(writer.as_raw_fd(), 0);
        let held = Rc::new(());
        record.set_owner(Some(Rc::clone(&held)));
        drop(OwnedSocket::new(Box::new(record)));
        assert_eq!(Rc::strong_count(&held), 1);
        writer.write_all(b"x").unwrap();
        let mut byte = [0];
        reader.read_exact(&mut byte).unwrap();
        assert_eq!(byte, *b"x");
    }

    #[test]
    fn a_loan_holder_releases_its_current_record_once() {
        use std::rc::Rc;
        let held = Rc::new(());
        let mut record = SocketRecord::new();
        record.set_owner(Some(Rc::clone(&held)));
        let holder = SocketLoans::new(|socket| {
            assert!(
                socket
                    .socket()
                    .edit(SocketFields::take_owner::<Rc<()>>)
                    .is_some()
            );
            socket.free();
        });
        holder.put(OwnedSocket::new(Box::new(record)), |_| unreachable!());
        assert_eq!(Rc::strong_count(&held), 2);
        drop(holder);
        assert_eq!(Rc::strong_count(&held), 1);
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
                            let borrows = socket.marks();
                            let readers = borrows.readers().len()
                                + usize::from(borrows.owner.load(Ordering::Relaxed) & !SHARED != 0);
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
    fn a_secondary_reader_keeps_its_borrow_after_the_primary_leaves() {
        let marks = SocketMarks::default();
        let primary = marks.take::<0>();
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let secondary = marks.take::<0>();
                barrier.wait();
                barrier.wait();
                let edit = catch_unwind(|| marks.take::<EDIT>());
                assert!(
                    edit.is_err(),
                    "the remaining reader cannot upgrade to an edit"
                );
                drop(secondary);
            });
            barrier.wait();
            drop(primary);
            barrier.wait();
            let _edit = marks.take::<EDIT>();
        });
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
                    while !Waiting::on(socket.marks()) {
                        assert!(began.elapsed() < Duration::from_secs(10), "B never waited");
                        std::thread::yield_now();
                    }
                    // Hold past the former test deadline while the reader waits.
                    std::thread::sleep(Duration::from_millis(25));
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
            handle().unwrap().marks().owner.load(Ordering::Relaxed),
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
