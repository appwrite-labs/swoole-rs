// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/memory/{fixed_pool,global_memory,
// shared_memory,table}.cc (Swoole contributors).
//! The upstream members of `GlobalMemory`, the `sw_shm_*` functions and `Table`'s
//! shared-memory lifecycle. A constructor's error is -1 for upstream's
//! `Exception(SW_ERROR_INVALID_PARAMS)` or -2 for `std::bad_alloc`.
//! `swoole::FixedPool`, the owner of an arena of its own, has no caller left but the
//! Rust ports of upstream's core tests, so it lives with them in
//! `tests/modules/memory/members.rs`; the table's collision pool is `fixed`'s.
//!
//! Each `// assert(...)` comment is the upstream assertion the `assert!` under it
//! carries.

use crate::log::{self, set_last_error};
use crate::memory::global::{self, Global};
use crate::memory::table::{Handle, Table};
use crate::sys;
use core::cell::Cell;
use core::marker::PhantomData;
use std::ffi::{c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

const INVALID: i32 = -1;
const NO_MEMORY: i32 = -2;
/// `SW_MIN_PAGE_SIZE`.
const MIN_PAGE_SIZE: u32 = 4096;

// ---------------------------------------------------------------- GlobalMemory

/// `GlobalMemoryImpl`: the pages, under the lock `alloc` holds.
pub struct GlobalImpl {
    pages: Mutex<Global>,
    page_size: u32,
    /// The process that made a shared pool, or 0: a forked process starts its own.
    creator: u32,
    /// The pool this one replaced in a forked process. Its pages stay mapped, since
    /// the parent's objects in them stay in use, and go with this pool.
    inherited: Option<Arc<GlobalImpl>>,
    task_id: std::sync::OnceLock<(&'static std::sync::atomic::AtomicI64, Arc<Page>)>,
}

impl GlobalImpl {
    /// `new GlobalMemoryImpl(size, shared)`, with its first page.
    fn new(size: u32, shared: bool, inherited: Option<Arc<Self>>) -> Result<Self, i32> {
        let page = global::page_size(size, crate::swr_v1_core_pagesize()).ok_or(INVALID)?;
        let mut pages = Global::new(page, shared);
        if !pages.add_page() {
            return Err(NO_MEMORY);
        }
        Ok(Self {
            pages: Mutex::new(pages),
            page_size: page,
            creator: if shared { std::process::id() } else { 0 },
            inherited,
            task_id: std::sync::OnceLock::new(),
        })
    }

    fn init_task_id(
        &self,
        allocate: impl FnOnce() -> (&'static std::sync::atomic::AtomicI64, Arc<Page>),
    ) {
        self.task_id.get_or_init(allocate);
    }

    fn task_id(&self) -> Option<&std::sync::atomic::AtomicI64> {
        let mut root = self;
        while let Some(parent) = root.inherited.as_deref() {
            root = parent;
        }
        root.task_id.get().map(|(word, _page)| *word)
    }

    fn pages(&self) -> MutexGuard<'_, Global> {
        self.pages.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// `swoole::GlobalMemory`: pages allocated once for the program's life, whose `free` is
/// a no-op. It holds its pool, which a forked process replaces on first access. Errors
/// are upstream's constructor's throws: `INVALID` for
/// `Exception(SW_ERROR_INVALID_PARAMS)` and `NO_MEMORY` for `std::bad_alloc`.
///
/// Each reader owns a reference to the generation it observed.
pub struct GlobalMemory {
    pool: AtomicPtr<GlobalImpl>,
    /// Serializes cloning or replacing the published Arc. A child can take over a
    /// publication guard held by a vanished parent thread: the pointer itself is atomic.
    publisher: AtomicU32,
}

struct Publication<'a>(&'a AtomicU32);

impl Drop for Publication<'_> {
    fn drop(&mut self) {
        self.0.store(0, Ordering::Release);
    }
}

impl GlobalMemory {
    const fn empty() -> Self {
        Self {
            pool: AtomicPtr::new(ptr::null_mut()),
            publisher: AtomicU32::new(0),
        }
    }

    fn publication(&self) -> Publication<'_> {
        let pid = std::process::id();
        loop {
            let owner = self.publisher.load(Ordering::Relaxed);
            if owner != pid
                && self
                    .publisher
                    .compare_exchange_weak(owner, pid, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                return Publication(&self.publisher);
            }
            std::thread::yield_now();
        }
    }

    fn snapshot(&self) -> Option<Arc<GlobalImpl>> {
        let _guard = self.publication();
        let pool = std::ptr::NonNull::new(self.pool.load(Ordering::Acquire))?;
        // SAFETY: publication holds one Arc count until replace takes this same guard.
        unsafe { Arc::increment_strong_count(pool.as_ptr()) };
        // SAFETY: this takes the independent strong count just acquired.
        Some(unsafe { Arc::from_raw(pool.as_ptr()) })
    }

    fn replace(&self, expected: Option<*const GlobalImpl>, next: Option<Arc<GlobalImpl>>) {
        let guard = self.publication();
        let old = self.pool.load(Ordering::Acquire);
        if expected.is_some_and(|p| p != old.cast_const()) {
            return;
        }
        self.pool.store(
            next.map_or(ptr::null_mut(), |p| Arc::into_raw(p).cast_mut()),
            Ordering::Release,
        );
        drop(guard);
        if !old.is_null() {
            // SAFETY: this removed the slot's Arc::into_raw count under the publication guard.
            drop(unsafe { Arc::from_raw(old) });
        }
    }

    /// `GlobalMemory(pagesize, shared)`.
    ///
    /// # Errors
    /// `INVALID` for a page size the pages can't use, `NO_MEMORY` when the first page
    /// can't be had.
    ///
    /// # Panics
    /// Upstream's assertion: pages are at least `SW_MIN_PAGE_SIZE`.
    #[expect(
        clippy::panic_in_result_fn,
        reason = "upstream's assertion on a caller's page size, not a runtime error"
    )]
    pub fn new(pagesize: u32, shared: bool) -> Result<Self, i32> {
        // assert(pagesize >= SW_MIN_PAGE_SIZE);
        assert!(
            pagesize >= MIN_PAGE_SIZE,
            "a page of at least SW_MIN_PAGE_SIZE"
        );
        let pool = GlobalImpl::new(pagesize, shared, None)?;
        Ok(Self {
            pool: AtomicPtr::new(Arc::into_raw(Arc::new(pool)).cast_mut()),
            publisher: AtomicU32::new(0),
        })
    }

    /// Read a snapshot outside the publication guard. After fork, publish fresh
    /// shared pages before touching a page mutex that a parent thread may hold.
    fn with<R>(&self, read: impl FnOnce(&GlobalImpl) -> R) -> Result<R, i32> {
        loop {
            let found = self.snapshot().ok_or(INVALID)?;
            if found.creator == 0 || found.creator == std::process::id() {
                return Ok(read(&found));
            }
            let expected = Arc::as_ptr(&found);
            let fresh = GlobalImpl::new(found.page_size, true, Some(found))?;
            self.replace(Some(expected), Some(Arc::new(fresh)));
        }
    }

    /// `void *GlobalMemory::alloc(uint32_t size)`: a zeroed block, or null with
    /// upstream's warning.
    ///
    /// # Errors
    /// `INVALID` once the pool is deleted, or what `GlobalMemory::new` returns when a
    /// forked process's fresh pool can't be had.
    pub fn alloc(&self, size: u32) -> Result<*mut c_void, i32> {
        self.with(|pool| alloc(&mut pool.pages(), size))
    }

    /// `size_t GlobalMemory::capacity() const`: zero without a pool.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.with(|pool| pool.pages().capacity()).unwrap_or(0)
    }

    /// `size_t GlobalMemory::get_memory_size() const`, as `capacity`.
    #[must_use]
    pub fn memory_size(&self) -> usize {
        self.with(|pool| pool.pages().memory_size()).unwrap_or(0)
    }

    /// Takes the pool out and frees it, every page and then the registry, unless a
    /// reader still holds it.
    fn delete(&self) {
        self.replace(None, None);
    }
}

impl Drop for GlobalMemory {
    /// `GlobalMemory::~GlobalMemory()`.
    fn drop(&mut self) {
        self.delete();
    }
}

/// `GlobalMemoryImpl::alloc`'s body: a block, or null with upstream's warning.
fn alloc(pages: &mut Global, size: u32) -> *mut c_void {
    if !pages.fits(size) {
        log::warning(
            c"GlobalMemory::alloc",
            format_args!(
                "failed to alloc {} bytes, exceed the maximum size[{}]",
                size.checked_add(7).map_or(0, |n| n & !7).cast_signed(),
                pages.page_size.cast_signed()
            ),
        );
        return ptr::null_mut();
    }
    let block = pages.alloc(size);
    if block.is_null() {
        log::warning(c"GlobalMemory::alloc", format_args!("alloc memory error"));
    }
    block
}

// ---------------------------------------------------------------- sw_shm_*

/// Mapping invariant: a shared mapping is `total` bytes, 8-aligned, whose first 8
/// bytes hold `total`; its payload follows.
/// A field holding a mapping `map` made (`sw_shm_malloc`'s, or a private one), or
/// null. Only its constructors and [`Shm::fill`] fill it, so [`Shm::free`] unmaps
/// exactly what they mapped, once. Only `free`, through `&mut`, unmaps it: a mapping
/// stays while the field is borrowed, so references into it may borrow the field.
/// Native code reads it as the payload pointer.
#[repr(transparent)]
#[derive(Debug)]
pub struct Shm(Cell<*mut c_void>);

/// A bump page whose pool and scoped allocations share ownership. The address is
/// immutable after construction. Only the last owner may unmap it.
pub(super) struct Page {
    address: AtomicPtr<c_void>,
    permanent: AtomicBool,
}

impl From<Shm> for Page {
    fn from(mapping: Shm) -> Self {
        Self {
            address: AtomicPtr::new(mapping.into_raw()),
            permanent: AtomicBool::new(false),
        }
    }
}

impl Page {
    pub(super) fn as_ptr(&self) -> *mut c_void {
        self.address.load(Ordering::Relaxed)
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        if !*self.permanent.get_mut() {
            Shm(Cell::new(*self.address.get_mut())).free();
        }
    }
}

impl Default for Shm {
    fn default() -> Self {
        Self::empty()
    }
}

impl Shm {
    /// No mapping.
    #[must_use]
    pub const fn empty() -> Self {
        Self(Cell::new(ptr::null_mut()))
    }

    /// `sw_shm_malloc(size)`: null when it fails.
    #[must_use]
    pub fn malloc(size: usize) -> Self {
        Self(Cell::new(sw_shm_malloc(size)))
    }

    /// `sw_shm_calloc(count, size)`: null when it fails.
    #[must_use]
    pub fn calloc(count: usize, size: usize) -> Self {
        Self(Cell::new(sw_shm_calloc(count, size)))
    }

    /// As [`Shm::calloc`], but a fork copies the mapping rather than sharing it, as
    /// it would `sw_calloc`'s memory.
    #[must_use]
    pub fn calloc_private(count: usize, size: usize) -> Self {
        Self(Cell::new(
            count
                .checked_mul(size)
                .map_or(ptr::null_mut(), |size| map(size, libc::MAP_PRIVATE)),
        ))
    }

    /// The mapping's base and total, if there is one.
    fn extent(&self) -> Option<(Region<'_>, usize)> {
        if self.0.get().is_null() {
            return None;
        }
        let base = self.0.get().cast::<usize>().wrapping_sub(1);
        // SAFETY: only `map` filled the field, so its header precedes the payload.
        let total = unsafe { base.read() };
        Some((Region(base.cast(), PhantomData), total))
    }

    /// The payload's bytes, 0 without a mapping.
    #[must_use]
    pub fn payload_len(&self) -> usize {
        self.extent().map_or(0, |(_, total)| total - 8)
    }

    /// The payload, or null.
    #[must_use]
    pub fn as_ptr(&self) -> *mut c_void {
        self.0.get()
    }

    #[must_use]
    pub fn is_null(&self) -> bool {
        self.0.get().is_null()
    }

    /// `SharedMemory::protect`: `mprotect`s the whole mapping.
    ///
    /// # Errors
    /// `mprotect`'s errno, or `EINVAL` without a mapping.
    pub fn protect(&self, flags: c_int) -> std::io::Result<()> {
        let Some((region, total)) = self.extent() else {
            return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
        };
        if mprotect(region, total, flags) == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    /// The payload, transferring ownership of the mapping to its next owner.
    #[must_use]
    pub fn into_raw(self) -> *mut c_void {
        self.0.into_inner()
    }

    /// `field = mapping` for an empty field, which takes `mapping`: whether it did.
    /// A field already mapped keeps its mapping, which a reference may borrow, and
    /// `mapping` is unmapped instead.
    pub fn fill(&self, mut mapping: Self) -> bool {
        let empty = self.is_null();
        if empty {
            self.0.set(mapping.into_raw());
        } else {
            mapping.free();
        }
        empty
    }

    /// `SharedMemory::free`: unmaps the mapping, if any, and leaves the field null.
    pub fn free(&mut self) {
        let Some((Region(base, _), total)) = self.extent() else {
            return;
        };
        // The field goes null with the one `Mapping` made of it.
        self.0.set(ptr::null_mut());
        if munmap(Mapping(base), total) < 0 {
            log::sys_warning(
                c"SharedMemory::free",
                format_args!("munmap({base:p}, {total}) failed"),
            );
        }
    }
}

/// A [`Shm`]'s mapping, as its base, for as long as the field is borrowed: one `map`
/// made, whose header holds the total. Only [`Shm::extent`] makes one.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Region<'a>(*mut c_void, PhantomData<&'a Shm>);

/// A mapping `map` made, as its base, which [`Shm::free`] makes once, from the field it
/// empties: so each mapping has one, and `munmap` takes it by value.
#[repr(transparent)]
struct Mapping(*mut c_void);

// libc's. Each takes a whole mapping this module made, as its header's total says, of
// which `Shm` lends no reference; `munmap` takes the one owner of a mapping no field
// names any more. They keep nothing, so they are declared `safe`.
unsafe extern "C" {
    safe fn mprotect(region: Region<'_>, len: usize, protection: c_int) -> c_int;
    safe fn munmap(mapping: Mapping, len: usize) -> c_int;
}

/// An anonymous mapping (`sharing`: `MAP_SHARED` or `MAP_PRIVATE`) of `size` bytes,
/// zeroed, whose size heads it, or null.
fn map(size: usize, sharing: c_int) -> *mut c_void {
    let Some(total) = size
        .checked_add(7)
        .and_then(|n| (n & !7).checked_add(8))
        .filter(|&n| isize::try_from(n).is_ok())
    else {
        return ptr::null_mut();
    };
    let memory = sys::map_anonymous(total, sharing == libc::MAP_SHARED, false);
    if memory.is_null() {
        log::sys_warning(c"SharedMemory::alloc", format_args!("mmap({total}) failed"));
        return ptr::null_mut();
    }
    // SAFETY: `memory` is a fresh, page-aligned mapping of `total >= 8` bytes.
    unsafe { memory.cast::<usize>().write(total) };
    memory.wrapping_byte_add(8)
}

/// `void *sw_shm_malloc(size_t size)`: an anonymous shared mapping whose size heads it.
#[must_use]
pub fn sw_shm_malloc(size: usize) -> *mut c_void {
    map(size, libc::MAP_SHARED)
}
/// `void *sw_shm_calloc(size_t num, size_t size)`: mappings start zeroed.
#[must_use]
pub fn sw_shm_calloc(count: usize, size: usize) -> *mut c_void {
    count
        .checked_mul(size)
        .map_or(ptr::null_mut(), sw_shm_malloc)
}

// ---------------------------------------------------------------- Table

/// `SwooleG.memory_pool`: the process's `GlobalMemory`, made by `swoole_init` and
/// deleted by `swoole_clean`. Its `free` is a no-op, so nothing calls it.
pub mod pool {
    use super::GlobalMemory;
    use std::mem::MaybeUninit;
    use std::ptr;
    use std::slice;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64};

    static POOL: GlobalMemory = GlobalMemory::empty();

    #[cfg(test)]
    #[cfg(not(miri))]
    pub(super) fn page() -> std::sync::Weak<super::Page> {
        POOL.with(|pool| std::sync::Arc::downgrade(pool.pages().last_page().unwrap()))
            .unwrap()
    }

    /// `SwooleG.memory_pool = new GlobalMemory(SW_GLOBAL_MEMORY_PAGESIZE, true)`,
    /// `swoole_init`'s pool. Where upstream's constructor throws, the process aborts. A
    /// pool that is already there stays.
    pub fn init() {
        let page = crate::constants::SW_GLOBAL_MEMORY_PAGESIZE;
        let Ok(fresh) = super::GlobalImpl::new(page, true, None) else {
            crate::fatal("the global memory pool can't be created");
        };
        POOL.replace(Some(ptr::null()), Some(Arc::new(fresh)));
    }

    /// Allocate the task sequence before the first worker fork. Its pool generation
    /// owns the mapping until all readers and inherited generations have released it.
    pub(crate) fn init_task_id() {
        let Some(root) = POOL.snapshot() else {
            crate::fatal("no pool for the task ID counter")
        };
        root.init_task_id(|| {
            let Some((words, page)) = scoped_words::<AtomicI64>(1) else {
                crate::fatal("task ID counter allocation failed")
            };
            let [word] = words else {
                crate::fatal("expected one task ID counter")
            };
            (word, page)
        });
    }

    /// Keep the inherited generation live while using its task sequence.
    pub(crate) fn with_task_id<R>(fallback: &AtomicI64, f: impl FnOnce(&AtomicI64) -> R) -> R {
        let live = POOL.snapshot();
        f(live
            .as_deref()
            .and_then(super::GlobalImpl::task_id)
            .unwrap_or(fallback))
    }

    /// `delete SwooleG.memory_pool`. A thread still reading the pool keeps it mapped.
    pub fn delete() {
        POOL.delete();
    }

    pub use crate::utilities::wait_registry_init;

    /// `sw_mem_pool()->alloc(size)`: a zeroed block, or null with upstream's warning.
    /// Where upstream throws (no pool, or a forked process's fresh pool can't be had),
    /// the process aborts.
    #[must_use]
    pub fn alloc(size: u32) -> *mut std::ffi::c_void {
        POOL.alloc(size)
            .unwrap_or_else(|_| crate::fatal("no memory pool to allocate from"))
    }

    /// `sw_mem_pool()->get_memory_size()`, or 0 before `swoole_init`.
    #[must_use]
    pub fn memory_size() -> usize {
        POOL.memory_size()
    }

    /// A type that is valid zeroed: an atomic (a process-shared mutex's storage is
    /// such words, zeroed until `init`), or room for a value not yet written.
    pub trait Word: sealed::Sealed {}
    impl Word for AtomicU32 {}
    impl Word for AtomicI64 {}
    impl Word for AtomicU64 {}
    impl Word for crate::locks::SharedMutex {}
    impl<T> Word for MaybeUninit<T> {}
    mod sealed {
        pub trait Sealed {}
        impl Sealed for std::sync::atomic::AtomicU32 {}
        impl Sealed for std::sync::atomic::AtomicI64 {}
        impl Sealed for std::sync::atomic::AtomicU64 {}
        impl Sealed for crate::locks::SharedMutex {}
        impl<T> Sealed for std::mem::MaybeUninit<T> {}
    }

    /// One pool allocation whose owner only lends the word for a borrow of itself.
    /// The pin holds this word’s mapping. A fork copies the count and
    /// the owner into the child while both processes share the word.
    pub struct PooledWord<W: 'static> {
        word: &'static W,
        _pin: PoolPin,
    }

    pub(super) type PoolPin = std::sync::Arc<super::Page>;

    pub(super) fn table_header(
        rows: u32,
        conflict: f32,
        hash: crate::memory::table::Hash,
    ) -> Option<(super::Handle, PoolPin)> {
        let (slots, pin) = scoped_words::<MaybeUninit<super::Table>>(1)?;
        let table = super::Table::init(
            slots.first_mut()?,
            rows,
            conflict,
            hash,
            super::table_warning,
            super::table_error,
        )?;
        Some((super::Handle::new(table), pin))
    }

    impl<W: Word> PooledWord<W> {
        /// Allocate one zeroed word, holding the pin taken by `words`.
        #[must_use]
        pub fn allocate() -> Option<Self> {
            let (words, pin) = scoped_words(1)?;
            Some(Self {
                word: words.first()?,
                _pin: pin,
            })
        }
    }

    impl<W> std::ops::Deref for PooledWord<W> {
        type Target = W;

        fn deref(&self) -> &W {
            self.word
        }
    }

    fn scoped_words<W: Word + 'static>(count: usize) -> Option<(&'static mut [W], PoolPin)> {
        let (words, page) = zero_words(count, Storage::Pool)?;
        Some((words, page?))
    }

    /// `count` zero words from [`alloc`], for the process's life: `Swoole\Atomic`'s
    /// and `Swoole\Lock`'s, or a shared table's header, which forked workers reach at
    /// the same address.
    #[must_use]
    pub fn words<W: Word + 'static>(count: usize) -> Option<&'static mut [W]> {
        let (words, page) = scoped_words(count)?;
        page.permanent
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Some(words)
    }

    /// A process-shared pthread lock whose PHP object owns the pool pin. Forked
    /// processes keep their own copy of the pin over the same shared storage.
    pub struct PooledLock {
        lock: crate::locks::Lock,
        _pin: PoolPin,
    }

    impl PooledLock {
        /// Allocate storage for the lock and keep its pin until the object drops.
        #[must_use]
        pub fn new(kind: crate::locks::Kind) -> Option<Self> {
            let (storage, pin) = scoped_words(crate::locks::Lock::words(kind))?;
            Some(Self {
                lock: crate::locks::Lock::shared(kind, storage)?,
                _pin: pin,
            })
        }
    }

    impl std::ops::Deref for PooledLock {
        type Target = crate::locks::Lock;

        fn deref(&self) -> &Self::Target {
            &self.lock
        }
    }

    /// The permanent shared mapping for the atomic wait registry.
    pub(crate) fn wait_registry_words(count: usize) -> Option<&'static [AtomicU64]> {
        zero_words(count, Storage::WaitRegistry).map(|(words, _)| &*words)
    }

    #[derive(Clone, Copy)]
    enum Storage {
        Pool,
        WaitRegistry,
    }

    fn zero_words<W: Word + 'static>(
        count: usize,
        storage: Storage,
    ) -> Option<(&'static mut [W], Option<PoolPin>)> {
        const { assert!(align_of::<W>() <= 8, "the pool's blocks are 8-aligned") };
        let size = u32::try_from(count.checked_mul(size_of::<W>())?).ok()?;
        let (block, page) = match storage {
            Storage::Pool => POOL
                .with(|pool| {
                    let mut pages = pool.pages();
                    (super::alloc(&mut pages, size), pages.last_page().cloned())
                })
                .unwrap_or_else(|_| crate::fatal("no memory pool to allocate from")),
            Storage::WaitRegistry => (super::sw_shm_malloc(size as usize), None),
        };
        let block = block.cast::<W>();
        if block.is_null() {
            return None;
        }
        // SAFETY: `count` zeroed (so valid) words, 8-aligned (enough for each `Word`,
        // asserted above). The registry mapping is permanent. Pool callers keep the returned
        // page until all borrows end or mark it permanent before lending a static slice.
        Some((unsafe { slice::from_raw_parts_mut(block, count) }, page))
    }
}

fn table_error(code: c_int) {
    set_last_error(code);
}

/// `table_warning(message, length)`: a warning whose text starts with its function,
/// `Name(): `.
fn table_warning(message: &[u8]) {
    let text = String::from_utf8_lossy(message);
    let (origin, rest) = text.split_once("(): ").unwrap_or(("", &text));
    let origin = std::ffi::CString::new(origin).unwrap_or_default();
    log::warning(&origin, format_args!("{rest}"));
}

/// `Table::create()`: lays the table out in a fresh shared arena.
#[must_use]
pub fn swr_table_create(table: &mut Handle) -> bool {
    let size = table.arena_size();
    if size == 0 {
        return false;
    }
    let mut arena = Shm::malloc(size);
    if arena.is_null() {
        return false;
    }
    if !table.create(&mut arena) {
        arena.free();
        return false;
    }
    true
}

/// A process-local owner of the shared table header. Its arena is released on the
/// final drop after an explicit destroy. Forked workers retain their own mapping.
pub struct SharedTable {
    table: Handle,
    destroyed: std::cell::Cell<bool>,
    // Release this process's pin after the handle and its header views.
    _pin: pool::PoolPin,
}

impl SharedTable {
    /// `Table::make(rows_size, conflict_proportion)` with the caller's row hash.
    #[must_use]
    pub fn make(rows: u32, conflict: f32, hash: crate::memory::table::Hash) -> Option<Self> {
        let (table, pin) = pool::table_header(rows, conflict, hash)?;
        Some(Self {
            table,
            destroyed: std::cell::Cell::new(false),
            _pin: pin,
        })
    }

    /// `Table::add_column`.
    pub fn add_column(&mut self, name: &[u8], kind: u32, size: usize) -> bool {
        self.table.add_column(name, kind, size)
    }

    /// `Table::create()` ([`swr_table_create`]).
    pub fn create(&mut self) -> bool {
        swr_table_create(&mut self.table)
    }

    /// `Table::destroy()`, once this is dropped.
    pub fn destroy(&self) {
        self.destroyed.set(true);
    }
}

impl std::ops::Deref for SharedTable {
    type Target = Handle;

    fn deref(&self) -> &Handle {
        &self.table
    }
}

impl Drop for SharedTable {
    fn drop(&mut self) {
        if self.destroyed.get() {
            // `Table::destroy()`: frees the arena; the pool keeps the header.
            self.table.destroy().free();
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/modules/memory/members.rs"]
#[expect(clippy::disallowed_methods, reason = "tests wake their own waits")]
mod tests;
