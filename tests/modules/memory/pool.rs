use super::{Pool, SLOTS};

#[test]
fn live_addresses_survive_slab_exhaustion_and_retired_slots_are_reused() {
    let mut pool = Pool::<[usize; 4]>::default();
    let nodes: Vec<_> = (0..SLOTS * 3)
        .map(|i| pool.insert([i; 4]).unwrap())
        .collect();
    for (i, node) in nodes.iter().enumerate() {
        assert_eq!(unsafe { node.as_ref() }, &[i; 4]);
    }
    // Both slab and fallback allocations can be retired in arbitrary order.
    for node in nodes.iter().step_by(2) {
        unsafe { pool.remove(*node) };
    }
    for (i, node) in nodes.iter().enumerate().skip(1).step_by(2) {
        assert_eq!(unsafe { node.as_ref() }, &[i; 4]);
    }
    let reused = pool.insert([usize::MAX; 4]).unwrap();
    assert!(nodes[1..=SLOTS].contains(&reused));
    unsafe { pool.remove(reused) };
    for node in nodes.iter().skip(1).step_by(2) {
        unsafe { pool.remove(*node) };
    }
}
