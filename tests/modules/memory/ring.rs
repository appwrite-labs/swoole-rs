use super::*;
#[test]
fn ring_layout_headers_wrap_and_multi_releaser_publication() {
    unsafe {
        assert_eq!(layout(1025), Some(1032));
        assert_eq!(layout(40), None);
        assert_eq!(layout(u32::MAX), None);
        let mut memory = [0u64; 128];
        let count = AtomicU32::new(0);
        let mut ring = Ring {
            memory: memory.as_mut_ptr().cast(),
            size: 1024,
            alloc_offset: 0,
            collect_offset: 0,
            alloc_count: u32::MAX,
            status: 0,
            free_count: count.as_ptr(),
        };
        let first = allocate(&mut ring, 7);
        assert_eq!(first.cast::<u8>(), ring.memory.add(8));
        assert_eq!(ring.memory.add(2).cast::<u16>().read(), u16::MAX);
        assert_eq!(ring.memory.add(4).cast::<u32>().read(), 8);
        assert_eq!(ring.alloc_count, 0);
        assert!(free(ring.memory.cast(), ring.size, ring.free_count, first));
        assert!(!free(ring.memory.cast(), ring.size, ring.free_count, first));
        for _ in 0..10000 {
            let item = allocate(&mut ring, 113);
            if !item.is_null() {
                ptr::write_bytes(item, 42, 113);
                assert!(free(ring.memory.cast(), ring.size, ring.free_count, item));
            }
        }
        assert!(ring.alloc_count > 1000);
        assert!(count.load(Ordering::SeqCst) <= 2);
    }
}

#[test]
fn release_range_guards_keep_header_underflow_alignment_and_extreme_rejections() {
    for memory in [0usize, 1, 8, 64, usize::MAX - 7, usize::MAX] {
        for count in [0usize, 1, 4, usize::MAX - 3] {
            for size in [0u32, 7, 8, 16, u32::MAX] {
                for distance in [0usize, 1, 7, 8, 9, 16, 24, u32::MAX as usize, usize::MAX] {
                    let data = memory.wrapping_add(distance);
                    // Independent checked-range model, including addresses near
                    // the machine limit; never dereference these scalar witnesses.
                    let expected = data
                        .checked_sub(memory)
                        .and_then(|n| n.checked_sub(8))
                        .filter(|&offset| {
                            memory != 0
                                && count != 0
                                && data != 0
                                && memory.is_multiple_of(8)
                                && count.is_multiple_of(4)
                                && size >= 8
                                && offset.is_multiple_of(8)
                                && offset <= (size as usize).saturating_sub(8)
                        });
                    assert_eq!(release_offset(memory, size, count, data), expected);
                }
            }
        }
    }
}

#[test]
fn actual_shared_storage_matches_frozen_snapshot_abi_without_whole_header_borrows() {
    unsafe {
        assert_eq!(std::mem::size_of::<Storage>(), 32);
        assert_eq!(std::mem::offset_of!(Storage, free_count), 28);
        let (mut left, mut right) = ([0u64; 128], [0u64; 128]);
        let counter = AtomicU32::new(0);
        let mut state = Ring {
            memory: left.as_mut_ptr().cast(),
            size: 1024,
            alloc_offset: 0,
            collect_offset: 0,
            alloc_count: 0,
            status: 0,
            free_count: counter.as_ptr(),
        };
        let mut storage = Storage {
            memory: right.as_mut_ptr().cast(),
            shared: 1,
            status: 0,
            size: 1024,
            alloc_offset: 0,
            collect_offset: 0,
            alloc_count: 0,
            free_count: 0,
        };
        let pointer = ptr::addr_of_mut!(storage);
        let released = ptr::addr_of_mut!((*pointer).free_count);
        for step in 0..5000u32 {
            let requested = match step % 271 {
                0 => 0,
                1 => u32::MAX,
                _ => step % 129 + 1,
            };
            let before = allocate(&mut state, requested);
            let after = storage_alloc(pointer, requested);
            assert_eq!(before.is_null(), after.is_null());
            assert_eq!(
                (
                    state.alloc_offset,
                    state.collect_offset,
                    state.alloc_count,
                    state.status
                ),
                (
                    storage.alloc_offset,
                    storage.collect_offset,
                    storage.alloc_count,
                    storage.status
                )
            );
            if !before.is_null() {
                assert_eq!(
                    before.addr() - state.memory.addr(),
                    after.addr() - storage.memory.addr()
                );
                assert!(free(
                    state.memory.cast(),
                    state.size,
                    state.free_count,
                    before
                ));
                assert!(free(storage.memory.cast(), storage.size, released, after));
            }
            assert_eq!(
                counter.load(Ordering::SeqCst),
                AtomicU32::from_ptr(released).load(Ordering::SeqCst)
            );
        }
    }
}

#[test]
fn collection_bound_can_come_from_a_later_record_with_an_earlier_release_in_flight() {
    unsafe {
        let mut memory = [0u64; 128];
        let count = AtomicU32::new(0);
        let mut ring = Ring {
            memory: memory.as_mut_ptr().cast(),
            size: 1024,
            alloc_offset: 0,
            collect_offset: 0,
            alloc_count: 0,
            status: 0,
            free_count: count.as_ptr(),
        };
        let first = allocate(&mut ring, 8);
        let second = allocate(&mut ring, 8);
        assert!(!first.is_null() && !second.is_null());
        assert!(free(ring.memory.cast(), ring.size, ring.free_count, second));
        // A later record's completed release cannot reclaim the still-live head.
        assert!(ring.collect());
        assert_eq!((ring.collect_offset, count.load(Ordering::SeqCst)), (0, 1));
        // Pause the earlier consumer between its release-store and counter RMW.
        // The old count belongs to `second`, so only the head's own acquire load
        // can publish this earlier consumer's payload access before reclamation.
        AtomicU16::from_ptr(first.cast::<u8>().sub(8).cast()).store(0, Ordering::Release);
        assert!(ring.collect());
        assert_eq!((ring.collect_offset, count.load(Ordering::SeqCst)), (16, 0));
        count.fetch_add(1, Ordering::SeqCst);
        assert!(ring.collect());
        assert_eq!((ring.collect_offset, count.load(Ordering::SeqCst)), (32, 0));
    }
}

#[test]
fn constructor_initializes_only_the_real_header_before_shared_publication() {
    assert_eq!(std::mem::size_of::<Storage>(), 32);
    for total in [48u32, 64, 1024, 1032] {
        for shared in [false, true] {
            let mut arena = vec![0xabab_abab_abab_ababu64; total as usize / 8];
            let storage = arena.as_mut_ptr().cast::<Storage>();
            unsafe {
                construct(storage, total, shared);
                assert_eq!(
                    ptr::addr_of!((*storage).memory).read(),
                    storage.cast::<u8>().add(32)
                );
                assert_eq!(ptr::addr_of!((*storage).size).read(), total - 32);
                assert_eq!(ptr::addr_of!((*storage).shared).read(), u8::from(shared));
                assert_eq!(
                    (
                        ptr::addr_of!((*storage).status).read(),
                        ptr::addr_of!((*storage).alloc_offset).read(),
                        ptr::addr_of!((*storage).collect_offset).read(),
                        ptr::addr_of!((*storage).alloc_count).read(),
                        ptr::addr_of!((*storage).free_count).read()
                    ),
                    (0, 0, 0, 0, 0)
                );
                assert_eq!(
                    std::slice::from_raw_parts(storage.cast::<u8>().add(32), total as usize - 32),
                    vec![0xab; total as usize - 32]
                );
                let record = storage_alloc(storage, 1);
                if total >= 64 {
                    assert!(!record.is_null());
                }
            }
        }
    }
}
