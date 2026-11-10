use super::*;
use swoole_rs_corebase::log::last_error;

#[test]
fn shared_mappings_are_zeroed_resized_protected_and_freed() {
    let memory = sw_shm_calloc(4, 25).cast::<u8>();
    assert!(!memory.is_null());
    unsafe {
        assert!((0..100).all(|i| memory.add(i).read() == 0));
        memory.write_bytes(7, 100);
    }
    let grown = sw_shm_realloc(memory.cast(), 200).cast::<u8>();
    assert!(!grown.is_null());
    unsafe { assert!((0..100).all(|i| grown.add(i).read() == 7)) };
    assert_eq!(sw_shm_protect(grown.cast(), libc::PROT_READ), 0);
    assert_eq!(
        sw_shm_protect(grown.cast(), libc::PROT_READ | libc::PROT_WRITE),
        0
    );
    // Shrinking copies only what fits.
    let shrunk = sw_shm_realloc(grown.cast(), 9).cast::<u8>();
    assert!(!shrunk.is_null());
    unsafe { assert!((0..16).all(|i| shrunk.add(i).read() == 7)) };
    assert_eq!(
        unsafe {
            shrunk
                .cast::<c_void>()
                .wrapping_byte_sub(8)
                .cast::<usize>()
                .read()
        },
        24
    );
    sw_shm_free(shrunk.cast());
    assert!(sw_shm_malloc(usize::MAX).is_null());
    assert!(sw_shm_calloc(usize::MAX, 2).is_null());
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
fn global_memory_bump_allocates_zeroed_blocks_within_a_page() {
    swoole_rs_corebase::swr_v1_core_set_pagesize(4096);
    let mut pool = ptr::null_mut();
    assert_eq!(
        unsafe { swr_v1_memory_global_memory_new(4096, false, &raw mut pool) },
        0
    );
    let object = [pool.cast::<c_void>()];
    let this = object.as_ptr().cast();
    let mut block = ptr::null_mut();
    assert_eq!(
        unsafe { swr_v1_memory_global_memory_alloc(&raw mut pool, 100, &raw mut block) },
        0
    );
    assert!(!block.is_null());
    assert_eq!(unsafe { swr_global_memory_capacity(this) }, 4096 - 112);
    assert_eq!(
        unsafe { swr_v1_memory_global_memory_alloc(&raw mut pool, 4096, &raw mut block) },
        0
    );
    assert!(block.is_null());
    for _ in 0..64 {
        assert_eq!(
            unsafe { swr_v1_memory_global_memory_alloc(&raw mut pool, 1000, &raw mut block) },
            0
        );
        assert!(!block.is_null());
    }
    assert!(unsafe { swr_global_memory_size(this) } > 4096);
    unsafe { swr_v1_memory_global_memory_delete(pool) };
}

#[test]
fn a_forked_process_starts_its_own_shared_pool_and_keeps_the_inherited_one() {
    swoole_rs_corebase::swr_v1_core_set_pagesize(4096);
    for shared in [false, true] {
        let mut pool = ptr::null_mut();
        assert_eq!(
            unsafe { swr_v1_memory_global_memory_new(4096, shared, &raw mut pool) },
            0
        );
        let parent = pool;
        // As if another process made it.
        unsafe { (*parent).creator = if shared { std::process::id() + 1 } else { 0 } };
        let mut object = [pool.cast::<c_void>()];
        let this = object.as_mut_ptr().cast_const().cast();
        assert_eq!(unsafe { swr_global_memory_capacity(this) }, 4096);
        let child = object[0].cast::<GlobalImpl>();
        assert_eq!(child == parent, !shared);
        if shared {
            assert_eq!(unsafe { (*child).inherited }, parent);
            assert_eq!(unsafe { (*child).creator }, std::process::id());
        }
        pool = child;
        let mut block = ptr::null_mut();
        assert_eq!(
            unsafe { swr_v1_memory_global_memory_alloc(&raw mut pool, 8, &raw mut block) },
            0
        );
        assert!(!block.is_null());
        assert_eq!(pool, child, "the fresh pool is this process's");
        unsafe { swr_v1_memory_global_memory_delete(pool) };
    }
}

// Upstream's core-tests/src/memory/{fixed_pool,global_memory}.cpp, one test per
// TEST, with its inputs and expected values.
// `this` is a stand-in C++ object: a GlobalMemory's `impl`.

/// `TEST(fixed_pool, alloc)`: 1200 random allocs (3 in 4) and frees (front or back
/// of the live list) over a 1024 x 256-byte private pool.
#[test]
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

/// `TEST(fixed_pool, realloc)`.
#[test]
fn fixed_pool_realloc() {
    let memory = sw_shm_malloc(1024);
    let new_memory = sw_shm_realloc(memory, 2048);
    assert!(!new_memory.is_null());
    sw_shm_free(new_memory);
}

/// `TEST(global_memory, alloc)`: `free` is a no-op, so a freed block keeps what is
/// written to it.
#[test]
fn global_memory_alloc() {
    swoole_rs_corebase::swr_v1_core_set_pagesize(4096);
    let mut pool = ptr::null_mut();
    assert_eq!(
        unsafe { swr_v1_memory_global_memory_new(2 * 1024 * 1024, false, &raw mut pool) },
        0
    );
    let object = [pool.cast::<c_void>()];
    let this = object.as_ptr().cast();
    let alloc = |pool: &mut *mut GlobalImpl, size| {
        let mut block = ptr::null_mut();
        assert_eq!(
            unsafe { swr_v1_memory_global_memory_alloc(pool, size, &raw mut block) },
            0
        );
        block.cast::<u8>()
    };
    let strcpy = |block: *mut u8, text: &std::ffi::CStr| unsafe {
        ptr::copy_nonoverlapping(text.as_ptr().cast(), block, text.count_bytes() + 1);
    };
    let ptr1 = alloc(&mut pool, 199);
    strcpy(ptr1, c"hello, world, #1");
    let ptr2 = alloc(&mut pool, 17);
    strcpy(ptr2, c"hello, world, #2");
    let ptr3 = alloc(&mut pool, 113);
    strcpy(ptr3, c"hello, world, #3");
    assert!(!ptr1.is_null());
    assert!(!ptr2.is_null());
    assert!(!ptr3.is_null());
    assert!(unsafe { swr_global_memory_capacity(this) } > 2 * 1024 * 1024 - 512);
    assert!(unsafe { swr_global_memory_size(this) } > 0);
    for (block, text) in [
        (ptr1, c"hello, world, #1"),
        (ptr2, c"hello, world, #2"),
        (ptr3, c"hello, world, #3"),
    ] {
        assert_eq!(unsafe { std::ffi::CStr::from_ptr(block.cast()) }, text);
    }
    unsafe { swr_v1_memory_global_memory_delete(pool) };
}
