use crate::pool::Pool;
use std::alloc::{Layout, alloc, dealloc};
use std::ffi::c_void;
use std::ptr::{self, NonNull};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Node {
    priority: u64,
    position: u32,
    data: *mut c_void,
}
/// Node invariant: every pointer in `nodes` and `spare` is a live `Node` allocation
/// owned by this heap's stable pool, retired only by `retire` (for the previous
/// spare) and `Drop`. `&mut self` makes node updates exclusive.
struct Heap {
    nodes: Vec<NonNull<Node>>,
    pool: Pool<Node>,
    spare: Option<NonNull<Node>>,
    max: bool,
}
impl Heap {
    fn worse(&self, a: u64, b: u64) -> bool {
        if self.max { a < b } else { a > b }
    }
    fn place(&mut self, index: usize, node: NonNull<Node>) {
        let Some(slot) = self.nodes.get_mut(index) else {
            return;
        };
        *slot = node;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "native uint32_t positions; insert refuses len >= u32::MAX - 1"
        )]
        // SAFETY: a node owned by this heap (see `Heap`).
        unsafe {
            (*node.as_ptr()).position = index as u32 + 1;
        };
    }
    fn up(&mut self, index: usize) {
        if self.max {
            self.up_ordered::<true>(index);
        } else {
            self.up_ordered::<false>(index);
        }
    }
    fn up_ordered<const MAX: bool>(&mut self, mut index: usize) {
        let Some(&node) = self.nodes.get(index) else {
            return;
        };
        // Placement changes positions, never the moving node's priority.
        // SAFETY: a node owned by this heap (see `Heap`).
        let priority = unsafe { node.as_ref().priority };
        while let Some(parent) = index.checked_sub(1).map(|i| i / 2)
            && let Some(&above) = self.nodes.get(parent)
            // SAFETY: a node owned by this heap (see `Heap`).
            && worse_ordered::<MAX>(unsafe { above.as_ref().priority }, priority)
        {
            self.place(index, above);
            index = parent;
        }
        self.place(index, node);
    }
    fn down(&mut self, index: usize) {
        if self.max {
            self.down_ordered::<true>(index);
        } else {
            self.down_ordered::<false>(index);
        }
    }
    fn down_ordered<const MAX: bool>(&mut self, mut index: usize) {
        let Some(&node) = self.nodes.get(index) else {
            return;
        };
        // SAFETY: a node owned by this heap (see `Heap`).
        let priority = unsafe { node.as_ref().priority };
        loop {
            let left = index * 2 + 1;
            let Some(&left_node) = self.nodes.get(left) else {
                break;
            };
            // SAFETY: a node owned by this heap (see `Heap`).
            let left_priority = unsafe { left_node.as_ref().priority };
            let mut child = (left, left_node);
            if let Some(&right) = self.nodes.get(left + 1) {
                // SAFETY: a node owned by this heap (see `Heap`).
                let right_priority = unsafe { right.as_ref().priority };
                if worse_ordered::<MAX>(left_priority, right_priority) {
                    child = (left + 1, right);
                }
            }
            let (child, child_node) = child;
            // SAFETY: a node owned by this heap (see `Heap`).
            if !worse_ordered::<MAX>(priority, unsafe { child_node.as_ref().priority }) {
                break;
            }
            self.place(index, child_node);
            index = child;
        }
        self.place(index, node);
    }
    fn index(&self, node: NonNull<Node>) -> Option<usize> {
        // SAFETY: callers pass a node returned by `push` and still live (see the exports).
        let index = unsafe { node.as_ref().position }.checked_sub(1)? as usize;
        (self.nodes.get(index) == Some(&node)).then_some(index)
    }
    fn remove(&mut self, index: usize) -> *mut c_void {
        let node = self.nodes.swap_remove(index);
        if let Some(&moved) = self.nodes.get(index) {
            // SAFETY: the removed node is still allocated until `retire`.
            let removed = unsafe { node.as_ref().priority };
            // SAFETY: a node owned by this heap (see `Heap`).
            if self.worse(removed, unsafe { moved.as_ref().priority }) {
                self.up(index);
            } else {
                self.down(index);
            }
        }
        self.retire(node)
    }
    fn pop(&mut self) -> *mut c_void {
        let Some(last) = self.nodes.pop() else {
            return ptr::null_mut();
        };
        // A root replacement can only sift down. In particular, do not route
        // pop through arbitrary-node removal's old/new-priority direction
        // check; upstream pop also always uses the left-child-tied down sift.
        let root = if let Some(slot) = self.nodes.first_mut() {
            let root = std::mem::replace(slot, last);
            self.down(0);
            root
        } else {
            last
        };
        self.retire(root)
    }
    fn retire(&mut self, node: NonNull<Node>) -> *mut c_void {
        // SAFETY: a node this heap owns, now detached from `nodes`.
        let data = unsafe { node.as_ref().data };
        // SAFETY: as above.
        unsafe { (*node.as_ptr()).position = 0 };
        if let Some(previous) = self.spare.replace(node) {
            // SAFETY: the previous spare was owned by this heap and nothing refers to it.
            unsafe { self.pool.remove(previous) };
        }
        data
    }
}
// The ordering is fixed for a heap's lifetime. Select it once per sift, rather
// than consulting the runtime flag for every parent/child comparison.
fn worse_ordered<const MAX: bool>(a: u64, b: u64) -> bool {
    if MAX { a < b } else { a > b }
}
impl Drop for Heap {
    fn drop(&mut self) {
        for node in self.nodes.iter().chain(self.spare.iter()) {
            // SAFETY: every owned node is freed exactly once, here.
            unsafe {
                self.pool.remove(*node);
            }
        }
    }
}

pub unsafe fn swr_v1_memory_heap_create(
    capacity: usize,
    max: u32,
    output: *mut *mut c_void,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    // SAFETY: `output` is non-null (checked above), aligned and writable.
    unsafe {
        output.write(ptr::null_mut());
    }
    if max > 1 || capacity >= u32::MAX as usize {
        return -1;
    }
    let mut heap = Heap {
        nodes: Vec::new(),
        pool: Pool::default(),
        spare: None,
        max: max == 1,
    };
    if heap.nodes.try_reserve_exact(capacity).is_err() {
        return -1;
    }
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "alloc honours Layout::new::<Heap>()"
    )]
    // SAFETY: `Heap` is not zero-sized; null is handled below.
    let result = unsafe { alloc(Layout::new::<Heap>()).cast::<Heap>() };
    if result.is_null() {
        return -1;
    }
    // SAFETY: a fresh, non-null allocation with `Heap`'s layout.
    unsafe { result.write(heap) };
    // SAFETY: `output` is non-null (checked above), aligned and writable.
    unsafe { output.write(result.cast()) };
    0
}
pub unsafe fn swr_v1_memory_heap_release(heap: *mut c_void) {
    if !heap.is_null() {
        // SAFETY: a live heap from `heap_create`, released exactly once.
        unsafe { ptr::drop_in_place(heap.cast::<Heap>()) };
        // SAFETY: allocated with this layout by create; its value was just dropped.
        unsafe { dealloc(heap.cast(), Layout::new::<Heap>()) };
    }
}
#[must_use]
pub unsafe fn swr_v1_memory_heap_count(heap: *const c_void) -> usize {
    if heap.is_null() {
        0
    } else {
        // SAFETY: `heap` is non-null (checked above), aligned and initialized per the caller contract.
        unsafe { &*heap.cast::<Heap>() }.nodes.len()
    }
}
#[must_use]
pub unsafe fn swr_v1_memory_heap_at(heap: *const c_void, index: usize) -> *mut Node {
    if heap.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: `heap` is non-null (checked above), aligned and initialized per the caller contract.
    unsafe { &*heap.cast::<Heap>() }
        .nodes
        .get(index)
        .map_or(ptr::null_mut(), |node| node.as_ptr())
}
pub unsafe fn swr_v1_memory_heap_push(
    heap: *mut c_void,
    priority: u64,
    data: *mut c_void,
) -> *mut Node {
    if heap.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: `heap` is non-null (checked above), aligned and exclusive per the caller contract.
    let heap = unsafe { &mut *heap.cast::<Heap>() };
    if heap.nodes.len() >= u32::MAX as usize - 1 || heap.nodes.try_reserve(1).is_err() {
        return ptr::null_mut();
    }
    let index = heap.nodes.len();
    let value = Node {
        priority,
        #[expect(
            clippy::cast_possible_truncation,
            reason = "insert refuses len >= u32::MAX - 1"
        )]
        position: index as u32 + 1,
        data,
    };
    let node = if let Some(node) = heap.spare.take() {
        // SAFETY: the spare is a live, detached Node owned by this heap.
        unsafe { node.as_ptr().write(value) };
        node
    } else {
        let Some(node) = heap.pool.insert(value) else {
            return ptr::null_mut();
        };
        node
    };
    heap.nodes.push(node);
    if index != 0 {
        heap.up(index);
    }
    node.as_ptr()
}
pub unsafe fn swr_v1_memory_heap_update(heap: *mut c_void, priority: u64, node: *mut Node) -> i32 {
    if heap.is_null() {
        return -1;
    }
    let Some(mut node) = NonNull::new(node) else {
        return -1;
    };
    // SAFETY: `heap` is non-null (checked above), aligned and exclusive per the caller contract.
    let heap = unsafe { &mut *heap.cast::<Heap>() };
    let Some(index) = heap.index(node) else {
        return -1;
    };
    // SAFETY: `index` confirmed `node` belongs to this heap.
    let old = unsafe { node.as_ref().priority };
    // SAFETY: as above; `&mut` heap access is exclusive.
    unsafe { node.as_mut().priority = priority };
    if heap.worse(old, priority) {
        heap.up(index);
    } else {
        heap.down(index);
    }
    0
}
pub unsafe fn swr_v1_memory_heap_remove(heap: *mut c_void, node: *mut Node) -> i32 {
    if heap.is_null() {
        return -1;
    }
    let Some(node) = NonNull::new(node) else {
        return -1;
    };
    // SAFETY: `heap` is non-null (checked above), aligned and exclusive per the caller contract.
    let heap = unsafe { &mut *heap.cast::<Heap>() };
    let Some(index) = heap.index(node) else {
        return -1;
    };
    heap.remove(index);
    0
}
pub unsafe fn swr_v1_memory_heap_pop(heap: *mut c_void) -> *mut c_void {
    if heap.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: `heap` is non-null (checked above), aligned and exclusive per the caller contract.
    let heap = unsafe { &mut *heap.cast::<Heap>() };
    heap.pop()
}

#[cfg(test)]
#[path = "../../../tests/modules/memory/heap.rs"]
#[expect(
    clippy::cast_possible_truncation,
    reason = "tests drive the native integer interfaces with literal and loop values"
)]
mod tests;
