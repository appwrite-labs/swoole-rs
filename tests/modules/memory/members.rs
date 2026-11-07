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
    let mut pool = ptr::null_mut();
    assert_eq!(
        unsafe { swr_v1_memory_fixed_pool_new(1, 16, false, &raw mut pool) },
        INVALID
    );
    assert_eq!(
        unsafe { swr_v1_memory_fixed_pool_new(4, 16, false, &raw mut pool) },
        0
    );
    // A `swoole::FixedPool`: its vtable pointer, then `impl`.
    let object = [ptr::null_mut(), pool.cast::<c_void>()];
    let this = object.as_ptr().cast();
    assert_eq!(
        unsafe { (swr_fixed_pool_total(this), swr_fixed_pool_slice_size(this)) },
        (4, 16)
    );
    let slices: Vec<_> = (0..4)
        .map(|_| unsafe { swr_v1_memory_fixed_pool_alloc(pool) })
        .collect();
    assert!(slices.iter().all(|slice| !slice.is_null()));
    assert!(unsafe { swr_v1_memory_fixed_pool_alloc(pool) }.is_null());
    assert_eq!(last_error(), MALLOC_FAIL);
    assert_eq!(unsafe { swr_fixed_pool_spare(this) }, 0);
    for slice in slices {
        unsafe { swr_v1_memory_fixed_pool_free(pool, slice) };
    }
    assert_eq!(unsafe { swr_fixed_pool_spare(this) }, 4);
    unsafe { swr_v1_memory_fixed_pool_delete(pool) };
}

#[test]
fn a_ring_buffer_reuses_freed_records() {
    let mut ring = ptr::null_mut();
    assert_eq!(
        unsafe { swr_v1_memory_ring_buffer_new(8, false, &raw mut ring) },
        INVALID
    );
    assert_eq!(
        unsafe { swr_v1_memory_ring_buffer_new(256, true, &raw mut ring) },
        0
    );
    for _ in 0..64 {
        let record = unsafe { swr_v1_memory_ring_buffer_alloc(ring, 40) };
        assert!(!record.is_null());
        unsafe { swr_v1_memory_ring_buffer_free(ring, record) };
    }
    unsafe { swr_v1_memory_ring_buffer_delete(ring) };
}

#[test]
fn global_memory_bump_allocates_zeroed_blocks_within_a_page() {
    swoole_rs_corebase::swr_v1_core_set_pagesize(4096);
    let mut pool = ptr::null_mut();
    assert_eq!(
        unsafe { swr_v1_memory_global_memory_new(4096, false, &raw mut pool) },
        0
    );
    let object = [ptr::null_mut(), pool.cast::<c_void>()];
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
