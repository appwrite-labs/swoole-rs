use super::*;
use std::collections::BTreeMap;
#[test]
fn arbitrary_heap_removal_and_updates_match_independent_min_max_oracle() {
    for max in [0u32, 1] {
        let mut handle = ptr::null_mut();
        unsafe {
            assert_eq!(swr_v1_memory_heap_create(0, max, &raw mut handle), 0);
            let mut model = BTreeMap::<usize, (*mut Node, u64)>::new();
            let mut seed = 17u64;
            for index in 1..6000usize {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                match seed % 4 {
                    0 => {
                        let priority = seed % 100;
                        let node = swr_v1_memory_heap_push(
                            handle,
                            priority,
                            ptr::without_provenance_mut(index),
                        );
                        assert!(!node.is_null());
                        model.insert(index, (node, priority));
                    }
                    1 | 2 if !model.is_empty() => {
                        let id = *model.keys().nth(seed as usize % model.len()).unwrap();
                        if seed % 4 == 1 {
                            assert_eq!(swr_v1_memory_heap_remove(handle, model[&id].0), 0);
                            model.remove(&id);
                        } else {
                            assert_eq!(
                                swr_v1_memory_heap_update(handle, seed % 100, model[&id].0),
                                0
                            );
                            model.get_mut(&id).unwrap().1 = seed % 100;
                        }
                    }
                    _ => {
                        if model.is_empty() {
                            assert!(swr_v1_memory_heap_pop(handle).is_null());
                        } else {
                            let priority = if max == 0 {
                                model.values().map(|x| x.1).min()
                            } else {
                                model.values().map(|x| x.1).max()
                            }
                            .unwrap();
                            let data = swr_v1_memory_heap_pop(handle).addr();
                            assert_eq!(model.remove(&data).unwrap().1, priority);
                        }
                    }
                }
                assert_eq!(swr_v1_memory_heap_count(handle), model.len());
                for (&id, &(node, priority)) in &model {
                    assert_eq!((*node).data.addr(), id);
                    assert_eq!((*node).priority, priority);
                    assert_eq!(
                        swr_v1_memory_heap_at(handle, (*node).position as usize - 1),
                        node
                    );
                }
            }
            swr_v1_memory_heap_release(handle);
        }
    }
}
#[test]
fn equal_priorities_keep_upstream_swap_last_order() {
    unsafe {
        let mut handle = ptr::null_mut();
        assert_eq!(swr_v1_memory_heap_create(3, 0, &raw mut handle), 0);
        for id in 1..=3 {
            assert!(!swr_v1_memory_heap_push(handle, 1, ptr::without_provenance_mut(id)).is_null());
        }
        for id in [1, 3, 2] {
            assert_eq!(swr_v1_memory_heap_pop(handle).addr(), id);
        }
        assert!(swr_v1_memory_heap_pop(handle).is_null());
        swr_v1_memory_heap_release(handle);
    }
}

#[test]
fn root_pop_zero_one_and_tied_min_max_boundaries_keep_swap_last_order() {
    unsafe {
        for max in [0, 1] {
            let mut heap = ptr::null_mut();
            assert_eq!(swr_v1_memory_heap_create(0, max, &raw mut heap), 0);
            for count in 0..=64 {
                for id in 1..=count {
                    let node = swr_v1_memory_heap_push(heap, 7, ptr::without_provenance_mut(id));
                    assert!(!node.is_null());
                }
                // All priorities tie: pinned pop emits the first inserted node,
                // then repeatedly takes the last slot without moving a child.
                for (remaining, expected) in (1..=count)
                    .rev()
                    .zip(std::iter::once(1).chain((2..=count).rev()))
                {
                    let root = swr_v1_memory_heap_at(heap, 0);
                    assert!(!root.is_null());
                    assert_eq!((*root).position, 1);
                    assert_eq!((*root).data.addr(), expected);
                    assert_eq!(swr_v1_memory_heap_pop(heap).addr(), expected);
                    assert_eq!(swr_v1_memory_heap_count(heap), remaining - 1);
                }
                assert!(swr_v1_memory_heap_pop(heap).is_null());
                assert!(swr_v1_memory_heap_at(heap, 0).is_null());
            }
            swr_v1_memory_heap_release(heap);
        }
    }
}

/// Upstream core-tests/src/core/heap.cpp `TEST(heap, random)`: a max heap of
/// `SIZE` (100) holding `SIZE * 2 - 1` nodes with random priorities in
/// [10000, 99999]; peek and every pop return a node whose priority is the one
/// pushed for its value. Upstream's single `print()` call is diagnostic output
/// with no assertion; `Heap::print` left with `include/swoole_heap.h`.
#[test]
fn heap_random() {
    use std::hash::BuildHasher;
    struct Pair {
        pri: u64,
        val: usize,
    }
    const SIZE: usize = 100;
    let random = std::hash::RandomState::new();
    unsafe {
        let mut pq = ptr::null_mut();
        assert_eq!(swr_v1_memory_heap_create(SIZE, 1, &raw mut pq), 0);
        assert!(swr_v1_memory_heap_at(pq, 0).is_null());
        let mut map = BTreeMap::new();
        for i in 0..SIZE * 2 - 1 {
            let pri = 10000 + random.hash_one(i) % 90000;
            let ns = Box::into_raw(Box::new(Pair { pri, val: i }));
            assert!(!swr_v1_memory_heap_push(pq, pri, ns.cast()).is_null());
            map.insert(i, pri);
        }
        let n = (*swr_v1_memory_heap_at(pq, 0)).data.cast::<Pair>();
        assert_eq!(map[&(*n).val], (*n).pri);
        loop {
            let n = swr_v1_memory_heap_pop(pq).cast::<Pair>();
            if n.is_null() {
                break;
            }
            assert_eq!(map[&(*n).val], (*n).pri);
            drop(Box::from_raw(n));
        }
        swr_v1_memory_heap_release(pq);
    }
}
