// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/memory/{fixed_pool,global_memory,
// shared_memory,table}.cc (Swoole contributors).
//! The upstream members of `FixedPool`, `GlobalMemory`, the `sw_shm_*`
//! functions and `Table`'s shared-memory lifecycle. `native/memory/swoole_memory.h` keeps
//! `GlobalMemory`'s inline members for C++ callers, whose constructor throws; they call
//! the `swr_v1_memory_global_memory_*` functions below. A
//! constructor returns 0, -1 for upstream's `Exception(SW_ERROR_INVALID_PARAMS)` or -2
//! for `std::bad_alloc`. `FixedPool` has no C++ facade left; its
//! members serve the Rust ports of upstream's core tests in `tests/modules/memory/members.rs`.
//!
//! Each `// assert(...)` comment is the upstream assertion the `assert!` under it
//! carries.

use crate::fixed::{self, DebugCursor, Fixed, Layout, Slice};
use crate::global::{self, Global};
use crate::table::Table;
use std::ffi::{c_char, c_int, c_void};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::{mem, ptr};
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

/// `FixedPool(slice_num, slice_size, shared)`: an owned arena.
///
/// # Safety
/// `output` is writable.
pub unsafe fn swr_v1_memory_fixed_pool_new(
    slices: u32,
    width: u32,
    shared: bool,
    output: *mut *mut Fixed,
) -> i32 {
    let Some(plan) = Layout::new(slices, width, 0, true) else {
        return INVALID;
    };
    let memory = arena(shared, plan.total);
    if memory.is_null() {
        return NO_MEMORY;
    }
    // SAFETY: a fresh, malloc-aligned arena of `plan.total` bytes.
    unsafe { fixed::construct(memory.cast(), plan, shared, true) };
    // SAFETY: as the caller promises.
    unsafe { output.write(memory.cast()) };
    0
}

/// `FixedPool(slice_size, memory, size, shared)`: a pool over the caller's `memory`.
///
/// # Safety
/// `memory` is null or `size` writable bytes the pool may use until it is destroyed;
/// `output` is writable.
pub unsafe fn swr_v1_memory_fixed_pool_borrow(
    width: u32,
    memory: *mut c_void,
    size: usize,
    shared: bool,
    output: *mut *mut Fixed,
) -> i32 {
    let Some(plan) = Layout::new(0, width, size, false) else {
        return INVALID;
    };
    if memory.is_null() || memory.addr().checked_add(plan.total).is_none() {
        return INVALID;
    }
    // SAFETY: as the caller promises, `size` (at least `plan.total`) writable bytes.
    if unsafe { !fixed::construct(memory.cast(), plan, shared, false) } {
        return INVALID;
    }
    // SAFETY: as the caller promises.
    unsafe { output.write(memory.cast()) };
    0
}

/// `FixedPool::~FixedPool()`.
///
/// # Safety
/// `pool` is a constructed pool, destroyed once.
pub unsafe fn swr_v1_memory_fixed_pool_delete(pool: *mut Fixed) {
    // SAFETY: as the caller promises.
    let header = unsafe { &*pool };
    if header.allocated != 0 {
        // SAFETY: an owned pool's arena came from `arena(shared, ..)`.
        unsafe { release(header.shared != 0, pool.cast()) };
    }
}

/// `void *FixedPool::alloc(uint32_t size)`: the slice width was fixed at construction.
///
/// # Panics
/// Upstream's assertion: an allocation fails only when no slice is spare.
///
/// # Safety
/// `pool` is a constructed pool; allocation is serialized.
pub unsafe fn swr_v1_memory_fixed_pool_alloc(pool: *mut Fixed) -> *mut c_void {
    // SAFETY: as the caller promises.
    let result = unsafe { fixed::alloc(pool) };
    if result.is_null() {
        set_last_error(MALLOC_FAIL);
        // SAFETY: as the caller promises.
        let header = unsafe { &*pool };
        // assert(get_number_of_spare_slice() == 0);
        assert_eq!(
            header.slices().0,
            0,
            "a failed allocation leaves no spare slice"
        );
    }
    result
}

/// `void FixedPool::free(void *ptr)`.
///
/// # Panics
/// Upstream's assertions: `data` is an allocated slice of the pool.
///
/// # Safety
/// `pool` is a constructed pool, `data` a live slice of it; release is serialized.
pub unsafe fn swr_v1_memory_fixed_pool_free(pool: *mut Fixed, data: *mut c_void) {
    // SAFETY: as the caller promises.
    let header = unsafe { &*pool };
    let (memory, size) = (header.memory.addr(), header.size);
    // assert(ptr > impl->memory && static_cast<char *>(ptr) < static_cast<char *>(impl->memory) + impl->size);
    assert!(
        data.addr() > memory && data.addr() < memory + size,
        "a slice of this pool"
    );
    let slice = data.cast::<u8>().wrapping_sub(mem::size_of::<Slice>());
    // SAFETY: a slice's header, `lock` first, precedes its data in the pool.
    let lock = unsafe { slice.read() };
    // assert(slice->lock == 1);
    assert_eq!(lock, 1, "an allocated slice");
    // SAFETY: as the caller promises.
    let freed = unsafe { fixed::free(pool, data) };
    assert!(freed, "a live slice of this pool");
}

/// `FixedPool`'s `impl`: its first member.
///
/// # Safety
/// `this` is a constructed `swoole::FixedPool`.
unsafe fn fixed<'a>(this: *const c_void) -> &'a Fixed {
    // SAFETY: as the caller promises; `impl` follows the vtable pointer.
    let pool = unsafe { this.cast::<*const Fixed>().wrapping_add(1).read() };
    // SAFETY: a constructed pool's header.
    unsafe { &*pool }
}

/// `uint32_t FixedPool::get_number_of_spare_slice() const`.
#[must_use]
pub unsafe extern "C" fn swr_fixed_pool_spare(this: *const c_void) -> u32 {
    // SAFETY: `this` is a constructed FixedPool.
    let pool = unsafe { fixed(this) };
    pool.slices().0
}

/// `uint32_t FixedPool::get_number_of_total_slice() const`.
#[must_use]
pub unsafe extern "C" fn swr_fixed_pool_total(this: *const c_void) -> u32 {
    // SAFETY: `this` is a constructed FixedPool.
    let pool = unsafe { fixed(this) };
    pool.slices().1
}

/// `uint32_t FixedPool::get_slice_size() const`.
#[must_use]
pub unsafe extern "C" fn swr_fixed_pool_slice_size(this: *const c_void) -> u32 {
    // SAFETY: `this` is a constructed FixedPool.
    let pool = unsafe { fixed(this) };
    pool.slice_size
}

/// `void FixedPool::debug(int max_lines) const`: the slices on stdout.
pub unsafe extern "C" fn swr_fixed_pool_debug(this: *const c_void, max_lines: c_int) {
    // SAFETY: `this` is a constructed FixedPool.
    let pool = unsafe { fixed(this) };
    let mut cursor = DebugCursor::default();
    // SAFETY: the format takes one string.
    unsafe {
        libc::printf(
            c"===============================%s=================================\n".as_ptr(),
            c"debug".as_ptr(),
        )
    };
    // SAFETY: `pool` is live.
    while let Some(record) = unsafe { fixed::debug_step(pool, &mut cursor, max_lines) } {
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

/// The pool in `slot`. In a process forked from the one that made a shared pool, a
/// fresh pool replaces it first, without taking the inherited lock a parent thread may
/// have held at the fork, and keeps it.
///
/// # Safety
/// `slot` is a constructed pool's `impl` slot.
unsafe fn current<'a>(slot: *mut *mut GlobalImpl) -> Result<&'a GlobalImpl, i32> {
    // SAFETY: as the caller promises.
    let slot = unsafe { &mut *slot };
    // SAFETY: as above; a pool lives until it is deleted.
    let pool = unsafe { &**slot };
    if pool.creator == 0 || pool.creator == std::process::id() {
        return Ok(pool);
    }
    *slot = GlobalImpl::new(pool.page_size, true, *slot)?;
    // SAFETY: as above.
    Ok(unsafe { &**slot })
}

/// `GlobalMemory(pagesize, shared)`.
///
/// # Panics
/// Upstream's assertion: pages are at least `SW_MIN_PAGE_SIZE`.
///
/// # Safety
/// `output` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_global_memory_new(
    pagesize: u32,
    shared: bool,
    output: *mut *mut GlobalImpl,
) -> i32 {
    // assert(pagesize >= SW_MIN_PAGE_SIZE);
    assert!(
        pagesize >= MIN_PAGE_SIZE,
        "a page of at least SW_MIN_PAGE_SIZE"
    );
    match GlobalImpl::new(pagesize, shared, ptr::null_mut()) {
        Ok(this) => {
            // SAFETY: as the caller promises.
            unsafe { output.write(this) };
            0
        }
        Err(status) => status,
    }
}

/// `GlobalMemory::~GlobalMemory()`: frees every page, then the registry.
///
/// # Panics
/// If the registry kept a page, which draining rules out.
///
/// # Safety
/// `pool` came from `swr_v1_memory_global_memory_new` or `alloc`'s reset, destroyed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_global_memory_delete(pool: *mut GlobalImpl) {
    // SAFETY: as the caller promises; dropping it frees its pages.
    drop(unsafe { Box::from_raw(pool) });
}

/// `void *GlobalMemory::alloc(uint32_t size)`, from this process's pool (`current`).
///
/// # Safety
/// `pool` is a constructed pool's `impl` slot; `output` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_global_memory_alloc(
    pool: *mut *mut GlobalImpl,
    size: u32,
    output: *mut *mut c_void,
) -> i32 {
    // SAFETY: as the caller promises.
    let (status, block) = match unsafe { current(pool) } {
        Ok(current) => (0, alloc(&mut current.pages(), size)),
        Err(status) => (status, ptr::null_mut()),
    };
    // SAFETY: as the caller promises.
    unsafe { output.write(block) };
    status
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

/// A `GlobalMemory`'s `impl` slot, its one member.
fn slot(this: *const c_void) -> *mut *mut GlobalImpl {
    this.cast::<*mut GlobalImpl>().cast_mut()
}

upstream! {
    "_ZNK6swoole12GlobalMemory8capacityEv", "_ZNK6swoole12GlobalMemory8capacityEv" =>
    /// `size_t GlobalMemory::capacity() const`.
    #[must_use]
    pub unsafe extern "C" fn swr_global_memory_capacity(this: *const c_void) -> usize {
        // SAFETY: `this` is a constructed GlobalMemory; zero if a fresh pool can't be had.
        unsafe { current(slot(this)) }.map_or(0, |pool| pool.pages().capacity())
    }
    "_ZNK6swoole12GlobalMemory15get_memory_sizeEv", "_ZNK6swoole12GlobalMemory15get_memory_sizeEv" =>
    /// `size_t GlobalMemory::get_memory_size() const`.
    #[must_use]
    pub unsafe extern "C" fn swr_global_memory_size(this: *const c_void) -> usize {
        // SAFETY: as above.
        unsafe { current(slot(this)) }.map_or(0, |pool| pool.pages().memory_size())
    }
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
    #[expect(
        clippy::not_unsafe_ptr_arg_deref,
        reason = "upstream's C signature; callers pass a live sw_shm_malloc payload"
    )]
    pub extern "C" fn sw_shm_protect(payload: *mut c_void, flags: c_int) -> c_int {
        // SAFETY: `payload` is a live mapping's payload.
        let (base, total) = unsafe { extent(payload) };
        // SAFETY: the mapping its header describes.
        unsafe { libc::mprotect(base, total, flags) }
    }
}

/// `void *sw_shm_realloc(void *ptr, size_t new_size)`: copies what fits.
#[must_use]
#[expect(
    clippy::not_unsafe_ptr_arg_deref,
    reason = "upstream's C signature; callers pass a live sw_shm_malloc payload"
)]
pub extern "C" fn sw_shm_realloc(payload: *mut c_void, size: usize) -> *mut c_void {
    let fresh = sw_shm_malloc(size);
    if fresh.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: `payload` is a live mapping's payload.
    let old = unsafe { extent(payload) }.1;
    // SAFETY: `fresh` is a new one.
    let new = unsafe { extent(fresh) }.1;
    // SAFETY: two disjoint live payloads, each `total - 8` bytes.
    unsafe { ptr::copy_nonoverlapping(payload.cast::<u8>(), fresh.cast::<u8>(), old.min(new) - 8) };
    unmap(payload);
    fresh
}

// ---------------------------------------------------------------- Table

/// `SwooleG.memory_pool`: a `GlobalMemory` (`native/memory/swoole_memory.h`), whose
/// one member is its `impl` slot. Its `free` is a no-op, so nothing calls it.
pub mod pool {
    use super::{
        GlobalImpl, swr_v1_memory_global_memory_alloc, swr_v1_memory_global_memory_delete,
    };
    use std::ffi::c_void;
    use std::ptr;

    /// `new GlobalMemory(SW_GLOBAL_MEMORY_PAGESIZE, true)`, `swoole_init`'s pool. Where
    /// upstream's constructor throws, the process aborts.
    #[must_use]
    pub fn new() -> *mut c_void {
        let page = swoole_rs_corebase::constants::SW_GLOBAL_MEMORY_PAGESIZE;
        let Ok(pool) = GlobalImpl::new(page, true, ptr::null_mut()) else {
            std::process::abort();
        };
        Box::into_raw(Box::new(pool)).cast()
    }

    /// `delete SwooleG.memory_pool`.
    pub fn delete() {
        let pool = swoole_rs_corebase::swr_v1_core_memory_pool().cast::<*mut GlobalImpl>();
        if !pool.is_null() {
            // SAFETY: `new`'s, deleted once, by `swoole_clean`.
            let pool = unsafe { Box::from_raw(pool) };
            // SAFETY: its live pages, which only it frees.
            unsafe { swr_v1_memory_global_memory_delete(*pool) };
        }
    }

    /// Without futex, waiters register in a Rust registry in shared memory, from the
    /// pool `swoole_init` just made: a fatal error when it can't be had.
    pub fn wait_registry_init() {
        let size = u32::try_from(swoole_rs_utilities::swr_v1_utils_wait_registry_size());
        // SAFETY: `swoole_init` has made the pool.
        let registry = unsafe { alloc(size.unwrap_or(u32::MAX)) };
        // SAFETY: null or that many shared bytes, for the program's life.
        if !unsafe { swoole_rs_utilities::swr_v1_utils_wait_registry_init(registry) } {
            let message = c"failed to allocate atomic wait registry";
            // SAFETY: `swoole_init`'s hook exits; no frame here owns anything.
            unsafe { swoole_rs_corebase::fatal_error(super::MALLOC_FAIL, message) };
        }
    }

    /// `sw_mem_pool()->alloc(size)`.
    ///
    /// # Safety
    /// `SwooleG.memory_pool` is a live pool.
    #[must_use]
    pub unsafe fn alloc(size: u32) -> *mut c_void {
        let pool = swoole_rs_corebase::swr_v1_core_memory_pool().cast();
        let mut block = ptr::null_mut();
        // SAFETY: as the caller promises, `pool` is a live pool's `impl` slot.
        if unsafe { swr_v1_memory_global_memory_alloc(pool, size, &raw mut block) } != 0 {
            // A forked process's fresh pool failed: upstream's `alloc` throws.
            std::process::abort();
        }
        block
    }
}

unsafe extern "C" fn table_error(code: c_int) {
    set_last_error(code);
}

/// `table_warning(message, length)`: a warning whose text starts with its function,
/// `Name(): `.
unsafe extern "C" fn table_warning(message: *const c_char, length: usize) {
    // SAFETY: the table passes `length` readable bytes.
    let text = unsafe { std::slice::from_raw_parts(message.cast::<u8>(), length) };
    let text = String::from_utf8_lossy(text);
    let (origin, rest) = text.split_once("(): ").unwrap_or(("", &text));
    let origin = std::ffi::CString::new(origin).unwrap_or_default();
    log::warning(&origin, format_args!("{rest}"));
}

/// `Table::make(rows_size, conflict_proportion)`: the header comes from the global
/// memory pool, so forked workers share it.
///
/// # Safety
/// `swoole_init` made the global pool.
#[must_use]
pub unsafe fn swr_table_make(rows: u32, conflict: f32) -> *mut Table {
    #[expect(clippy::cast_possible_truncation, reason = "a sub-page header")]
    // SAFETY: as the caller promises.
    let table = unsafe { pool::alloc(mem::size_of::<Table>() as u32) }.cast::<Table>();
    if table.is_null() {
        return table;
    }
    let hash = swoole_rs_utilities::misc::swoole_hash_php;
    // SAFETY: the pool's blocks are 8-aligned; `size_of::<Table>()` writable bytes.
    if unsafe { !Table::init(table, rows, conflict, hash, table_warning, table_error) } {
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

#[cfg(test)]
#[path = "../../../tests/modules/memory/members.rs"]
mod tests;
