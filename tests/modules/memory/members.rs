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

// Upstream's core-tests/src/memory/{fixed_pool,global_memory,ringbuffer}.cpp, one test
// per TEST, with its inputs and expected values. `this` is a stand-in C++ object: a
// vtable pointer, then `impl`.

/// `TEST(fixed_pool, alloc)`: 1200 random allocs (3 in 4) and frees (front or back
/// of the live list) over a 1024 x 256-byte private pool.
#[test]
fn fixed_pool_alloc() {
    let mut pool = ptr::null_mut();
    assert_eq!(
        unsafe { swr_v1_memory_fixed_pool_new(1024, 256, false, &raw mut pool) },
        0
    );
    let object = [ptr::null_mut(), pool.cast::<c_void>()];
    let this = object.as_ptr().cast();
    let mut alloc_list = std::collections::VecDeque::new();
    assert_eq!(unsafe { swr_fixed_pool_slice_size(this) }, 256);
    for _ in 0..1200 {
        let j = unsafe { libc::rand() };
        if j % 4 < 3 {
            let mem = unsafe { swr_v1_memory_fixed_pool_alloc(pool) };
            assert!(!mem.is_null());
            alloc_list.push_back(mem);
        } else if !alloc_list.is_empty() {
            let mem = if j % 2 == 1 {
                alloc_list.pop_front()
            } else {
                alloc_list.pop_back()
            };
            unsafe { swr_v1_memory_fixed_pool_free(pool, mem.unwrap()) };
        }
    }
    unsafe { swr_fixed_pool_debug(this, 1) };
    unsafe { swr_v1_memory_fixed_pool_delete(pool) };
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
    let object = [ptr::null_mut(), pool.cast::<c_void>()];
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

/// `TEST(ringbuffer, thread)`: one writer allocates 10000 records of 10000..99999
/// bytes from a 4 MiB ring (shared, then private), stamps each with its size and a
/// serial number, and sends it over one of 4 datagram socket pairs; each reader checks
/// both stamps and frees the record.
#[test]
fn ringbuffer_thread() {
    use std::os::unix::net::UnixDatagram;
    const READ_THREAD_N: usize = 4;
    const WRITE_N: usize = 10000;
    const PACKET_LEN: u32 = 90000;
    /// `pkg`: `id`, `size`, `serial_num`, `ptr`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Pkg {
        id: u32,
        size: u32,
        serial_num: u32,
        ptr: usize,
    }
    const PKG: usize = mem::size_of::<Pkg>();
    for shared in [true, false] {
        let mut ring = ptr::null_mut();
        assert_eq!(
            unsafe { swr_v1_memory_ring_buffer_new(1024 * 1024 * 4, shared, &raw mut ring) },
            0
        );
        let address = ring.addr();
        let mut pipes = Vec::new();
        let mut readers = Vec::new();
        for _ in 0..READ_THREAD_N {
            let (write, read) = UnixDatagram::pair().unwrap();
            pipes.push(write);
            readers.push(std::thread::spawn(move || {
                let ring = ptr::with_exposed_provenance_mut::<Storage>(address);
                for _ in 0..WRITE_N / READ_THREAD_N {
                    let mut bytes = [0u8; PKG];
                    assert_eq!(read.recv(&mut bytes).unwrap(), PKG);
                    let pkg = unsafe { bytes.as_ptr().cast::<Pkg>().read_unaligned() };
                    let data = ptr::with_exposed_provenance_mut::<u8>(pkg.ptr);
                    assert_eq!(unsafe { data.cast::<u32>().read_unaligned() }, pkg.size);
                    let tail = unsafe { data.add(pkg.size as usize - 4) };
                    assert_eq!(
                        unsafe { tail.cast::<u32>().read_unaligned() },
                        pkg.serial_num
                    );
                    unsafe { swr_v1_memory_ring_buffer_free(ring, data.cast()) };
                }
            }));
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
        for i in 0..WRITE_N {
            let size = 10000 + unsafe { libc::rand() }.unsigned_abs() % PACKET_LEN;
            let mut data = ptr::null_mut();
            for _ in 0..100 {
                data = unsafe { swr_v1_memory_ring_buffer_alloc(ring, size) };
                if !data.is_null() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_micros(10));
            }
            assert!(!data.is_null(), "alloc failed. index={i}, break");
            let pkg = Pkg {
                id: u32::try_from(i).unwrap(),
                size,
                serial_num: unsafe { libc::rand() }.unsigned_abs(),
                ptr: data.expose_provenance(),
            };
            let data = data.cast::<u8>();
            unsafe { data.cast::<u32>().write_unaligned(size) };
            unsafe {
                data.add(size as usize - 4)
                    .cast::<u32>()
                    .write_unaligned(pkg.serial_num);
            }
            let bytes =
                unsafe { std::slice::from_raw_parts(ptr::from_ref(&pkg).cast::<u8>(), PKG) };
            assert!(pipes[i % READ_THREAD_N].send(bytes).is_ok());
        }
        for reader in readers {
            reader.join().unwrap();
        }
        unsafe { swr_v1_memory_ring_buffer_delete(ring) };
    }
}
