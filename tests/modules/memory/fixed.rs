use super::*;
use swoole_rs_corebase::CCast;

// `FixedPool::debug`'s walk, which only the ported `FixedPool` (members.rs) prints.

#[derive(Default)]
pub(crate) struct DebugCursor {
    next: *mut Slice,
    line: u64,
}
pub(crate) struct DebugRecord {
    pub(crate) slice: *mut Slice,
    pub(crate) prev: *mut Slice,
    pub(crate) next: *mut Slice,
    pub(crate) data: *mut u8,
    pub(crate) line: u64,
    pub(crate) lock: u8,
    pub(crate) self_link: bool,
}

/// The next slice `FixedPool::debug` prints, walking the live links from the head.
pub(crate) fn debug_step(
    pool: &Fixed,
    cursor: &mut DebugCursor,
    max_lines: i32,
) -> Option<DebugRecord> {
    let slice = if cursor.line == 0 {
        pool.head.get()
    } else {
        cursor.next
    };
    let node = pool.nodes().get(slice)?;
    let record = DebugRecord {
        slice,
        prev: node.prev,
        next: node.next,
        data: slice.cast::<u8>().wrapping_add(mem::size_of::<Slice>()),
        line: cursor.line,
        lock: node.lock,
        self_link: node.next == slice,
    };
    // Preserve the original post-print `line++ > max_lines` bound: max0 emits2.
    cursor.next = if max_lines < 0 || u64::try_from(max_lines).is_ok_and(|max| cursor.line > max) {
        ptr::null_mut()
    } else {
        node.next
    };
    cursor.line += 1;
    Some(record)
}

/// A pool over a fresh `0xa5`-filled buffer of `plan.total` bytes plus one word.
fn pool_in(plan: Layout, shared: bool, owned: bool) -> (Vec<u64>, *mut Fixed) {
    let mut memory = vec![0xa5a5_a5a5_a5a5_a5a5u64; plan.total.div_ceil(8) + 1];
    let arena = unsafe {
        std::slice::from_raw_parts_mut(
            memory.as_mut_ptr().cast::<MaybeUninit<u8>>(),
            memory.len() * 8,
        )
    };
    let pool = construct(arena, plan, shared, owned);
    assert!(pool.is_some());
    (memory, pool.unwrap().as_ptr())
}

fn borrowed(width: u32, slices: usize) -> Layout {
    let available = mem::size_of::<Fixed>() + slices * (mem::size_of::<Slice>() + width as usize);
    Layout::new(0, width, available, false).unwrap()
}

#[test]
fn fixed_pool_preserves_lifo_reuse_at_aligned_and_odd_table_widths() {
    for width in [1u32, 7, 8, 31, 256] {
        let stride = mem::size_of::<Slice>() + width as usize;
        let (_memory, pool) = pool_in(borrowed(width, 17), true, false);
        unsafe {
            let base = (*pool).memory;
            let mut entries = Vec::new();
            for index in (0..17).rev() {
                let data = alloc(&*pool);
                assert_eq!(
                    data.cast::<u8>(),
                    base.add(index * stride + mem::size_of::<Slice>())
                );
                ptr::write_bytes(data, index.c_cast::<u8>(), width as usize);
                entries.push(data);
            }
            assert!(alloc(&*pool).is_null());
            assert_eq!((*pool).slice_use.get(), 17);
            for &i in &[3, 10, 0] {
                assert!(free(&*pool, entries[i]));
            }
            assert!(!free(&*pool, entries[0]));
            assert!(!free(&*pool, entries[0].cast::<u8>().add(1).cast()));
            for &i in &[0, 10, 3] {
                assert_eq!(alloc(&*pool), entries[i]);
            }
            assert_eq!((*pool).slice_use.get(), 17);
        }
    }
}

#[test]
fn fixed_construction_plans_own_alignment_and_borrowed_floor_counts() {
    for width in [0u32, 1, 7, 8, 31, 256] {
        let plan = Layout::new(17, width, 0, true).unwrap();
        let aligned = (width + 7) & !7;
        assert_eq!(plan.width, aligned);
        assert_eq!(plan.slices, 17);
        assert_eq!(plan.usable, 17 * (24 + aligned as usize));
        assert_eq!(plan.total, 48 + plan.usable);
        let available = 48 + 17 * (24 + width as usize) + 3;
        let plan = Layout::new(0, width, available, false).unwrap();
        assert_eq!(
            (plan.total, plan.usable, plan.slices, plan.width),
            (available, available - 48, 17, width)
        );
    }
    // Invalid and overflowing plans are rejected without requesting an arena.
    for (count, width, available, owned) in [
        (0, 8, 0, true),
        (1, 8, 0, true),
        (2, u32::MAX, 0, true),
        (2, 8, 1, true),
        (0, 8, 0, false),
        (0, 8, usize::MAX, false),
        (1, 8, 128, false),
    ] {
        assert_eq!(Layout::new(count, width, available, owned), None);
    }
}

#[test]
fn fixed_constructs_complete_real_header_and_links_without_touching_payload_or_tail() {
    for owned in [false, true] {
        for shared in [false, true] {
            for width in [0u32, 1, 7, 8, 31, 256] {
                let plan = if owned {
                    Layout::new(17, width, 0, true).unwrap()
                } else {
                    Layout::new(0, width, 48 + 17 * (24 + width as usize) + 3, false).unwrap()
                };
                let (memory, arena) = pool_in(plan, shared, owned);
                unsafe {
                    let pool = &*arena;
                    assert_eq!(pool.memory, arena.cast::<u8>().add(48));
                    assert_eq!(
                        (
                            pool.size,
                            pool.slice_num,
                            pool.slice_use.get(),
                            pool.slice_size,
                            pool.shared,
                            pool.allocated
                        ),
                        (plan.usable, 17, 0, plan.width, shared, owned)
                    );
                    let stride = 24 + plan.width as usize;
                    for index in (0..17).rev() {
                        let data = alloc(&*arena).cast::<u8>();
                        assert_eq!(data, arena.cast::<u8>().add(48 + index * stride + 24));
                        assert!(
                            std::slice::from_raw_parts(data, plan.width as usize)
                                .iter()
                                .all(|&b| b == 0xa5)
                        );
                        assert!(free(&*arena, data.cast()));
                        // Reuse consumes the same node; leave it live before the next allocation.
                        assert_eq!(alloc(&*arena), data.cast());
                    }
                    assert!(alloc(&*arena).is_null());
                    let end = 48 + 17 * stride;
                    assert!(
                        std::slice::from_raw_parts(
                            arena.cast::<u8>().add(end),
                            memory.len() * 8 - end
                        )
                        .iter()
                        .all(|&b| b == 0xa5)
                    );
                }
            }
        }
    }
}

#[test]
fn fixed_construction_rejects_a_misaligned_arena_before_writing() {
    let plan = Layout::new(2, 8, 0, true).unwrap();
    let mut memory = [0xa5a5_a5a5_a5a5_a5a5u64; 32];
    let before = memory;
    let arena = memory
        .as_mut_ptr()
        .cast::<u8>()
        .wrapping_add(1)
        .cast::<MaybeUninit<u8>>();
    let arena = unsafe { std::slice::from_raw_parts_mut(arena, plan.total) };
    assert!(construct(arena, plan, true, true).is_none());
    assert_eq!(memory, before);
}

#[test]
fn diagnostic_cursor_matches_live_odd_links_and_original_postprint_limit() {
    unsafe {
        for width in [1u32, 8, 31] {
            let (_arena, pool) = pool_in(borrowed(width, 5), false, false);
            assert!(!alloc(&*pool).is_null());
            for (max, expected) in [(-2, 1), (-1, 1), (0, 2), (1, 3), (3, 5), (99, 5)] {
                let mut cursor = DebugCursor::default();
                let mut next = (*pool).head.get();
                for line in 0..expected {
                    let actual = next.read_unaligned();
                    let record = debug_step(&*pool, &mut cursor, max).unwrap();
                    assert_eq!(
                        (
                            record.slice,
                            record.prev,
                            record.next,
                            record.line,
                            record.lock
                        ),
                        (next, actual.prev, actual.next, line, actual.lock)
                    );
                    assert_eq!(record.data, next.cast::<u8>().add(mem::size_of::<Slice>()));
                    assert!(!record.self_link);
                    next = actual.next;
                }
                assert!(debug_step(&*pool, &mut cursor, max).is_none());
            }
            // The original diagnostic explicitly marks self-links and bounds them.
            let head = (*pool).head.get();
            let original = head.read_unaligned();
            head.write_unaligned(Slice {
                next: head,
                ..original
            });
            let mut cursor = DebugCursor::default();
            for line in 0..2 {
                let record = debug_step(&*pool, &mut cursor, 0).unwrap();
                assert_eq!(
                    (record.line, record.self_link, record.slice),
                    (line, true, head)
                );
            }
            assert!(debug_step(&*pool, &mut cursor, 0).is_none());
            head.write_unaligned(original);
        }
    }
}

#[test]
fn normal_queue_changes_publish_only_mutable_header_fields() {
    unsafe {
        for count in [2, 3, 8, 17] {
            let (_arena, pool) = pool_in(Layout::new(count, 16, 0, true).unwrap(), false, true);
            let original = pool.read();
            let fixed = |current: Fixed| {
                (
                    current.memory,
                    current.size,
                    current.slice_num,
                    current.slice_size,
                    current.shared,
                    current.allocated,
                )
            };
            let expected = (original.memory, original.size, count, 16, false, true);
            let mut live = Vec::new();
            for _ in 0..count {
                let value = alloc(&*pool);
                assert!(!value.is_null());
                live.push(value);
                assert_eq!(fixed(pool.read()), expected);
                assert_eq!(pool.read().slice_use.get(), live.len().c_cast::<u32>());
            }
            for (index, value) in live.iter().enumerate() {
                assert!(free(&*pool, *value));
                assert_eq!(fixed(pool.read()), expected);
                assert_eq!(
                    pool.read().slice_use.get(),
                    count - index.c_cast::<u32>() - 1
                );
            }
        }
    }
}

/// Source-backed initialization regression. Only the repaired implementation is
/// executed: every observed byte has been explicitly initialized before reading.
#[test]
fn initialized_fixed_headers_keep_native_zero_padding_through_normal_operations() {
    for width in [1, 7, 8, 17] {
        for owned in [false, true] {
            let layout = if owned {
                Layout::new(3, width, 0, true).unwrap()
            } else {
                borrowed(width, 3)
            };
            let (storage, pool) = pool_in(layout, false, owned);
            unsafe {
                let verify = || {
                    let base = pool.cast::<u8>();
                    for i in mem::offset_of!(Fixed, allocated) + 1..mem::size_of::<Fixed>() {
                        assert_eq!(base.add(i).read(), 0);
                    }
                    for i in 0..3 {
                        let header = base.add(
                            mem::size_of::<Fixed>()
                                + i * (mem::size_of::<Slice>() + layout.width as usize),
                        );
                        for j in mem::size_of::<u8>()..mem::offset_of!(Slice, next) {
                            assert_eq!(header.add(j).read(), 0);
                        }
                    }
                    let end = storage.as_ptr().cast::<u8>();
                    for i in layout.total..storage.len() * 8 {
                        assert_eq!(end.add(i).read(), 0xa5);
                    }
                };
                verify();
                let first = alloc(&*pool);
                let second = alloc(&*pool);
                assert!(!first.is_null() && !second.is_null());
                verify();
                assert!(free(&*pool, second));
                assert!(free(&*pool, first));
                verify();
            }
        }
    }
}
