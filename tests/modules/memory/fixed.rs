use super::*;
use crate::CCast;

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
    let node = pool.slice(slice)?;
    let record = DebugRecord {
        slice,
        prev: node.prev.get(),
        next: node.next.get(),
        data: slice.cast::<u8>().wrapping_add(mem::size_of::<Slice>()),
        line: cursor.line,
        lock: u8::from(node.lock.get()),
        self_link: node.next.get() == slice,
    };
    // Preserve the original post-print `line++ > max_lines` bound: max0 emits2.
    cursor.next = if max_lines < 0 || u64::try_from(max_lines).is_ok_and(|max| cursor.line > max) {
        ptr::null_mut()
    } else {
        node.next.get()
    };
    cursor.line += 1;
    Some(record)
}

/// A pool over a fresh `0xa5`-filled buffer of `plan.total` bytes plus one word.
fn pool_in(plan: Layout) -> (Vec<u64>, Fixed) {
    let mut memory = vec![0xa5a5_a5a5_a5a5_a5a5u64; plan.total.div_ceil(8) + 1];
    // The whole buffer's pointer: the tests read past the bytes the pool borrows.
    let base = memory.as_mut_ptr();
    let arena =
        unsafe { std::slice::from_raw_parts_mut(base.cast::<MaybeUninit<u8>>(), memory.len() * 8) };
    let pool = construct(arena, plan).unwrap();
    assert_eq!(pool.memory, base.cast::<u8>().wrapping_add(HEADER));
    (memory, pool)
}

fn borrowed(width: u32, slices: usize) -> Layout {
    let stride = mem::size_of::<Slice>() + (width as usize).next_multiple_of(8);
    Layout::new(0, width, HEADER + slices * stride, false).unwrap()
}

#[test]
fn fixed_pool_preserves_lifo_reuse_at_every_width() {
    for width in [1u32, 7, 8, 31, 256] {
        let (_memory, pool) = pool_in(borrowed(width, 17));
        let stride = pool.stride();
        unsafe {
            let base = pool.memory;
            let mut entries = Vec::new();
            for index in (0..17).rev() {
                let data = alloc(&pool);
                assert_eq!(
                    data.cast::<u8>(),
                    base.add(index * stride + mem::size_of::<Slice>())
                );
                ptr::write_bytes(data, index.c_cast::<u8>(), width as usize);
                entries.push(data);
            }
            assert!(alloc(&pool).is_null());
            assert_eq!(pool.slice_use.get(), 17);
            for &i in &[3, 10, 0] {
                assert!(free(&pool, entries[i]));
            }
            assert!(!free(&pool, entries[0]));
            assert!(!free(&pool, entries[0].cast::<u8>().add(1).cast()));
            for &i in &[0, 10, 3] {
                assert_eq!(alloc(&pool), entries[i]);
            }
            assert_eq!(pool.slice_use.get(), 17);
        }
    }
}

#[test]
fn fixed_construction_plans_round_widths_and_floor_borrowed_counts() {
    for width in [0u32, 1, 7, 8, 31, 256] {
        let plan = Layout::new(17, width, 0, true).unwrap();
        let aligned = (width + 7) & !7;
        assert_eq!(plan.width, aligned);
        assert_eq!(plan.slices, 17);
        assert_eq!(plan.total, 48 + 17 * (24 + aligned as usize));
        let available = 48 + 17 * (24 + aligned as usize) + 3;
        let plan = Layout::new(0, width, available, false).unwrap();
        assert_eq!(
            (plan.total, plan.slices, plan.width),
            (available, 17, aligned)
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
fn fixed_constructs_links_without_touching_payload_or_tail() {
    for owned in [false, true] {
        for width in [0u32, 1, 7, 8, 31, 256] {
            let plan = if owned {
                Layout::new(17, width, 0, true).unwrap()
            } else {
                borrowed(width, 17)
            };
            let (memory, pool) = pool_in(plan);
            let base = memory.as_ptr().cast::<u8>();
            assert_eq!(
                (pool.slice_num, pool.slice_use.get(), pool.slice_size),
                (17, 0, plan.width)
            );
            let stride = 24 + plan.width as usize;
            unsafe {
                for index in (0..17).rev() {
                    let data = alloc(&pool).cast::<u8>();
                    assert_eq!(data.cast_const(), base.add(48 + index * stride + 24));
                    assert!(
                        std::slice::from_raw_parts(data, plan.width as usize)
                            .iter()
                            .all(|&b| b == 0xa5)
                    );
                    assert!(free(&pool, data.cast()));
                    // Reuse consumes the same node; leave it live before the next allocation.
                    assert_eq!(alloc(&pool), data.cast());
                }
                assert!(alloc(&pool).is_null());
                // The reserved header and the bytes past the slices are untouched.
                let end = 48 + 17 * stride;
                let bytes = std::slice::from_raw_parts(base, memory.len() * 8);
                assert!(bytes[..48].iter().chain(&bytes[end..]).all(|&b| b == 0xa5));
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
    assert!(construct(arena, plan).is_none());
    assert_eq!(memory, before);
}

#[test]
fn diagnostic_cursor_matches_live_links_and_original_postprint_limit() {
    for width in [1u32, 8, 31] {
        let (_arena, pool) = pool_in(borrowed(width, 5));
        assert!(!alloc(&pool).is_null());
        for (max, expected) in [(-2, 1), (-1, 1), (0, 2), (1, 3), (3, 5), (99, 5)] {
            let mut cursor = DebugCursor::default();
            let mut next = pool.head.get();
            for line in 0..expected {
                let actual = pool.slice(next).unwrap();
                let record = debug_step(&pool, &mut cursor, max).unwrap();
                assert_eq!(
                    (
                        record.slice,
                        record.prev,
                        record.next,
                        record.line,
                        record.lock
                    ),
                    (
                        next,
                        actual.prev.get(),
                        actual.next.get(),
                        line,
                        u8::from(actual.lock.get())
                    )
                );
                assert_eq!(
                    record.data,
                    next.cast::<u8>().wrapping_add(mem::size_of::<Slice>())
                );
                assert!(!record.self_link);
                next = actual.next.get();
            }
            assert!(debug_step(&pool, &mut cursor, max).is_none());
        }
        // The original diagnostic explicitly marks self-links and bounds them.
        let head = pool.head.get();
        let header = pool.slice(head).unwrap();
        let original = header.next.replace(head);
        let mut cursor = DebugCursor::default();
        for line in 0..2 {
            let record = debug_step(&pool, &mut cursor, 0).unwrap();
            assert_eq!(
                (record.line, record.self_link, record.slice),
                (line, true, head)
            );
        }
        assert!(debug_step(&pool, &mut cursor, 0).is_none());
        header.next.set(original);
    }
}
