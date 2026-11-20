use super::*;
use crate::memory::fixed::tests::{DebugCursor, debug_step};
use crate::memory::fixed::{self, Fixed, Layout};

/// `SW_ERROR_MALLOC_FAIL`.
const MALLOC_FAIL: c_int = swoole_rs_error::ErrorCode::SW_ERROR_MALLOC_FAIL as c_int;
use crate::log::last_error;
use crate::sys::malloc;

// `swoole::FixedPool`, as upstream's fixed_pool TESTs use it: nothing else constructs a
// pool over an arena of its own (the table lays its collision pool out in its own).

/// An arena of `size` bytes from `sw_shm_malloc` (`shared`) or libc's `malloc`.
fn arena(shared: bool, size: usize) -> *mut c_void {
    if shared {
        sw_shm_malloc(size)
    } else {
        malloc(size)
    }
}

/// Frees what `arena(shared, ..)` returned.
///
/// # Safety
/// `memory` is live and came from `arena(shared, ..)`.
unsafe fn release(shared: bool, memory: *mut c_void) {
    if shared {
        Shm(Cell::new(memory)).free();
    } else {
        // SAFETY: as the caller promises.
        unsafe { libc::free(memory) };
    }
}

/// `swoole::FixedPool` over an owned arena: upstream's header room, then its slices,
/// in memory from `arena(shared, ..)`. `&mut self` serializes allocation and release.
struct FixedPool {
    arena: *mut c_void,
    shared: bool,
    pool: Fixed,
}

impl FixedPool {
    /// `FixedPool(slice_num, slice_size, shared)`: `INVALID` for upstream's
    /// `Exception(SW_ERROR_INVALID_PARAMS)`, `NO_MEMORY` for `std::bad_alloc`.
    ///
    /// # Errors
    /// As above.
    fn new(slices: u32, width: u32, shared: bool) -> Result<Self, i32> {
        let plan = Layout::new(slices, width, 0, true).ok_or(INVALID)?;
        let arena = arena(shared, plan.total);
        if arena.is_null() {
            return Err(NO_MEMORY);
        }
        // SAFETY: a fresh, malloc-aligned arena of `plan.total` bytes.
        let memory = unsafe { std::slice::from_raw_parts_mut(arena.cast(), plan.total) };
        let Some(pool) = fixed::construct(memory, plan) else {
            unsafe { release(shared, arena) };
            return Err(INVALID);
        };
        Ok(Self {
            arena,
            shared,
            pool,
        })
    }

    fn header(&self) -> &Fixed {
        &self.pool
    }

    /// `void *FixedPool::alloc(uint32_t size)`: the slice width was fixed at construction.
    ///
    /// # Panics
    /// Upstream's assertion: an allocation fails only when no slice is spare.
    fn alloc(&mut self) -> *mut c_void {
        // `&mut self` serializes allocation.
        let result = fixed::alloc(self.header());
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
    fn free(&mut self, data: *mut c_void) {
        let memory = self.header().memory;
        let size = self.total() as usize * self.header().stride();
        // assert(ptr > impl->memory && static_cast<char *>(ptr) < static_cast<char *>(impl->memory) + impl->size);
        assert!(
            data.addr() > memory.addr() && data.addr() < memory.addr() + size,
            "a slice of this pool"
        );
        // The same address, from the arena: nothing is read through the caller's pointer.
        let slice = memory.wrapping_add(data.addr() - memory.addr());
        // `free` checks the stride before it reads a slice header, then upstream's
        // `slice->lock == 1`, so a pointer into a slice's (maybe uninitialized) data is
        // refused unread.
        let freed = fixed::free(self.header(), slice.cast());
        assert!(freed, "a live slice of this pool");
    }

    /// `uint32_t FixedPool::get_number_of_spare_slice() const`.
    fn spare(&self) -> u32 {
        self.header().slices().0
    }

    /// `uint32_t FixedPool::get_number_of_total_slice() const`.
    fn total(&self) -> u32 {
        self.header().slices().1
    }

    /// `uint32_t FixedPool::get_slice_size() const`.
    fn slice_size(&self) -> u32 {
        self.header().slice_size
    }

    /// `void FixedPool::debug(int max_lines) const`: the slices on stdout, through C stdio.
    fn debug(&self, max_lines: c_int) {
        let mut cursor = DebugCursor::default();
        crate::sys::print(
            b"===============================debug=================================\n",
        );
        while let Some(record) = debug_step(self.header(), &mut cursor, max_lines) {
            if record.self_link {
                crate::sys::print(b"-------------------@@@@@@@@@@@@@@@@@@@@@@----------------\n");
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
        // SAFETY: the arena came from `arena(shared, ..)` in `new`, freed once.
        unsafe { release(self.shared, self.arena) };
    }
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no PROT_NONE mappings")]
fn shared_mappings_are_zeroed_protected_and_freed() {
    let memory = sw_shm_calloc(4, 25).cast::<u8>();
    assert!(!memory.is_null());
    unsafe {
        assert!((0..100).all(|i| memory.add(i).read() == 0));
        assert!(
            Shm(Cell::new(memory.cast()))
                .protect(libc::PROT_READ)
                .is_ok()
        );
        assert!(
            Shm(Cell::new(memory.cast()))
                .protect(libc::PROT_READ | libc::PROT_WRITE)
                .is_ok()
        );
        memory.write_bytes(7, 100);
        let total = memory
            .cast::<c_void>()
            .wrapping_byte_sub(8)
            .cast::<usize>()
            .read();
        assert_eq!(total, 112);
    }
    Shm(Cell::new(memory.cast())).free();
    assert!(sw_shm_malloc(usize::MAX).is_null());
    assert!(sw_shm_calloc(usize::MAX, 2).is_null());
}

#[test]
fn a_freed_shm_field_is_null_and_freeing_it_again_unmaps_nothing() {
    let mut field = Shm::calloc(2, 8);
    assert!(!field.is_null());
    field.free();
    assert!(field.is_null());
    // A second free, like a shutdown after a failed startup freed the field,
    // reads no header from the unmapped payload.
    field.free();
    assert!(field.is_null());
    assert!(Shm::empty().is_null());
    assert!(Shm::calloc(usize::MAX, 2).is_null());
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no fork or shared mappings")]
fn filling_a_mapped_field_keeps_its_payload_and_unmaps_the_new_mapping() {
    // Isolate the address space: another test's mapping must not reuse the new address.
    let child = unsafe { libc::fork() };
    assert!(child >= 0);
    if child == 0 {
        let mut field = Shm::empty();
        let first = Shm::calloc(2, 8);
        unsafe { first.as_ptr().cast::<u32>().write(42) };
        let filled = field.fill(first);
        let second = Shm::calloc(4, 8);
        let new_header = second.as_ptr().cast::<u8>().wrapping_sub(8).cast();
        let kept = !field.fill(second);
        let payload =
            field.payload_len() == 16 && unsafe { field.as_ptr().cast::<u32>().read() } == 42;
        let unmapped = unsafe { libc::mprotect(new_header, 1, libc::PROT_READ) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOMEM);
        field.free();
        field.free();
        let ok = filled && kept && payload && unmapped && field.is_null();
        unsafe { libc::_exit(i32::from(!ok)) };
    }
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(child, &raw mut status, 0) }, child);
    assert!(libc::WIFEXITED(status));
    assert_eq!(libc::WEXITSTATUS(status), 0);
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no fork")]
fn a_forked_child_writes_its_own_copy_of_a_private_mapping_and_the_shared_one() {
    let mut private = Shm::calloc_private(1, 4);
    let mut shared = Shm::calloc(1, 4);
    let (mine, ours) = (
        private.as_ptr().cast::<u32>(),
        shared.as_ptr().cast::<u32>(),
    );
    let child = unsafe { libc::fork() };
    assert!(child >= 0);
    if child == 0 {
        unsafe {
            mine.write(7);
            ours.write(7);
            libc::_exit(0);
        }
    }
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(child, &raw mut status, 0) }, child);
    assert_eq!(unsafe { (mine.read(), ours.read()) }, (0, 7));
    private.free();
    shared.free();
}

#[test]
fn a_fixed_pool_hands_out_each_slice_once() {
    assert_eq!(FixedPool::new(1, 16, false).err(), Some(INVALID));
    let mut pool = FixedPool::new(4, 16, false).unwrap();
    assert_eq!((pool.total(), pool.slice_size()), (4, 16));
    let slices: Vec<_> = (0..4).map(|_| pool.alloc()).collect();
    assert!(slices.iter().all(|slice| !slice.is_null()));
    assert!(pool.alloc().is_null());
    assert_eq!(last_error(), MALLOC_FAIL);
    assert_eq!(pool.spare(), 0);
    for slice in slices {
        pool.free(slice);
    }
    assert_eq!(pool.spare(), 4);
}

#[test]
#[should_panic(expected = "a live slice of this pool")]
fn a_fixed_pool_refuses_a_pointer_inside_a_slice() {
    let mut pool = FixedPool::new(2, 64, false).unwrap();
    let slice = pool.alloc().cast::<u8>();
    // Inside the slice's data, which nothing has written: no lock byte is read there.
    pool.free(slice.wrapping_add(48).cast());
}

#[test]
fn words_stay_mapped_after_the_pool_is_deleted() {
    crate::swr_v1_core_set_pagesize(4096);
    // Twice: each kept pool stays reachable, so LeakSanitizer reports neither.
    for _ in 0..2 {
        pool::init();
        let words = pool::words::<std::sync::atomic::AtomicU32>(1).expect("a word");
        words[0].store(7, Ordering::SeqCst);
        pool::delete();
        assert_eq!(words[0].load(Ordering::SeqCst), 7, "a 'static word's page");
    }
}

#[test]
#[cfg(all(not(miri), any(target_os = "linux", target_os = "macos")))]
fn a_permanent_word_keeps_only_its_own_page() {
    use std::sync::atomic::AtomicU32;

    if crate::utilities::isolated_process::run_in_child(
        "memory::members::tests::a_permanent_word_keeps_only_its_own_page",
    ) {
        return;
    }
    assert!(crate::swr_v1_core_init());
    pool::init();
    let word = &pool::words::<AtomicU32>(1).unwrap()[0];
    let size = crate::constants::SW_GLOBAL_MEMORY_PAGESIZE - 8;
    let block = pool::alloc(size);
    assert!(!block.is_null());
    // A full-page block starts just after the mapping and bump-allocation headers.
    let base = block.wrapping_byte_sub(16);
    let protect = || unsafe { libc::mprotect(base, 1, libc::PROT_READ | libc::PROT_WRITE) };
    assert_eq!(protect(), 0);
    pool::delete();
    assert_eq!(protect(), -1);
    assert_eq!(crate::sys::errno(), libc::ENOMEM);
    word.store(19, Ordering::SeqCst);
    assert_eq!(word.load(Ordering::SeqCst), 19);
}

#[test]
#[cfg(not(miri))] // The process-wide pool needs its own test process.
fn pooled_words_balance_their_pins_across_pool_replacement() {
    use std::sync::atomic::AtomicU32;

    if crate::utilities::isolated_process::run_in_child(
        "memory::members::tests::pooled_words_balance_their_pins_across_pool_replacement",
    ) {
        return;
    }
    assert!(crate::swr_v1_core_init());
    pool::init();
    let old = pool::PooledWord::<AtomicU32>::allocate().unwrap();
    old.store(7, Ordering::SeqCst);
    let old_page = pool::page();
    assert_eq!(old_page.strong_count(), 2);
    pool::delete();
    pool::init();
    let fresh = pool::PooledWord::<AtomicU32>::allocate().unwrap();
    let fresh_page = pool::page();
    assert_eq!(old_page.strong_count(), 1);
    assert_eq!(fresh_page.strong_count(), 2);
    assert_eq!(old.load(Ordering::SeqCst), 7);
    drop(old);
    assert_eq!(old_page.strong_count(), 0);
    assert_eq!(fresh.load(Ordering::SeqCst), 0);
    drop(fresh);
    assert_eq!(fresh_page.strong_count(), 1);
    pool::delete();
    assert_eq!(fresh_page.strong_count(), 0);
}

#[test]
#[cfg(not(miri))] // The process-wide pool and pthread locks need an isolated process.
fn shared_table_keeps_its_pin_until_the_last_handle_drops() {
    use std::rc::Rc;

    if crate::utilities::isolated_process::run_in_child(
        "memory::members::tests::shared_table_keeps_its_pin_until_the_last_handle_drops",
    ) {
        return;
    }
    assert!(crate::swr_v1_core_init());
    pool::init();
    let mut table = SharedTable::make(64, 0.2, |_, _| 0).unwrap();
    assert!(table.create());
    let object = Rc::new(table);
    let reader = Rc::clone(&object);
    object.destroy();
    drop(object);
    let page = pool::page();
    assert_eq!(page.strong_count(), 2);
    pool::delete();
    assert_eq!(reader.size(), 64);
    drop(reader);
    assert_eq!(page.strong_count(), 0);
}

#[test]
#[cfg(not(miri))]
fn sleeping_coroutine_lock_releases_its_shared_word() {
    use crate::locks::coroutine::{CoroutineHost, CoroutineLock, LockWord};

    static HOST: CoroutineHost = CoroutineHost {
        current: || Some(1),
        alloc_shared: || pool::PooledWord::allocate().map(LockWord::Pooled),
        sleep: Some(|_| false),
        wait: None,
        wake: None,
        warning: || {},
    };
    if crate::utilities::isolated_process::run_in_child(
        "memory::members::tests::sleeping_coroutine_lock_releases_its_shared_word",
    ) {
        return;
    }
    assert!(crate::swr_v1_core_init());
    pool::init();
    let lock = CoroutineLock::new(true, &HOST);
    let page = pool::page();
    assert_eq!(page.strong_count(), 2);
    pool::delete();
    assert_eq!(lock.acquire(true), Ok(()));
    assert_eq!(lock.unlock(), Ok(()));
    drop(lock);
    assert_eq!(page.strong_count(), 0);
}

#[test]
fn unwinding_a_pool_read_releases_its_count() {
    crate::swr_v1_core_set_pagesize(4096);
    let pool = GlobalMemory::new(4096, false).unwrap();
    let generation = Arc::downgrade(&pool.snapshot().unwrap());
    let result = std::panic::catch_unwind(|| {
        pool.with(|_| {
            panic!("interrupt the read");
        })
    });
    assert!(result.is_err());
    assert_eq!(generation.strong_count(), 1);
    pool.delete();
    assert_eq!(generation.strong_count(), 0);
}

#[test]
fn global_memory_bump_allocates_zeroed_blocks_within_a_page() {
    crate::swr_v1_core_set_pagesize(4096);
    let pool = GlobalMemory::new(4096, false).unwrap();
    assert!(!pool.alloc(100).unwrap().is_null());
    assert_eq!(pool.capacity(), 4096 - 112);
    assert!(pool.alloc(4096).unwrap().is_null());
    for _ in 0..64 {
        assert!(!pool.alloc(1000).unwrap().is_null());
    }
    assert!(pool.memory_size() > 4096);
}

#[test]
fn deleting_a_generation_during_a_read_releases_it_after_the_callback() {
    crate::swr_v1_core_set_pagesize(4096);
    let pool = GlobalMemory::new(4096, true).unwrap();
    let capacity = pool.capacity();
    let old = Arc::downgrade(&pool.snapshot().unwrap());
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            pool.with(|generation| {
                barrier.wait();
                barrier.wait();
                assert_eq!(generation.pages().capacity(), capacity);
            })
            .unwrap();
        });
        barrier.wait();
        pool.delete();
        let detached = pool.snapshot().is_none();
        let fresh = GlobalImpl::new(4096, true, None).unwrap();
        pool.replace(Some(ptr::null()), Some(Arc::new(fresh)));
        let readers = old.strong_count();
        barrier.wait();
        reader.join().unwrap();
        assert!(detached);
        assert_eq!(readers, 1);
        assert_eq!(old.strong_count(), 0);
        assert!(pool.capacity() >= 4096);
    });
}

#[test]
#[cfg(not(miri))]
fn child_can_replace_a_shared_pool_while_a_parent_thread_holds_both_guards() {
    if crate::utilities::isolated_process::run_in_child(
        "memory::members::tests::child_can_replace_a_shared_pool_while_a_parent_thread_holds_both_guards",
    ) {
        return;
    }
    crate::swr_v1_core_set_pagesize(4096);
    let pool = GlobalMemory::new(4096, true).unwrap();
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let generation = pool.snapshot().unwrap();
            let _pages = generation.pages();
            let _publication = pool.publication();
            barrier.wait();
            barrier.wait();
        });
        barrier.wait();
        let pid = sys::fork();
        if pid == 0 {
            // Bound a regression that waits forever on either inherited guard.
            unsafe { libc::alarm(5) };
            let allocated = pool.alloc(8).is_ok_and(|block| !block.is_null());
            let generation = Arc::downgrade(&pool.snapshot().unwrap());
            pool.delete();
            let released = generation.strong_count() == 0;
            // libtest's other threads belong to the parent. fork_pool_pages exercises
            // normal child shutdown separately from this multithreaded guard probe.
            unsafe { libc::_exit(i32::from(!(allocated && released))) };
        }
        let mut status = 0;
        let waited = (pid > 0).then(|| sys::waitpid(pid, Some(&mut status), 0));
        barrier.wait();
        assert_eq!(waited.unwrap().unwrap(), Some(pid));
        assert_eq!(status, 0);
    });
    assert_eq!(pool.capacity(), 4096);
}

fn inherited_pool(shared: bool) -> GlobalMemory {
    let mut generation = GlobalImpl::new(4096, shared, None).unwrap();
    if shared {
        generation.creator = std::process::id() + 1;
    }
    let pool = GlobalMemory::empty();
    pool.replace(Some(ptr::null()), Some(Arc::new(generation)));
    pool
}

#[test]
fn a_forked_process_starts_its_own_shared_pool_and_keeps_the_inherited_one() {
    crate::swr_v1_core_set_pagesize(4096);
    for shared in [false, true] {
        let pool = inherited_pool(shared);
        let parent = Arc::downgrade(&pool.snapshot().unwrap());
        assert_eq!(pool.capacity(), 4096);
        let child = pool.pool.load(Ordering::Relaxed);
        assert_eq!(child.cast_const() == parent.as_ptr(), !shared);
        if shared {
            assert_eq!(parent.strong_count(), 1);
            assert_eq!(pool.snapshot().unwrap().creator, std::process::id());
        }
        assert!(!pool.alloc(8).unwrap().is_null());
        assert_eq!(
            pool.pool.load(Ordering::Relaxed),
            child,
            "the fresh pool is this process's"
        );
    }
}

#[test]
fn threads_racing_to_replace_a_forked_pool_all_get_the_one_published() {
    crate::swr_v1_core_set_pagesize(4096);
    for _ in 0..20 {
        let pool = inherited_pool(true);
        let parent = Arc::downgrade(&pool.snapshot().unwrap());
        let start = std::sync::Barrier::new(8);
        let seen: Vec<usize> = std::thread::scope(|scope| {
            let readers: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        start.wait();
                        pool.with(|found| std::ptr::from_ref(found) as usize)
                            .unwrap()
                    })
                })
                .collect();
            readers
                .into_iter()
                .map(|reader| reader.join().unwrap())
                .collect()
        });
        let published = pool.pool.load(Ordering::Relaxed);
        assert!(seen.iter().all(|&found| found == published as usize));
        assert_eq!(parent.strong_count(), 1);
        pool.delete();
        assert_eq!(parent.strong_count(), 0);
    }
}

// Upstream's core-tests/src/memory/{fixed_pool,global_memory}.cpp, one test per
// TEST, with its inputs and expected values.

/// `TEST(fixed_pool, alloc)`: 1200 random allocs (3 in 4) and frees (front or back
/// of the live list) over a 1024 x 256-byte private pool.
#[test]
#[cfg_attr(miri, ignore = "Miri has no rand")]
fn fixed_pool_alloc() {
    let mut pool = FixedPool::new(1024, 256, false).unwrap();
    let mut alloc_list = std::collections::VecDeque::new();
    assert_eq!(pool.slice_size(), 256);
    for _ in 0..1200 {
        let j = unsafe { libc::rand() };
        if j % 4 < 3 {
            let mem = pool.alloc();
            assert!(!mem.is_null());
            alloc_list.push_back(mem);
        } else if !alloc_list.is_empty() {
            let mem = if j % 2 == 1 {
                alloc_list.pop_front()
            } else {
                alloc_list.pop_back()
            };
            pool.free(mem.unwrap());
        }
    }
    pool.debug(1);
}

/// `TEST(global_memory, alloc)`: `free` is a no-op, so a freed block keeps what is
/// written to it.
#[test]
fn global_memory_alloc() {
    crate::swr_v1_core_set_pagesize(4096);
    let pool = GlobalMemory::new(2 * 1024 * 1024, false).unwrap();
    let alloc = |pool: &GlobalMemory, size| pool.alloc(size).unwrap().cast::<u8>();
    let strcpy = |block: *mut u8, text: &std::ffi::CStr| unsafe {
        ptr::copy_nonoverlapping(text.as_ptr().cast(), block, text.count_bytes() + 1);
    };
    let ptr1 = alloc(&pool, 199);
    strcpy(ptr1, c"hello, world, #1");
    let ptr2 = alloc(&pool, 17);
    strcpy(ptr2, c"hello, world, #2");
    let ptr3 = alloc(&pool, 113);
    strcpy(ptr3, c"hello, world, #3");
    assert!(!ptr1.is_null());
    assert!(!ptr2.is_null());
    assert!(!ptr3.is_null());
    assert!(pool.capacity() > 2 * 1024 * 1024 - 512);
    assert!(pool.memory_size() > 0);
    for (block, text) in [
        (ptr1, c"hello, world, #1"),
        (ptr2, c"hello, world, #2"),
        (ptr3, c"hello, world, #3"),
    ] {
        assert_eq!(unsafe { std::ffi::CStr::from_ptr(block.cast()) }, text);
    }
}

#[test]
#[cfg(not(miri))] // Isolate the process-wide pool and native pthread primitives.
fn pooled_lock_releases_its_pin_after_using_the_shared_storage() {
    use crate::locks::{Kind, LOCK_EX, LOCK_NB};

    if crate::utilities::isolated_process::run_in_child(
        "memory::members::tests::pooled_lock_releases_its_pin_after_using_the_shared_storage",
    ) {
        return;
    }
    assert!(crate::swr_v1_core_init());
    for &kind in Kind::ALL {
        pool::init();
        let lock = pool::PooledLock::new(kind).unwrap();
        let page = pool::page();
        assert_eq!(page.strong_count(), 2);
        pool::delete();
        lock.lock(LOCK_EX | LOCK_NB, 0, 0).unwrap();
        lock.unlock().unwrap();
        drop(lock);
        assert_eq!(page.strong_count(), 0);
    }
}
