// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/memory/{fixed_pool,ring_buffer,global_memory,
// shared_memory,table}.cc (Swoole contributors).
//! The upstream members of `FixedPool`, `RingBuffer`, `GlobalMemory`, the `sw_shm_*`
//! functions and `Table`'s shared-memory lifecycle. `include/swoole_memory.h` keeps the
//! constructors, which throw, and the virtual members, whose vtables every user emits;
//! they call the `swr_v1_memory_*_pool` functions below. A constructor returns 0, -1 for
//! upstream's `Exception(SW_ERROR_INVALID_PARAMS)` or -2 for `std::bad_alloc`.
//!
//! Each `// assert(...)` comment is the upstream assertion the `assert!` under it
//! carries; tests/modules/memory/gate.py checks every one is still here.

use crate::fixed::{DebugCursor, DebugRecord, Fixed, Layout, Slice};
use crate::ring::Storage;
use crate::table::Table;
use crate::{
    swr_v1_memory_arena_lifecycle, swr_v1_memory_fixed_alloc, swr_v1_memory_fixed_construct,
    swr_v1_memory_fixed_debug_step, swr_v1_memory_fixed_free, swr_v1_memory_fixed_layout,
    swr_v1_memory_global_add_page, swr_v1_memory_global_alloc, swr_v1_memory_global_capacity,
    swr_v1_memory_global_memory_size, swr_v1_memory_global_open, swr_v1_memory_global_release,
    swr_v1_memory_global_step, swr_v1_memory_mapping_extent, swr_v1_memory_mapping_initialize,
    swr_v1_memory_mapping_product, swr_v1_memory_mapping_relocate, swr_v1_memory_mapping_size,
    swr_v1_memory_ring_construct, swr_v1_memory_ring_free, swr_v1_memory_ring_layout,
    swr_v1_memory_ring_storage_alloc,
};
use std::ffi::{c_char, c_int, c_void};
use std::sync::{Mutex, PoisonError};
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
fn arena(shared: bool, size: usize) -> *mut c_void {
    if swr_v1_memory_arena_lifecycle(u32::from(shared), 1, 0) == 2 {
        sw_shm_malloc(size)
    } else {
        sw_malloc(size)
    }
}

/// Frees what `arena` returned, or what an owned pool's lifecycle says to free.
///
/// # Safety
/// `memory` is live and came from the allocator `action` names.
unsafe fn release(action: i32, memory: *mut c_void) {
    match action {
        2 | 4 => sw_shm_free(memory),
        // SAFETY: as the caller promises.
        1 | 3 => unsafe { sw_free(memory) },
        _ => {}
    }
}

// ---------------------------------------------------------------- FixedPool

/// `FixedPool(slice_num, slice_size, shared)`: an owned arena.
///
/// # Safety
/// `output` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_fixed_pool_new(
    slices: u32,
    width: u32,
    shared: bool,
    output: *mut *mut Fixed,
) -> i32 {
    let mut plan = Layout::default();
    // SAFETY: `plan` is a writable local.
    if unsafe { swr_v1_memory_fixed_layout(slices, width, 0, 1, &raw mut plan) } != 0 {
        return INVALID;
    }
    let memory = arena(shared, plan.total);
    if memory.is_null() {
        return NO_MEMORY;
    }
    // SAFETY: a fresh arena of `plan.total` bytes.
    if unsafe { swr_v1_memory_fixed_construct(memory.cast(), &raw const plan, shared.into(), 1) }
        != 0
    {
        // SAFETY: `arena` just allocated it.
        unsafe { release(swr_v1_memory_arena_lifecycle(shared.into(), 1, 0), memory) };
        return INVALID;
    }
    // SAFETY: as the caller promises.
    unsafe { output.write(memory.cast()) };
    0
}

/// `FixedPool(slice_size, memory, size, shared)`: a pool over the caller's `memory`.
///
/// # Safety
/// `memory` is null or `size` writable bytes the pool may use until it is destroyed;
/// `output` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_fixed_pool_borrow(
    width: u32,
    memory: *mut c_void,
    size: usize,
    shared: bool,
    output: *mut *mut Fixed,
) -> i32 {
    let mut plan = Layout::default();
    // SAFETY: `plan` is a writable local.
    if unsafe { swr_v1_memory_fixed_layout(0, width, size, 0, &raw mut plan) } != 0
        || memory.is_null()
        || !memory.addr().is_multiple_of(mem::align_of::<Fixed>())
        || memory.addr().checked_add(plan.total).is_none()
    {
        return INVALID;
    }
    // SAFETY: as the caller promises, `size` (at least `plan.total`) writable bytes.
    if unsafe { swr_v1_memory_fixed_construct(memory.cast(), &raw const plan, shared.into(), 0) }
        != 0
    {
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_fixed_pool_delete(pool: *mut Fixed) {
    // SAFETY: as the caller promises.
    let header = unsafe { &*pool };
    let (shared, owned) = (header.shared, header.allocated);
    let action = swr_v1_memory_arena_lifecycle(shared.into(), owned.into(), 1);
    // SAFETY: an owned pool's arena came from the allocator its lifecycle names.
    unsafe { release(action, pool.cast()) };
}

/// `void *FixedPool::alloc(uint32_t size)`: the slice width was fixed at construction.
///
/// # Panics
/// Upstream's assertion: an allocation fails only when no slice is spare.
///
/// # Safety
/// `pool` is a constructed pool; allocation is serialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_fixed_pool_alloc(pool: *mut Fixed) -> *mut c_void {
    // SAFETY: as the caller promises.
    let result = unsafe { swr_v1_memory_fixed_alloc(pool) };
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_fixed_pool_free(pool: *mut Fixed, data: *mut c_void) {
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
    let status = unsafe { swr_v1_memory_fixed_free(pool, data) };
    assert_eq!(status, 0, "a live slice of this pool");
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

upstream! {
    "_ZNK6swoole9FixedPool25get_number_of_spare_sliceEv", "_ZNK6swoole9FixedPool25get_number_of_spare_sliceEv" =>
    /// `uint32_t FixedPool::get_number_of_spare_slice() const`.
    #[must_use]
    pub unsafe extern "C" fn swr_fixed_pool_spare(this: *const c_void) -> u32 {
        // SAFETY: `this` is a constructed FixedPool.
        let pool = unsafe { fixed(this) };
        pool.slices().0
    }
    "_ZNK6swoole9FixedPool25get_number_of_total_sliceEv", "_ZNK6swoole9FixedPool25get_number_of_total_sliceEv" =>
    /// `uint32_t FixedPool::get_number_of_total_slice() const`.
    #[must_use]
    pub unsafe extern "C" fn swr_fixed_pool_total(this: *const c_void) -> u32 {
        // SAFETY: `this` is a constructed FixedPool.
        let pool = unsafe { fixed(this) };
        pool.slices().1
    }
    "_ZNK6swoole9FixedPool14get_slice_sizeEv", "_ZNK6swoole9FixedPool14get_slice_sizeEv" =>
    /// `uint32_t FixedPool::get_slice_size() const`.
    #[must_use]
    pub unsafe extern "C" fn swr_fixed_pool_slice_size(this: *const c_void) -> u32 {
        // SAFETY: `this` is a constructed FixedPool.
        let pool = unsafe { fixed(this) };
        pool.slice_size
    }
    "_ZN6swoole9FixedPool19sizeof_struct_sliceEv", "_ZN6swoole9FixedPool19sizeof_struct_sliceEv" =>
    /// `static size_t FixedPool::sizeof_struct_slice()`.
    #[must_use]
    pub extern "C" fn swr_fixed_pool_sizeof_slice() -> usize {
        mem::size_of::<Slice>()
    }
    "_ZN6swoole9FixedPool18sizeof_struct_implEv", "_ZN6swoole9FixedPool18sizeof_struct_implEv" =>
    /// `static size_t FixedPool::sizeof_struct_impl()`.
    #[must_use]
    pub extern "C" fn swr_fixed_pool_sizeof_impl() -> usize {
        mem::size_of::<Fixed>()
    }
    "_ZNK6swoole9FixedPool5debugEi", "_ZNK6swoole9FixedPool5debugEi" =>
    /// `void FixedPool::debug(int max_lines) const`: the slices on stdout.
    pub unsafe extern "C" fn swr_fixed_pool_debug(this: *const c_void, max_lines: c_int) {
        // SAFETY: `this` is a constructed FixedPool.
        let pool = ptr::from_ref(unsafe { fixed(this) });
        let (mut cursor, mut record) = (DebugCursor::default(), DebugRecord::default());
        // SAFETY: the format takes one string.
        unsafe {
            libc::printf(
                c"===============================%s=================================\n".as_ptr(),
                c"debug".as_ptr(),
            )
        };
        // SAFETY: `pool` is live; `cursor` and `record` are writable locals.
        while unsafe { swr_v1_memory_fixed_debug_step(pool, &raw mut cursor, max_lines, &raw mut record) } == 1 {
            if record.self_link != 0 {
                // SAFETY: a literal format with no arguments.
                unsafe { libc::printf(c"-------------------@@@@@@@@@@@@@@@@@@@@@@----------------\n".as_ptr()) };
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

// ---------------------------------------------------------------- RingBuffer

/// `RingBuffer(size, shared)`.
///
/// # Safety
/// `output` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_ring_buffer_new(
    size: u32,
    shared: bool,
    output: *mut *mut Storage,
) -> i32 {
    let (mut total, mut payload) = (0, 0);
    // SAFETY: both are writable locals.
    if unsafe { swr_v1_memory_ring_layout(size, &raw mut total, &raw mut payload) } != 0 {
        return INVALID;
    }
    let memory = arena(shared, total as usize);
    if memory.is_null() {
        return NO_MEMORY;
    }
    // SAFETY: a fresh arena of `total` bytes.
    if unsafe { swr_v1_memory_ring_construct(memory.cast(), total, shared.into()) } != 0 {
        // SAFETY: `arena` just allocated it.
        unsafe { release(swr_v1_memory_arena_lifecycle(shared.into(), 1, 0), memory) };
        return INVALID;
    }
    // SAFETY: as the caller promises.
    unsafe { output.write(memory.cast()) };
    0
}

/// `RingBuffer::~RingBuffer()`.
///
/// # Safety
/// `ring` is a constructed ring, destroyed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_ring_buffer_delete(ring: *mut Storage) {
    // SAFETY: as the caller promises.
    let shared = unsafe { &*ring }.shared;
    let action = swr_v1_memory_arena_lifecycle(shared.into(), 1, 1);
    // SAFETY: the ring's arena came from the allocator its lifecycle names.
    unsafe { release(action, ring.cast()) };
}

/// `void *RingBuffer::alloc(uint32_t size)`.
///
/// # Panics
/// Upstream's assertion: `size` is positive.
///
/// # Safety
/// `ring` is a constructed ring, allocated from by one producer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_ring_buffer_alloc(
    ring: *mut Storage,
    size: u32,
) -> *mut c_void {
    // assert(size > 0);
    assert!(size > 0, "a non-empty record");
    // SAFETY: as the caller promises.
    unsafe { swr_v1_memory_ring_storage_alloc(ring, size) }
}

/// `void RingBuffer::free(void *ptr)`.
///
/// # Panics
/// Upstream's assertions: `data` is an allocated record of the ring.
///
/// # Safety
/// `ring` is a constructed ring and `data` a live record of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_ring_buffer_free(ring: *mut Storage, data: *mut c_void) {
    // Consumers share the header, and only touch `free_count`, atomically.
    let free_count = ring
        .wrapping_byte_add(mem::offset_of!(Storage, free_count))
        .cast::<u32>();
    // SAFETY: as the caller promises; `memory` and `size` don't change after construction.
    let memory = unsafe { ring.cast::<*mut u8>().read() };
    // SAFETY: as above.
    let size = unsafe {
        ring.wrapping_byte_add(mem::offset_of!(Storage, size))
            .cast::<u32>()
            .read()
    };
    // assert(ptr >= impl->memory);
    assert!(data.addr() >= memory.addr(), "a record of this ring");
    // assert(static_cast<char *>(ptr) <= static_cast<char *>(impl->memory) + impl->size);
    assert!(
        data.addr() <= memory.addr() + size as usize,
        "a record of this ring"
    );
    let item = data.cast::<u16>().wrapping_sub(4);
    // SAFETY: a record's 8-byte header, `uint16_t lock` first, precedes its data.
    let lock = unsafe { item.read() };
    // assert(item->lock == 1);
    assert_eq!(lock, 1, "an allocated record");
    // SAFETY: as the caller promises.
    let status = unsafe { swr_v1_memory_ring_free(memory.cast(), size, free_count, data) };
    assert_eq!(status, 0, "a live record of this ring");
}

// ---------------------------------------------------------------- GlobalMemory

/// `GlobalMemoryImpl`: the page registry and the lock `alloc` holds.
pub struct GlobalImpl {
    lock: Mutex<()>,
    registry: *mut c_void,
}

impl GlobalImpl {
    /// `new GlobalMemoryImpl(size, shared)`, with its first page.
    fn new(size: u32, shared: bool) -> Result<*mut Self, i32> {
        let mut registry = ptr::null_mut();
        let page = swoole_rs_corebase::swr_v1_core_pagesize();
        // SAFETY: getpid has no preconditions.
        let pid = i64::from(unsafe { libc::getpid() });
        // SAFETY: `registry` is a writable local.
        let status =
            unsafe { swr_v1_memory_global_open(size, page, shared.into(), pid, &raw mut registry) };
        match status {
            0 => {}
            -2 => return Err(INVALID),
            _ => return Err(NO_MEMORY),
        }
        // SAFETY: `registry` is the live registry `open` made.
        if unsafe { !new_page(registry) } {
            // SAFETY: it has no pages to drain.
            unsafe { swr_v1_memory_global_release(registry) };
            return Err(NO_MEMORY);
        }
        Ok(Box::into_raw(Box::new(Self {
            lock: Mutex::new(()),
            registry,
        })))
    }
}

/// `GlobalMemoryImpl::new_page()`: one more page in `registry`.
///
/// # Safety
/// `registry` is live.
unsafe fn new_page(registry: *mut c_void) -> bool {
    let mut effect = crate::global::GlobalEffect::default();
    // SAFETY: `registry` is live; `effect` is a writable local.
    if unsafe { swr_v1_memory_global_step(registry, 0, 0, 0, &raw mut effect) } != 0 {
        return false;
    }
    let shared = effect.action == 2;
    let page = arena(shared, effect.page_size as usize);
    if page.is_null() {
        return false;
    }
    // SAFETY: `registry` is live and `page` a fresh page of its size.
    if unsafe { swr_v1_memory_global_add_page(registry, page) } != 0 {
        // SAFETY: `arena` just allocated it.
        unsafe { release(if shared { 2 } else { 1 }, page) };
        return false;
    }
    true
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
    match GlobalImpl::new(pagesize, shared) {
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
    // SAFETY: as the caller promises.
    let this = unsafe { Box::from_raw(pool) };
    let mut effect = crate::global::GlobalEffect::default();
    // SAFETY: the registry is live; `effect` is a writable local.
    while unsafe { swr_v1_memory_global_step(this.registry, 2, 0, 0, &raw mut effect) } == 0
        && effect.action != 0
    {
        // SAFETY: each drained page came from the allocator its action names.
        unsafe { release(if effect.action == 4 { 2 } else { 1 }, effect.page) };
    }
    // SAFETY: the registry is drained.
    let status = unsafe { swr_v1_memory_global_release(this.registry) };
    assert_eq!(status, 0, "a drained registry");
}

/// `void *GlobalMemory::alloc(uint32_t size)`. In a process forked from the one that
/// made a shared pool, `*pool` becomes a fresh pool first: the inherited pages and
/// lock stay as they are.
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
    unsafe { output.write(ptr::null_mut()) };
    // SAFETY: as the caller promises; the lock outlives the guard, since a reset
    // keeps the old impl.
    let current = unsafe { pool.read() };
    // SAFETY: as above.
    let current = unsafe { &*current };
    let _guard = current.lock.lock().unwrap_or_else(PoisonError::into_inner);
    let mut effect = crate::global::GlobalEffect::default();
    // SAFETY: getpid has no preconditions.
    let pid = i64::from(unsafe { libc::getpid() });
    // SAFETY: the registry is live; `effect` is a writable local.
    if unsafe { swr_v1_memory_global_step(current.registry, 1, pid, size, &raw mut effect) } != 0 {
        log::warning(
            c"GlobalMemory::alloc",
            format_args!(
                "failed to alloc {} bytes, exceed the maximum size[{}]",
                effect.aligned.cast_signed(),
                effect.page_size.cast_signed()
            ),
        );
        return 0;
    }
    let mut registry = current.registry;
    if effect.action == 5 {
        match GlobalImpl::new(effect.page_size, true) {
            Ok(fresh) => {
                // SAFETY: as the caller promises; `fresh` is live.
                unsafe { pool.write(fresh) };
                // SAFETY: as above.
                registry = unsafe { (*fresh).registry };
            }
            Err(status) => return status,
        }
    }
    let mut result = ptr::null_mut();
    // SAFETY: the registry is live; `result` is a writable local.
    let mut status = unsafe { swr_v1_memory_global_alloc(registry, size, &raw mut result) };
    if status == 1 {
        // SAFETY: the registry is live.
        if unsafe { !new_page(registry) } {
            log::warning(c"GlobalMemory::alloc", format_args!("alloc memory error"));
            return 0;
        }
        // SAFETY: as above.
        status = unsafe { swr_v1_memory_global_alloc(registry, size, &raw mut result) };
    }
    if status == 0 {
        // SAFETY: as the caller promises.
        unsafe { output.write(result) };
    }
    0
}

/// `GlobalMemory`'s `impl`.
///
/// # Safety
/// `this` is a constructed `swoole::GlobalMemory`.
unsafe fn global<'a>(this: *const c_void) -> &'a GlobalImpl {
    // SAFETY: as the caller promises; `impl` follows the vtable pointer.
    let pool = unsafe { this.cast::<*const GlobalImpl>().wrapping_add(1).read() };
    // SAFETY: a constructed pool's impl.
    unsafe { &*pool }
}

upstream! {
    "_ZNK6swoole12GlobalMemory8capacityEv", "_ZNK6swoole12GlobalMemory8capacityEv" =>
    /// `size_t GlobalMemory::capacity() const`.
    #[must_use]
    pub unsafe extern "C" fn swr_global_memory_capacity(this: *const c_void) -> usize {
        // SAFETY: `this` is a constructed GlobalMemory.
        let registry = unsafe { global(this) }.registry;
        // SAFETY: its live registry.
        unsafe { swr_v1_memory_global_capacity(registry) }
    }
    "_ZNK6swoole12GlobalMemory15get_memory_sizeEv", "_ZNK6swoole12GlobalMemory15get_memory_sizeEv" =>
    /// `size_t GlobalMemory::get_memory_size() const`.
    #[must_use]
    pub unsafe extern "C" fn swr_global_memory_size(this: *const c_void) -> usize {
        // SAFETY: `this` is a constructed GlobalMemory.
        let registry = unsafe { global(this) }.registry;
        // SAFETY: its live registry.
        unsafe { swr_v1_memory_global_memory_size(registry) }
    }
}

// ---------------------------------------------------------------- sw_shm_*

/// `SharedMemory::free`: unmaps the mapping `payload` heads.
fn unmap(payload: *mut c_void) {
    let mut extent = crate::mapping::Extent::default();
    // SAFETY: `extent` is a writable local; the caller passes a live mapping's payload.
    let status = unsafe { swr_v1_memory_mapping_extent(payload.cast(), &raw mut extent) };
    assert!(status == 0, "a live shared mapping");
    // SAFETY: the mapping `extent` read from its header.
    if unsafe { libc::munmap(extent.base, extent.total) } < 0 {
        log::sys_warning(
            c"SharedMemory::free",
            format_args!("munmap({:p}, {}) failed", extent.base, extent.total),
        );
    }
}

upstream! {
    "_Z13sw_shm_mallocm", "_Z13sw_shm_mallocm" =>
    /// `void *sw_shm_malloc(size_t size)`: an anonymous shared mapping whose size heads it.
    #[must_use]
    pub extern "C" fn sw_shm_malloc(size: usize) -> *mut c_void {
        let mut total = 0;
        // SAFETY: `total` is a writable local.
        if unsafe { swr_v1_memory_mapping_size(size, &raw mut total) } != 0 {
            return ptr::null_mut();
        }
        let flags = libc::MAP_SHARED | libc::MAP_ANONYMOUS;
        // SAFETY: a fresh anonymous mapping.
        let memory = unsafe { libc::mmap(ptr::null_mut(), total, libc::PROT_READ | libc::PROT_WRITE, flags, -1, 0) };
        if memory == libc::MAP_FAILED {
            log::sys_warning(c"SharedMemory::alloc", format_args!("mmap({total}) failed"));
            return ptr::null_mut();
        }
        // SAFETY: `memory` is a fresh, page-aligned mapping of `total` bytes.
        unsafe { swr_v1_memory_mapping_initialize(memory.cast(), total) }.cast()
    }
    "_Z13sw_shm_callocmm", "_Z13sw_shm_callocmm" =>
    /// `void *sw_shm_calloc(size_t num, size_t size)`: mappings start zeroed.
    #[must_use]
    pub extern "C" fn sw_shm_calloc(count: usize, size: usize) -> *mut c_void {
        let mut requested = 0;
        // SAFETY: `requested` is a writable local.
        if unsafe { swr_v1_memory_mapping_product(count, size, &raw mut requested) } != 0 {
            return ptr::null_mut();
        }
        sw_shm_malloc(requested)
    }
    "_Z11sw_shm_freePv", "_Z11sw_shm_freePv" =>
    /// `void sw_shm_free(void *ptr)`.
    pub extern "C" fn sw_shm_free(payload: *mut c_void) {
        unmap(payload);
    }
    "_Z14sw_shm_protectPvi", "_Z14sw_shm_protectPvi" =>
    /// `int sw_shm_protect(void *ptr, int flags)`.
    pub extern "C" fn sw_shm_protect(payload: *mut c_void, flags: c_int) -> c_int {
        let mut extent = crate::mapping::Extent::default();
        // SAFETY: `extent` is a writable local; a bad header is rejected.
        if unsafe { swr_v1_memory_mapping_extent(payload.cast(), &raw mut extent) } != 0 {
            set_errno(libc::EINVAL);
            return -1;
        }
        // SAFETY: the mapping `extent` read from its header.
        unsafe { libc::mprotect(extent.base, extent.total, flags) }
    }
    "_Z14sw_shm_reallocPvm", "_Z14sw_shm_reallocPvm" =>
    /// `void *sw_shm_realloc(void *ptr, size_t new_size)`: copies what fits.
    #[must_use]
    pub extern "C" fn sw_shm_realloc(payload: *mut c_void, size: usize) -> *mut c_void {
        let fresh = sw_shm_malloc(size);
        if fresh.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: both are live mapping payloads; relocate rejects overlap.
        if unsafe { swr_v1_memory_mapping_relocate(payload.cast(), fresh.cast()) } != 0 {
            sw_shm_free(fresh);
            return ptr::null_mut();
        }
        unmap(payload);
        fresh
    }
}

/// Sets this thread's errno.
fn set_errno(value: c_int) {
    #[cfg(target_os = "linux")]
    // SAFETY: no preconditions.
    let errno = unsafe { libc::__errno_location() };
    #[cfg(target_os = "macos")]
    // SAFETY: no preconditions.
    let errno = unsafe { libc::__error() };
    // SAFETY: this thread's errno.
    unsafe { errno.write(value) };
}

// ---------------------------------------------------------------- Table

/// `sw_mem_pool()->alloc(size)` and `->free(ptr)`: virtual calls through the pool's
/// Itanium vtable, whose slots are the two destructors, `alloc`, then `free`.
pub mod pool {
    use std::ffi::c_void;

    type Alloc = unsafe extern "C" fn(this: *mut c_void, size: u32) -> *mut c_void;
    type Free = unsafe extern "C" fn(this: *mut c_void, ptr: *mut c_void);

    /// The pool and its vtable.
    ///
    /// # Safety
    /// `SwooleG.memory_pool` is a live `swoole::MemoryPool`.
    unsafe fn pool() -> (*mut c_void, *const c_void) {
        let pool = swoole_rs_corebase::swr_v1_core_memory_pool();
        // SAFETY: as the caller promises; a polymorphic object starts with its vptr.
        (pool, unsafe { pool.cast::<*const c_void>().read() })
    }

    /// # Safety
    /// As `pool`.
    #[must_use]
    pub unsafe fn alloc(size: u32) -> *mut c_void {
        // SAFETY: as the caller promises.
        let (pool, vtable) = unsafe { pool() };
        // SAFETY: slot 2 is `MemoryPool::alloc`'s override.
        let alloc = unsafe { vtable.cast::<Alloc>().wrapping_add(2).read() };
        // SAFETY: a virtual call on the live pool.
        unsafe { alloc(pool, size) }
    }

    /// # Safety
    /// As `pool`; `ptr` came from `alloc`.
    pub unsafe fn free(ptr: *mut c_void) {
        // SAFETY: as the caller promises.
        let (pool, vtable) = unsafe { pool() };
        // SAFETY: slot 3 is `MemoryPool::free`'s override.
        let free = unsafe { vtable.cast::<Free>().wrapping_add(3).read() };
        // SAFETY: a virtual call on the live pool.
        unsafe { free(pool, ptr) };
    }
}

unsafe extern "C" fn table_error(code: c_int) {
    set_last_error(code);
}

upstream! {
    "_ZN6swoole13table_warningEPKcm", "_ZN6swoole13table_warningEPKcm" =>
    /// `void table_warning(const char *message, size_t length)`: a warning whose text
    /// starts with its function, `Name(): `.
    pub unsafe extern "C" fn table_warning(message: *const c_char, length: usize) {
        // SAFETY: the table passes `length` readable bytes.
        let text = unsafe { std::slice::from_raw_parts(message.cast::<u8>(), length) };
        let text = String::from_utf8_lossy(text);
        let (origin, rest) = text.split_once("(): ").unwrap_or(("", &text));
        let origin = std::ffi::CString::new(origin).unwrap_or_default();
        log::warning(&origin, format_args!("{rest}"));
    }
    "_ZN6swoole5Table4makeEjf", "_ZN6swoole5Table4makeEjf" =>
    /// `static Table *Table::make(uint32_t rows_size, float conflict_proportion)`: the
    /// header comes from the global memory pool, so forked workers share it.
    #[must_use]
    pub unsafe extern "C" fn swr_table_make(rows: u32, conflict: f32) -> *mut Table {
        #[expect(clippy::cast_possible_truncation, reason = "a sub-page header")]
        // SAFETY: `swoole_init` made the global pool.
        let table = unsafe { pool::alloc(crate::table::swr_v1_memory_table_header_size() as u32) }.cast::<Table>();
        if table.is_null() {
            return table;
        }
        let hash = swoole_rs_utilities::misc::swoole_hash_php;
        // SAFETY: `header_size()` writable bytes.
        if unsafe { !crate::table::swr_v1_memory_table_init(table, rows, conflict, hash, table_warning, table_error) } {
            // SAFETY: from the pool above.
            unsafe { pool::free(table.cast()) };
            return ptr::null_mut();
        }
        table
    }
    "_ZN6swoole5Table6createEv", "_ZN6swoole5Table6createEv" =>
    /// `bool Table::create()`: lays the table out in a fresh shared arena.
    #[must_use]
    pub unsafe extern "C" fn swr_table_create(this: *mut Table) -> bool {
        // SAFETY: `this` is a made table.
        let size = unsafe { crate::table::swr_v1_memory_table_arena_size(this) };
        if size == 0 {
            return false;
        }
        let arena = sw_shm_malloc(size);
        if arena.is_null() {
            return false;
        }
        // SAFETY: a fresh arena of `size` bytes.
        if unsafe { !crate::table::swr_v1_memory_table_create(this, arena) } {
            sw_shm_free(arena);
            return false;
        }
        true
    }
    "_ZN6swoole5Table7destroyEv", "_ZN6swoole5Table7destroyEv" =>
    /// `void Table::destroy()`: frees the arena, then the header.
    pub unsafe extern "C" fn swr_table_destroy(this: *mut Table) {
        // SAFETY: `this` is a made table, destroyed once.
        let arena = unsafe { crate::table::swr_v1_memory_table_destroy(this) };
        if !arena.is_null() {
            sw_shm_free(arena);
        }
        // SAFETY: from the pool `make` used.
        unsafe { pool::free(this.cast()) };
    }
}

#[cfg(test)]
#[path = "../../../tests/modules/memory/members.rs"]
mod tests;
