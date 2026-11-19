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
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

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
    inherited: *mut GlobalImpl,
}

impl GlobalImpl {
    /// `new GlobalMemoryImpl(size, shared)`, with its first page.
    fn new(size: u32, shared: bool, inherited: *mut Self) -> Result<*mut Self, i32> {
        let page = global::page_size(size, crate::swr_v1_core_pagesize()).ok_or(INVALID)?;
        let mut pages = Global::new(page, shared);
        if !pages.add_page() {
            return Err(NO_MEMORY);
        }
        Ok(Box::into_raw(Box::new(Self {
            pages: Mutex::new(pages),
            page_size: page,
            creator: if shared { std::process::id() } else { 0 },
            inherited,
        })))
    }

    fn pages(&self) -> MutexGuard<'_, Global> {
        self.pages.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for GlobalImpl {
    fn drop(&mut self) {
        if !self.inherited.is_null() {
            // SAFETY: `current` gave this pool the one it replaced, which only this frees.
            drop(unsafe { Box::from_raw(self.inherited) });
        }
    }
}

/// `swoole::GlobalMemory`: pages allocated once for the program's life, whose `free` is
/// a no-op. It holds its pool, which a forked process replaces (see `current`). Errors
/// are upstream's constructor's throws: `INVALID` for
/// `Exception(SW_ERROR_INVALID_PARAMS)` and `NO_MEMORY` for `std::bad_alloc`.
///
/// Every read of the pool counts itself in `users` first, so `delete` frees the pool only
/// when nothing reads it; a pool a reader still holds is left mapped instead.
pub struct GlobalMemory {
    pool: AtomicPtr<GlobalImpl>,
    users: AtomicUsize,
}

impl GlobalMemory {
    /// A memory with no pool yet, `SwooleG.memory_pool` before `swoole_init`.
    const fn empty() -> Self {
        Self {
            pool: AtomicPtr::new(ptr::null_mut()),
            users: AtomicUsize::new(0),
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
        let pool = GlobalImpl::new(pagesize, shared, ptr::null_mut())?;
        Ok(Self {
            pool: AtomicPtr::new(pool),
            users: AtomicUsize::new(0),
        })
    }

    /// Runs `read` over this process's pool, counted in `users` while it runs. In a
    /// process forked from the one that made a shared pool, a fresh pool replaces it
    /// first, without taking the inherited lock a parent thread may have held at the
    /// fork, and keeps it.
    fn with<R>(&self, read: impl FnOnce(&GlobalImpl) -> R) -> Result<R, i32> {
        self.users.fetch_add(1, Ordering::SeqCst);
        let result = self.current().map(read);
        self.users.fetch_sub(1, Ordering::SeqCst);
        result
    }

    /// The pool, for `with`, which counts the caller as a user.
    fn current(&self) -> Result<&GlobalImpl, i32> {
        let pool = self.pool.load(Ordering::SeqCst);
        // SAFETY: null or a pool this memory owns. `delete` takes it out before it frees
        // it, and frees it only when no `with` is running; `drop` has `&mut self`.
        let Some(found) = (unsafe { pool.as_ref() }) else {
            return Err(INVALID);
        };
        if found.creator == 0 || found.creator == std::process::id() {
            return Ok(found);
        }
        let fresh = GlobalImpl::new(found.page_size, true, pool)?;
        // Publish it only over the pool it replaces: a `delete` or another thread's
        // replacement in between wins, and the unpublished pool goes, minus the
        // inherited one it doesn't own.
        let published = self
            .pool
            .compare_exchange(pool, fresh, Ordering::SeqCst, Ordering::SeqCst);
        if published.is_err() {
            // SAFETY: `GlobalImpl::new`'s box, never published, so only this frame holds it.
            let mut unpublished = unsafe { Box::from_raw(fresh) };
            unpublished.inherited = ptr::null_mut();
            drop(unpublished);
        }
        // The pool there now: the fresh one, or whatever won.
        self.current()
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
        let pool = self.pool.swap(ptr::null_mut(), Ordering::SeqCst);
        if pool.is_null() {
            return;
        }
        if self.users.load(Ordering::SeqCst) == 0 {
            // SAFETY: `GlobalImpl::new`'s box, taken out of this memory, freed once. A
            // `with` that counted itself before this load holds it, so none does; one that
            // counts itself after it loads null.
            drop(unsafe { Box::from_raw(pool) });
        } else {
            // Still read, or lent out by `pool::words`: every such pool stays reachable,
            // so a leak check doesn't report it.
            static KEPT: Mutex<Vec<usize>> = Mutex::new(Vec::new());
            KEPT.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(pool.expose_provenance());
        }
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
    use std::ffi::c_void;
    use std::mem::MaybeUninit;
    use std::ptr;
    use std::slice;
    use std::sync::atomic::Ordering::SeqCst;
    use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64};

    static POOL: GlobalMemory = GlobalMemory::empty();

    /// `SwooleG.memory_pool = new GlobalMemory(SW_GLOBAL_MEMORY_PAGESIZE, true)`,
    /// `swoole_init`'s pool. Where upstream's constructor throws, the process aborts. A
    /// pool that is already there stays.
    pub fn init() {
        let page = crate::constants::SW_GLOBAL_MEMORY_PAGESIZE;
        let Ok(fresh) = GlobalMemory::new(page, true) else {
            crate::fatal("the global memory pool can't be created");
        };
        let pool = fresh.pool.swap(ptr::null_mut(), SeqCst);
        let installed = POOL
            .pool
            .compare_exchange(ptr::null_mut(), pool, SeqCst, SeqCst);
        if installed.is_err() {
            // Not installed: `fresh` frees it.
            fresh.pool.store(pool, SeqCst);
        }
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
    pub fn alloc(size: u32) -> *mut c_void {
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

    /// A block of `size` bytes from [`alloc`], for the process's life, or null. The pool
    /// counts a user that never leaves, so [`delete`] keeps the pages it sits in mapped.
    fn pinned(size: u32) -> *mut c_void {
        POOL.users.fetch_add(1, SeqCst);
        let block = alloc(size);
        if block.is_null() {
            POOL.users.fetch_sub(1, SeqCst);
        }
        block
    }

    /// `count` zero words from [`alloc`], for the process's life: `Swoole\Atomic`'s
    /// and `Swoole\Lock`'s, or a shared table's header, which forked workers reach at
    /// the same address.
    #[must_use]
    pub fn words<W: Word + 'static>(count: usize) -> Option<&'static mut [W]> {
        with_zero_words(count, Storage::Pool, |words| words)
    }

    /// The permanent shared mapping for the atomic wait registry.
    pub(crate) fn wait_registry_words(count: usize) -> Option<&'static [AtomicU64]> {
        with_zero_words(count, Storage::WaitRegistry, |words| &*words)
    }

    #[derive(Clone, Copy)]
    enum Storage {
        Pool,
        WaitRegistry,
    }

    fn with_zero_words<W: Word + 'static, T>(
        count: usize,
        storage: Storage,
        init: impl FnOnce(&'static mut [W]) -> T,
    ) -> Option<T> {
        const { assert!(align_of::<W>() <= 8, "the pool's blocks are 8-aligned") };
        let size = u32::try_from(count.checked_mul(size_of::<W>())?).ok()?;
        let block = match storage {
            Storage::Pool => pinned(size),
            Storage::WaitRegistry => super::sw_shm_malloc(size as usize),
        }
        .cast::<W>();
        if block.is_null() {
            return None;
        }
        // SAFETY: `count` zeroed (so valid) words, 8-aligned (enough for each `Word`,
        // asserted above), which the allocator gives no one else and keeps for the process's
        // life.
        Some(init(unsafe { slice::from_raw_parts_mut(block, count) }))
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

/// `swoole_hash_php` of the bytes a row compares: a made table's hash.
fn php_hash(key: &[u8], compared: usize) -> u64 {
    let key = key.get(..compared).unwrap_or(key);
    // `char`'s signedness, which the hash promotes, as `swoole_hash_php` passes it.
    crate::utilities::swoole_hash(key, 0, std::ffi::c_char::MIN != 0)
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

/// A table [`SharedTable::make`] made, owned by this process: its header lives for the
/// process's life, and its arena until [`SharedTable::destroy`] has freed it.
/// Dropping it leaves the table to the processes that share it (forked workers
/// may still use it), as upstream's objects do.
pub struct SharedTable {
    table: Handle,
    destroyed: std::cell::Cell<bool>,
}

impl SharedTable {
    /// `Table::make(rows_size, conflict_proportion)`, keys hashed with `hash`: the
    /// header comes from the global memory pool, which keeps it for the process's life,
    /// so forked workers share it.
    #[must_use]
    pub fn make(rows: u32, conflict: f32, hash: crate::memory::table::Hash) -> Option<Self> {
        let [slot] = pool::words::<std::mem::MaybeUninit<Table>>(1)? else {
            return None;
        };
        let table = Table::init(slot, rows, conflict, php_hash, table_warning, table_error)?;
        table.set_hash(hash);
        Some(Self {
            table: Handle::new(table),
            destroyed: std::cell::Cell::new(false),
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
mod tests;
