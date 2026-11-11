use super::*;
use swoole_rs_corebase::log::last_error;

#[test]
#[cfg_attr(miri, ignore = "Miri has no shared mappings")]
fn shared_mappings_are_zeroed_protected_and_freed() {
    let memory = sw_shm_calloc(4, 25).cast::<u8>();
    assert!(!memory.is_null());
    unsafe {
        assert!((0..100).all(|i| memory.add(i).read() == 0));
        assert_eq!(sw_shm_protect(memory.cast(), libc::PROT_READ), 0);
        assert_eq!(
            sw_shm_protect(memory.cast(), libc::PROT_READ | libc::PROT_WRITE),
            0
        );
        memory.write_bytes(7, 100);
        let total = memory
            .cast::<c_void>()
            .wrapping_byte_sub(8)
            .cast::<usize>()
            .read();
        assert_eq!(total, 112);
    }
    sw_shm_free(memory.cast());
    assert!(sw_shm_malloc(usize::MAX).is_null());
    assert!(sw_shm_calloc(usize::MAX, 2).is_null());
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no shared mappings")]
fn a_freed_shm_field_is_null_and_freeing_it_again_unmaps_nothing() {
    let mut field = Shm::calloc(2, 8);
    assert!(!field.is_null());
    field.free();
    assert!(field.is_null());
    // A second free, like a shutdown after a failed startup freed the field,
    // reads no header from the unmapped payload.
    field.free();
    assert!(field.is_null());
    assert!(Shm::NULL.is_null());
    assert!(Shm::calloc(usize::MAX, 2).is_null());
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
#[cfg_attr(miri, ignore = "Miri has no shared mappings")]
fn words_stay_mapped_after_the_pool_is_deleted() {
    swoole_rs_corebase::swr_v1_core_set_pagesize(4096);
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
fn global_memory_bump_allocates_zeroed_blocks_within_a_page() {
    swoole_rs_corebase::swr_v1_core_set_pagesize(4096);
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
#[cfg_attr(miri, ignore = "Miri has no shared mappings")]
fn a_forked_process_starts_its_own_shared_pool_and_keeps_the_inherited_one() {
    swoole_rs_corebase::swr_v1_core_set_pagesize(4096);
    for shared in [false, true] {
        let pool = GlobalMemory::new(4096, shared).unwrap();
        let parent = pool.pool.load(Ordering::Relaxed);
        // As if another process made it.
        unsafe { (*parent).creator = if shared { std::process::id() + 1 } else { 0 } };
        assert_eq!(pool.capacity(), 4096);
        let child = pool.pool.load(Ordering::Relaxed);
        assert_eq!(child == parent, !shared);
        if shared {
            assert_eq!(unsafe { (*child).inherited }, parent);
            assert_eq!(unsafe { (*child).creator }, std::process::id());
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
#[cfg_attr(miri, ignore = "Miri has no shared mappings")]
fn threads_racing_to_replace_a_forked_pool_all_get_the_one_published() {
    swoole_rs_corebase::swr_v1_core_set_pagesize(4096);
    for _ in 0..20 {
        let pool = GlobalMemory::new(4096, true).unwrap();
        let parent = pool.pool.load(Ordering::Relaxed);
        // As if another process made it.
        unsafe { (*parent).creator = std::process::id() + 1 };
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
        assert_eq!(unsafe { (*published).inherited }, parent);
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
    swoole_rs_corebase::swr_v1_core_set_pagesize(4096);
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
