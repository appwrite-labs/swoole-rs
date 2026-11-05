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
/// Diagnostic/peek snapshot; native formatting happens after Rust returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_heap_debug_step(
    state: *const c_void,
    index: *mut usize,
    output: *mut Node,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe {
        output.write(Node {
            priority: 0,
            position: 0,
            data: ptr::null_mut(),
        })
    };
    if state.is_null() || index.is_null() {
        return -1;
    }
    let state = unsafe { &*state.cast::<Heap>() };
    let index = unsafe { &mut *index };
    let Some(node) = state.nodes.get(*index) else {
        return 0;
    };
    unsafe { output.write(node.as_ptr().read()) };
    *index += 1;
    1
}

struct Heap {
    nodes: Vec<NonNull<Node>>,
    pool: crate::storage::Pool<Node>,
    spare: Option<NonNull<Node>>,
    max: bool,
}
impl Heap {
    fn worse(&self, a: u64, b: u64) -> bool {
        if self.max { a < b } else { a > b }
    }
    fn place(&mut self, index: usize, node: NonNull<Node>) {
        self.nodes[index] = node;
        unsafe {
            (*node.as_ptr()).position = index as u32 + 1;
        }
    }
    fn up(&mut self, index: usize) {
        if self.max {
            self.up_ordered::<true>(index);
        } else {
            self.up_ordered::<false>(index);
        }
    }
    fn up_ordered<const MAX: bool>(&mut self, mut index: usize) {
        let node = self.nodes[index];
        // Placement changes positions, never the moving node's priority.
        let priority = unsafe { node.as_ref().priority };
        while index > 0 {
            let parent = (index - 1) / 2;
            if !worse_ordered::<MAX>(unsafe { self.nodes[parent].as_ref().priority }, priority) {
                break;
            }
            self.place(index, self.nodes[parent]);
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
        let node = self.nodes[index];
        let priority = unsafe { node.as_ref().priority };
        loop {
            let left = index * 2 + 1;
            if left >= self.nodes.len() {
                break;
            }
            let right = left + 1;
            let child = if right < self.nodes.len()
                && worse_ordered::<MAX>(unsafe { self.nodes[left].as_ref().priority }, unsafe {
                    self.nodes[right].as_ref().priority
                }) {
                right
            } else {
                left
            };
            if !worse_ordered::<MAX>(priority, unsafe { self.nodes[child].as_ref().priority }) {
                break;
            }
            self.place(index, self.nodes[child]);
            index = child;
        }
        self.place(index, node);
    }
    fn index(&self, node: NonNull<Node>) -> Option<usize> {
        let index = unsafe { node.as_ref().position }.checked_sub(1)? as usize;
        (self.nodes.get(index) == Some(&node)).then_some(index)
    }
    fn remove(&mut self, index: usize) -> *mut c_void {
        let node = self.nodes.swap_remove(index);
        if index < self.nodes.len() {
            if self.worse(unsafe { node.as_ref().priority }, unsafe {
                self.nodes[index].as_ref().priority
            }) {
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
        let data = unsafe { node.as_ref().data };
        unsafe { (*node.as_ptr()).position = 0 };
        if let Some(previous) = self.spare.replace(node) {
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
            unsafe {
                self.pool.remove(*node);
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_heap_create(
    capacity: usize,
    max: u32,
    output: *mut *mut c_void,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    unsafe {
        output.write(ptr::null_mut());
    }
    if max > 1 || capacity >= u32::MAX as usize {
        return -1;
    }
    let mut heap = Heap {
        nodes: Vec::new(),
        pool: Default::default(),
        spare: None,
        max: max == 1,
    };
    if heap.nodes.try_reserve_exact(capacity).is_err() {
        return -1;
    }
    let result = unsafe { alloc(Layout::new::<Heap>()).cast::<Heap>() };
    if result.is_null() {
        return -1;
    }
    unsafe {
        result.write(heap);
        output.write(result.cast());
    }
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_heap_release(heap: *mut c_void) {
    if !heap.is_null() {
        unsafe {
            ptr::drop_in_place(heap.cast::<Heap>());
            dealloc(heap.cast(), Layout::new::<Heap>());
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_heap_count(heap: *const c_void) -> usize {
    if heap.is_null() {
        0
    } else {
        unsafe { &*heap.cast::<Heap>() }.nodes.len()
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_heap_at(heap: *const c_void, index: usize) -> *mut Node {
    if heap.is_null() {
        return ptr::null_mut();
    }
    unsafe { &*heap.cast::<Heap>() }
        .nodes
        .get(index)
        .map_or(ptr::null_mut(), |node| node.as_ptr())
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_heap_compare(heap: *const c_void, a: u64, b: u64) -> i32 {
    if heap.is_null() {
        0
    } else {
        i32::from(unsafe { &*heap.cast::<Heap>() }.worse(a, b))
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_heap_push(
    heap: *mut c_void,
    priority: u64,
    data: *mut c_void,
) -> *mut Node {
    if heap.is_null() {
        return ptr::null_mut();
    }
    let heap = unsafe { &mut *heap.cast::<Heap>() };
    if heap.nodes.len() >= u32::MAX as usize - 1 || heap.nodes.try_reserve(1).is_err() {
        return ptr::null_mut();
    }
    let index = heap.nodes.len();
    let value = Node {
        priority,
        position: index as u32 + 1,
        data,
    };
    let node = if let Some(node) = heap.spare.take() {
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_heap_update(
    heap: *mut c_void,
    priority: u64,
    node: *mut Node,
) -> i32 {
    if heap.is_null() {
        return -1;
    }
    let Some(mut node) = NonNull::new(node) else {
        return -1;
    };
    let heap = unsafe { &mut *heap.cast::<Heap>() };
    let Some(index) = heap.index(node) else {
        return -1;
    };
    let old = unsafe { node.as_ref().priority };
    unsafe {
        node.as_mut().priority = priority;
    }
    if heap.worse(old, priority) {
        heap.up(index);
    } else {
        heap.down(index);
    }
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_heap_remove(heap: *mut c_void, node: *mut Node) -> i32 {
    if heap.is_null() {
        return -1;
    }
    let Some(node) = NonNull::new(node) else {
        return -1;
    };
    let heap = unsafe { &mut *heap.cast::<Heap>() };
    let Some(index) = heap.index(node) else {
        return -1;
    };
    heap.remove(index);
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_heap_pop(heap: *mut c_void) -> *mut c_void {
    if heap.is_null() {
        return ptr::null_mut();
    }
    let heap = unsafe { &mut *heap.cast::<Heap>() };
    heap.pop()
}

#[cfg(test)]
#[path = "../../../tests/modules/memory/heap.rs"]
mod tests;
