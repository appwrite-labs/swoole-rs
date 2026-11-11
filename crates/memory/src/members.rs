// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/memory/{fixed_pool,global_memory,
// shared_memory,table}.cc (Swoole contributors).
//! The upstream members of `FixedPool`, `GlobalMemory`, the `sw_shm_*`
//! functions and `Table`'s shared-memory lifecycle. A constructor's error is -1 for
//! upstream's `Exception(SW_ERROR_INVALID_PARAMS)` or -2 for `std::bad_alloc`.
//! `FixedPool` and `GlobalMemory` have no C++ facade left: they are Rust owners whose
//! methods serve the Rust ports of upstream's core tests in `tests/modules/memory/members.rs`.
//!
//! Each `// assert(...)` comment is the upstream assertion the `assert!` under it
//! carries.

use crate::fixed::{self, DebugCursor, Fixed, Layout};
use crate::global::{self, Global};
use crate::table::Table;
use std::ffi::{c_int, c_void};
use std::mem;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use swoole_rs_corebase::log::{self, set_last_error};
use swoole_rs_corebase::{sw_free, sw_malloc};

const INVALID: i32 = -1;
const NO_MEMORY: i32 = -2;
/// `SW_ERROR_MALLOC_FAIL`.
const MALLOC_FAIL: c_int = 501;
/// `SW_MIN_PAGE_SIZE`.
const MIN_PAGE_SIZE: u32 = 4096;

/// An arena of `size` bytes from `sw_shm_malloc` (`shared`) or `sw_malloc`.
pub(crate) fn arena(shared: bool, size: usize) -> *mut c_void {
    if shared {
        sw_shm_malloc(size)
    } else {
        sw_malloc(size)
    }
}

/// Frees what `arena(shared, ..)` returned.
///
/// # Safety
/// `memory` is live and came from `arena(shared, ..)`.
pub(crate) unsafe fn release(shared: bool, memory: *mut c_void) {
    if shared {
        sw_shm_free(memory);
    } else {
        // SAFETY: as the caller promises.
        unsafe { sw_free(memory) };
    }
}

// ---------------------------------------------------------------- FixedPool

/// `swoole::FixedPool` over an owned arena: its header, then its slices, in
/// memory from `arena(shared, ..)`. `&mut self` serializes allocation and release.
pub struct FixedPool(NonNull<Fixed>);

impl FixedPool {
    /// `FixedPool(slice_num, slice_size, shared)`: `INVALID` for upstream's
    /// `Exception(SW_ERROR_INVALID_PARAMS)`, `NO_MEMORY` for `std::bad_alloc`.
    ///
    /// # Errors
    /// As above.
    pub fn new(slices: u32, width: u32, shared: bool) -> Result<Self, i32> {
        let plan = Layout::new(slices, width, 0, true).ok_or(INVALID)?;
        let memory = NonNull::new(arena(shared, plan.total).cast::<Fixed>()).ok_or(NO_MEMORY)?;
        // SAFETY: a fresh, malloc-aligned arena of `plan.total` bytes.
        unsafe { fixed::construct(memory.as_ptr(), plan, shared, true) };
        Ok(Self(memory))
    }

    fn header(&self) -> &Fixed {
        // SAFETY: the pool's constructed header, live until drop.
        unsafe { self.0.as_ref() }
    }

    /// `void *FixedPool::alloc(uint32_t size)`: the slice width was fixed at construction.
    ///
    /// # Panics
    /// Upstream's assertion: an allocation fails only when no slice is spare.
    pub fn alloc(&mut self) -> *mut c_void {
        // SAFETY: a constructed pool; `&mut self` serializes allocation.
        let result = unsafe { fixed::alloc(self.0.as_ptr()) };
        if result.is_null() {
            set_last_error(MALLOC_FAIL);
            // assert(get_number_of_spare_slice() == 0);
            assert_eq!(self.spare(), 0, "a failed allocation leaves no spare slice");
        }
        result
    }

    /// `void FixedPool::free(void *ptr)`.
    ///
    /// # Panics
    /// Upstream's assertions: `data` is an allocated slice of the pool.
    pub fn free(&mut self, data: *mut c_void) {
        let (memory, size) = (self.header().memory, self.header().size);
        // assert(ptr > impl->memory && static_cast<char *>(ptr) < static_cast<char *>(impl->memory) + impl->size);
        assert!(
            data.addr() > memory.addr() && data.addr() < memory.addr() + size,
            "a slice of this pool"
        );
        // The same address, from the arena: nothing is read through the caller's pointer.
        let slice = memory.wrapping_add(data.addr() - memory.addr());
        // SAFETY: a constructed pool; `free` checks the stride before it reads a
        // slice header, then upstream's `slice->lock == 1`, so a pointer into a
        // slice's (maybe uninitialized) data is refused unread.
        let freed = unsafe { fixed::free(self.0.as_ptr(), slice.cast()) };
        assert!(freed, "a live slice of this pool");
    }

    /// `uint32_t FixedPool::get_number_of_spare_slice() const`.
    #[must_use]
    pub fn spare(&self) -> u32 {
        self.header().slices().0
    }

    /// `uint32_t FixedPool::get_number_of_total_slice() const`.
    #[must_use]
    pub fn total(&self) -> u32 {
        self.header().slices().1
    }

    /// `uint32_t FixedPool::get_slice_size() const`.
    #[must_use]
    pub fn slice_size(&self) -> u32 {
        self.header().slice_size
    }

    /// `void FixedPool::debug(int max_lines) const`: the slices on stdout, through C stdio.
    pub fn debug(&self, max_lines: c_int) {
        let mut cursor = DebugCursor::default();
        // SAFETY: the format takes one string.
        unsafe {
            libc::printf(
                c"===============================%s=================================\n".as_ptr(),
                c"debug".as_ptr(),
            )
        };
        // SAFETY: a constructed pool.
        while let Some(record) = unsafe { fixed::debug_step(self.header(), &mut cursor, max_lines) }
        {
            if record.self_link {
                // SAFETY: a literal format with no arguments.
                unsafe {
                    libc::printf(
                        c"-------------------@@@@@@@@@@@@@@@@@@@@@@----------------\n".as_ptr(),
                    )
                };
            }
            // SAFETY: the arguments match the format.
            unsafe {
                libc::printf(
                    c"#%llu\tslice[%p]\tprev=%p\tnext=%p\ttag=%d\tdata=%p\n".as_ptr(),
                    libc::c_ulonglong::from(record.line),
                    record.slice,
                    record.prev,
                    record.next,
                    c_int::from(record.lock),
                    record.data,
                )
            };
        }
    }
}

impl Drop for FixedPool {
    /// `FixedPool::~FixedPool()`.
    fn drop(&mut self) {
        let shared = self.header().shared != 0;
        // SAFETY: the arena came from `arena(shared, ..)` in `new`, freed once.
        unsafe { release(shared, self.0.as_ptr().cast()) };
    }
}

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
        let page =
            global::page_size(size, swoole_rs_corebase::swr_v1_core_pagesize()).ok_or(INVALID)?;
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
            return self.current();
        }
        // SAFETY: the fresh pool, owned as above.
        Ok(unsafe { &*fresh })
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
        if !pool.is_null() && self.users.load(Ordering::SeqCst) == 0 {
            // SAFETY: `GlobalImpl::new`'s box, taken out of this memory, freed once. A
            // `with` that counted itself before this load holds it, so none does; one that
            // counts itself after it loads null.
            drop(unsafe { Box::from_raw(pool) });
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
///
/// The mapping whose payload is `payload`: its base and total.
///
/// # Safety
/// `payload` is a live mapping's payload.
unsafe fn extent(payload: *mut c_void) -> (*mut c_void, usize) {
    let base = payload.cast::<usize>().wrapping_sub(1);
    // SAFETY: as the caller promises, the header precedes the payload.
    (base.cast(), unsafe { base.read() })
}

/// A field holding an `sw_shm_calloc` mapping, or null. Only [`Shm::calloc`]
/// fills it, so [`Shm::free`] unmaps exactly what it mapped, once. Native code
/// reads it as the payload pointer.
#[repr(transparent)]
#[derive(Debug)]
pub struct Shm(*mut c_void);

impl Shm {
    /// No mapping.
    pub const NULL: Self = Self(ptr::null_mut());

    /// `sw_shm_calloc(count, size)`: null when it fails.
    #[must_use]
    pub fn calloc(count: usize, size: usize) -> Self {
        Self(sw_shm_calloc(count, size))
    }

    /// The payload, or null.
    #[must_use]
    pub fn as_ptr(&self) -> *mut c_void {
        self.0
    }

    #[must_use]
    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }

    /// Unmaps the mapping, if any, and leaves the field null.
    pub fn free(&mut self) {
        let payload = core::mem::replace(&mut self.0, ptr::null_mut());
        if !payload.is_null() {
            unmap(payload);
        }
    }
}

/// `SharedMemory::free`: unmaps the mapping `payload` heads.
fn unmap(payload: *mut c_void) {
    // SAFETY: the caller passes a live mapping's payload.
    let (base, total) = unsafe { extent(payload) };
    // SAFETY: the mapping its header describes.
    if unsafe { libc::munmap(base, total) } < 0 {
        log::sys_warning(
            c"SharedMemory::free",
            format_args!("munmap({base:p}, {total}) failed"),
        );
    }
}

upstream! {
    "_Z13sw_shm_mallocm", "_Z13sw_shm_mallocm" =>
    /// `void *sw_shm_malloc(size_t size)`: an anonymous shared mapping whose size heads it.
    #[must_use]
    pub extern "C" fn sw_shm_malloc(size: usize) -> *mut c_void {
        let Some(total) = size
            .checked_add(7)
            .and_then(|n| (n & !7).checked_add(8))
            .filter(|&n| isize::try_from(n).is_ok())
        else {
            return ptr::null_mut();
        };
        let flags = libc::MAP_SHARED | libc::MAP_ANONYMOUS;
        // SAFETY: a fresh anonymous mapping.
        let memory = unsafe { libc::mmap(ptr::null_mut(), total, libc::PROT_READ | libc::PROT_WRITE, flags, -1, 0) };
        if memory == libc::MAP_FAILED {
            log::sys_warning(c"SharedMemory::alloc", format_args!("mmap({total}) failed"));
            return ptr::null_mut();
        }
        // SAFETY: `memory` is a fresh, page-aligned mapping of `total >= 8` bytes.
        unsafe { memory.cast::<usize>().write(total) };
        memory.wrapping_byte_add(8)
    }
    "_Z13sw_shm_callocmm", "_Z13sw_shm_callocmm" =>
    /// `void *sw_shm_calloc(size_t num, size_t size)`: mappings start zeroed.
    #[must_use]
    pub extern "C" fn sw_shm_calloc(count: usize, size: usize) -> *mut c_void {
        count.checked_mul(size).map_or(ptr::null_mut(), |size| sw_shm_malloc(size))
    }
    "_Z11sw_shm_freePv", "_Z11sw_shm_freePv" =>
    /// `void sw_shm_free(void *ptr)`.
    pub extern "C" fn sw_shm_free(payload: *mut c_void) {
        unmap(payload);
    }
    "_Z14sw_shm_protectPvi", "_Z14sw_shm_protectPvi" =>
    /// `int sw_shm_protect(void *ptr, int flags)`.
    ///
    /// # Safety
    /// `payload` is a live `sw_shm_malloc` payload.
    pub unsafe extern "C" fn sw_shm_protect(payload: *mut c_void, flags: c_int) -> c_int {
        // SAFETY: per the caller.
        let (base, total) = unsafe { extent(payload) };
        // SAFETY: the mapping its header describes.
        unsafe { libc::mprotect(base, total, flags) }
    }
}

// ---------------------------------------------------------------- Table

/// `SwooleG.memory_pool`: the process's `GlobalMemory`, made by `swoole_init` and
/// deleted by `swoole_clean`. Its `free` is a no-op, so nothing calls it.
pub mod pool {
    use super::GlobalMemory;
    use std::ffi::c_void;
    use std::ptr;
    use std::slice;
    use std::sync::atomic::Ordering::SeqCst;
    use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64};

    static POOL: GlobalMemory = GlobalMemory::empty();

    /// `SwooleG.memory_pool = new GlobalMemory(SW_GLOBAL_MEMORY_PAGESIZE, true)`,
    /// `swoole_init`'s pool. Where upstream's constructor throws, the process aborts. A
    /// pool that is already there stays.
    pub fn init() {
        let page = swoole_rs_corebase::constants::SW_GLOBAL_MEMORY_PAGESIZE;
        let Ok(fresh) = GlobalMemory::new(page, true) else {
            std::process::abort();
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

    /// Without futex, waiters register in a Rust registry in shared memory, from the
    /// pool `swoole_init` just made: a fatal error when it can't be had.
    pub fn wait_registry_init() {
        let size = u32::try_from(swoole_rs_utilities::swr_v1_utils_wait_registry_size());
        let registry = alloc(size.unwrap_or(u32::MAX));
        // SAFETY: null or that many shared bytes, for the program's life.
        if !unsafe { swoole_rs_utilities::swr_v1_utils_wait_registry_init(registry) } {
            let message = c"failed to allocate atomic wait registry";
            // SAFETY: `swoole_init`'s hook exits; no frame here owns anything.
            unsafe { swoole_rs_corebase::fatal_error(super::MALLOC_FAIL, message) };
        }
    }

    /// `sw_mem_pool()->alloc(size)`: a zeroed block, or null with upstream's warning.
    /// Where upstream throws (no pool, or a forked process's fresh pool can't be had),
    /// the process aborts.
    #[must_use]
    pub fn alloc(size: u32) -> *mut c_void {
        POOL.alloc(size).unwrap_or_else(|_| std::process::abort())
    }

    /// `sw_mem_pool()->get_memory_size()`, or 0 before `swoole_init`.
    #[must_use]
    pub fn memory_size() -> usize {
        POOL.memory_size()
    }

    /// An atomic that is valid zeroed and needs no more than the pool's alignment.
    pub trait Word: sealed::Sealed {}
    impl Word for AtomicU32 {}
    impl Word for AtomicI64 {}
    impl Word for AtomicU64 {}
    mod sealed {
        pub trait Sealed {}
        impl Sealed for std::sync::atomic::AtomicU32 {}
        impl Sealed for std::sync::atomic::AtomicI64 {}
        impl Sealed for std::sync::atomic::AtomicU64 {}
    }

    /// `count` zero words from [`alloc`], for the process's life: `Swoole\Atomic`'s
    /// and `Swoole\Lock`'s. The pool counts a user that never leaves, so [`delete`]
    /// keeps the pages these words sit in mapped.
    #[must_use]
    pub fn words<W: Word>(count: usize) -> Option<&'static mut [W]> {
        let size = u32::try_from(count.checked_mul(size_of::<W>())?).ok()?;
        POOL.users.fetch_add(1, SeqCst);
        let block = alloc(size).cast::<W>();
        if block.is_null() {
            POOL.users.fetch_sub(1, SeqCst);
            return None;
        }
        // SAFETY: `count` zeroed (so valid) words, 8-aligned (enough for each
        // `Word`), which the pool gives no one else.
        Some(unsafe { slice::from_raw_parts_mut(block, count) })
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
    // SAFETY: `key` holds its length.
    unsafe { swoole_rs_utilities::misc::swoole_hash_php(key.as_ptr().cast(), key.len()) }
}

/// `Table::make(rows_size, conflict_proportion)`: the header comes from the global
/// memory pool, so forked workers share it.
#[must_use]
pub fn swr_table_make(rows: u32, conflict: f32) -> *mut Table {
    #[expect(clippy::cast_possible_truncation, reason = "a sub-page header")]
    let table = pool::alloc(mem::size_of::<Table>() as u32).cast::<Table>();
    if table.is_null() {
        return table;
    }
    // SAFETY: the pool's blocks are 8-aligned; `size_of::<Table>()` writable bytes.
    if unsafe { !Table::init(table, rows, conflict, php_hash, table_warning, table_error) } {
        return ptr::null_mut();
    }
    table
}

/// `Table::create()`: lays the table out in a fresh shared arena.
///
/// # Safety
/// `this` is a made table.
#[must_use]
pub unsafe fn swr_table_create(this: *mut Table) -> bool {
    // SAFETY: as the caller promises.
    let table = unsafe { &mut *this };
    let size = table.arena_size();
    if size == 0 {
        return false;
    }
    let arena = sw_shm_malloc(size);
    if arena.is_null() {
        return false;
    }
    // SAFETY: a fresh arena of `size` bytes.
    if unsafe { !table.create(arena.cast()) } {
        sw_shm_free(arena);
        return false;
    }
    true
}

/// `Table::destroy()`: frees the arena, then the header.
///
/// # Safety
/// `this` is a made table, destroyed once.
pub unsafe fn swr_table_destroy(this: *mut Table) {
    // SAFETY: as the caller promises.
    let arena = unsafe { Table::destroy(this) };
    if !arena.is_null() {
        sw_shm_free(arena.cast());
    }
}

/// A table [`swr_table_make`] made, owned by this process: its header lives as
/// long as this does, unless [`SharedTable::destroy`] has freed it by then.
/// Dropping it leaves the table to the processes that share it (forked workers
/// may still use it), as upstream's objects do.
pub struct SharedTable {
    table: NonNull<Table>,
    destroyed: std::cell::Cell<bool>,
}

impl SharedTable {
    /// [`swr_table_make`], keys hashed with `hash`.
    #[must_use]
    pub fn make(rows: u32, conflict: f32, hash: crate::table::Hash) -> Option<Self> {
        let mut table = NonNull::new(swr_table_make(rows, conflict))?;
        // SAFETY: a made table nothing else references yet.
        unsafe { table.as_mut() }.set_hash(hash);
        Some(Self {
            table,
            destroyed: std::cell::Cell::new(false),
        })
    }

    /// `Table::add_column`.
    pub fn add_column(&mut self, name: &[u8], kind: u32, size: usize) -> bool {
        // SAFETY: a live table, and `&mut self` excludes every borrow of it.
        unsafe { self.table.as_mut() }.add_column(name, kind, size)
    }

    /// `Table::create()` ([`swr_table_create`]).
    pub fn create(&mut self) -> bool {
        // SAFETY: a made table, which `&mut self` borrows alone.
        unsafe { swr_table_create(self.table.as_ptr()) }
    }

    /// `Table::destroy()` ([`swr_table_destroy`]), once this is dropped.
    pub fn destroy(&self) {
        self.destroyed.set(true);
    }
}

impl std::ops::Deref for SharedTable {
    type Target = Table;

    fn deref(&self) -> &Table {
        // SAFETY: live until this drops; only `&mut self` mutates it.
        unsafe { self.table.as_ref() }
    }
}

impl Drop for SharedTable {
    fn drop(&mut self) {
        if self.destroyed.get() {
            // SAFETY: a made table, destroyed once: nothing borrows it any more.
            unsafe { swr_table_destroy(self.table.as_ptr()) };
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/modules/memory/members.rs"]
mod tests;
