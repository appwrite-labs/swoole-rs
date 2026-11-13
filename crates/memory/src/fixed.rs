use std::cell::Cell;
use std::ffi::c_void;
use std::mem::offset_of;
use std::{mem, ptr};

// Slice invariant: every slice pointer reachable from a pool's `head`, `tail` or a
// slice's links is one of `slice_num` slices, each `size_of::<Slice>() + slice_size`
// bytes, laid out inside `memory` by `construct`. Strides may be odd, so slice headers
// are only read and written unaligned, through [`Slices`], which also refuses any
// address that is not a slice's.

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Slice {
    lock: u8,
    next: *mut Slice,
    prev: *mut Slice,
}
/// `swoole::FixedPool`'s header. Only `construct` makes one, in place, in front of its
/// slices (the slice invariant), and it stays there, so a `&Fixed` is a constructed
/// pool. It is not `Sync`: threads and processes that share an arena (a table's)
/// serialize allocation and release with the arena's mutex before they reach it.
/// Immutable ABI getters may observe the configuration meanwhile, so only the queue
/// fields change, through their cells.
#[repr(C)]
pub struct Fixed {
    pub(crate) memory: *mut u8,
    pub(crate) size: usize,
    head: Cell<*mut Slice>,
    tail: Cell<*mut Slice>,
    slice_num: u32,
    slice_use: Cell<u32>,
    pub(crate) slice_size: u32,
    pub(crate) shared: u8,
    pub(crate) allocated: u8,
}

/// The address of one field at `offset` inside `base`, computed without dereferencing it.
fn field<B, T>(base: *mut B, offset: usize) -> *mut T {
    base.wrapping_byte_add(offset).cast()
}

/// A pool's slice headers, bounds-checked against its arena.
struct Slices {
    memory: *mut u8,
    stride: usize,
    count: usize,
}

impl Slices {
    /// The slice at `node`'s address, rebuilt from the arena, if it is one.
    fn at(&self, node: *mut Slice) -> Option<*mut Slice> {
        let offset = node.addr().checked_sub(self.memory.addr())?;
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "slice headers are only accessed unaligned (see the slice invariant)"
        )]
        (offset % self.stride == 0 && offset / self.stride < self.count)
            .then(|| self.memory.wrapping_add(offset).cast::<Slice>())
    }

    fn get(&self, node: *mut Slice) -> Option<Slice> {
        let node = self.at(node)?;
        // SAFETY: a slice header of the pool (`at`), read unaligned; access to headers is
        // serialized (see `Fixed`).
        Some(unsafe { node.read_unaligned() })
    }

    /// Writes one header field (`T` at `offset`) of the slice at `node`, if it is one.
    fn set<T>(&self, node: *mut Slice, offset: usize, value: T) {
        debug_assert!(
            offset + mem::size_of::<T>() <= mem::size_of::<Slice>(),
            "a field of the slice header"
        );
        if let Some(node) = self.at(node) {
            // SAFETY: a field inside a slice header of the pool (`at`; callers pass a
            // field's offset and type), written unaligned; access to headers is
            // serialized (see `Fixed`).
            unsafe { field::<_, T>(node, offset).write_unaligned(value) };
        }
    }

    fn set_next(&self, node: *mut Slice, next: *mut Slice) {
        self.set(node, offset_of!(Slice, next), next);
    }

    fn set_prev(&self, node: *mut Slice, prev: *mut Slice) {
        self.set(node, offset_of!(Slice, prev), prev);
    }

    fn set_lock(&self, node: *mut Slice, lock: u8) {
        self.set(node, offset_of!(Slice, lock), lock);
    }
}

impl Fixed {
    fn nodes(&self) -> Slices {
        Slices {
            memory: self.memory,
            stride: mem::size_of::<Slice>() + self.slice_size as usize,
            count: self.slice_num as usize,
        }
    }

    /// Spare and total slices, as `get_number_of_spare_slice` and
    /// `get_number_of_total_slice` read them.
    pub(crate) fn slices(&self) -> (u32, u32) {
        (
            self.slice_num.wrapping_sub(self.slice_use.get()),
            self.slice_num,
        )
    }
}

/// Complete arena plan; the native facade only supplies allocation effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) total: usize,
    usable: usize,
    slices: u32,
    width: u32,
}
impl Layout {
    /// An owned arena of `count` slices (`available` 0), or a pool over `available`
    /// borrowed bytes (`count` 0).
    pub(crate) fn new(count: u32, width: u32, available: usize, owned: bool) -> Option<Self> {
        let (total, usable, slices, width) = match owned {
            true if count >= 2 && available == 0 => {
                let width = width.checked_add(7)? & !7;
                let usable =
                    (mem::size_of::<Slice>() + width as usize).checked_mul(count as usize)?;
                (
                    usable.checked_add(mem::size_of::<Fixed>())?,
                    usable,
                    count,
                    width,
                )
            }
            false if count == 0 => {
                let usable = available.checked_sub(mem::size_of::<Fixed>())?;
                let slices =
                    u32::try_from(usable / (mem::size_of::<Slice>() + width as usize)).ok()?;
                if slices < 2 {
                    return None;
                }
                (available, usable, slices, width)
            }
            _ => return None,
        };
        isize::try_from(total).is_ok().then_some(Self {
            total,
            usable,
            slices,
            width,
        })
    }
}
/// Lays a pool out in `arena`: its zeroed header, then `plan.slices` linked slices.
/// False, touching nothing, if `arena` is misaligned.
///
/// # Safety
/// `arena` covers `plan.total` exclusive bytes, which stay put while the pool is used.
pub(crate) unsafe fn construct(arena: *mut Fixed, plan: Layout, shared: bool, owned: bool) -> bool {
    if !arena.is_aligned() {
        return false;
    }
    // `arena` is aligned (checked above) and covers `plan.total` exclusive bytes, the
    // header and then `plan.usable`. Zero the whole header, padding included, as
    // upstream does: a repr(C) value write can copy uninitialized Rust padding.
    // SAFETY: as above.
    unsafe { ptr::write_bytes(arena.cast::<u8>(), 0, mem::size_of::<Fixed>()) };
    // SAFETY: as above; the header is now initialized (all-zero is a valid `Fixed`).
    let p = unsafe { &mut *arena };
    p.memory = arena.cast::<u8>().wrapping_add(mem::size_of::<Fixed>());
    p.size = plan.usable;
    p.slice_num = plan.slices;
    p.slice_size = plan.width;
    p.shared = shared.into();
    p.allocated = owned.into();
    // Table row sizes may be odd: slice headers require unaligned loads/stores.
    let nodes = p.nodes();
    p.tail.set(p.memory.cast());
    for i in 0..nodes.count {
        let node = p.memory.wrapping_add(i * nodes.stride);
        // `slice_num * stride <= usable` (`Layout::new`), so slice `i` lies inside `memory`.
        // SAFETY: as above; the pool is exclusive during construction.
        unsafe { ptr::write_bytes(node, 0, mem::size_of::<Slice>()) };
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "slice headers are only accessed unaligned (see the slice invariant)"
        )]
        let node = node.cast::<Slice>();
        nodes.set_next(node, p.head.get());
        nodes.set_prev(p.head.get(), node);
        p.head.set(node);
    }
    true
}

/// A spare slice, or null.
pub(crate) fn alloc(pool: &Fixed) -> *mut c_void {
    let nodes = pool.nodes();
    let Some(node) = nodes.at(pool.head.get()) else {
        return ptr::null_mut();
    };
    let Some(value) = nodes.get(node).filter(|value| value.lock == 0) else {
        return ptr::null_mut();
    };
    // At least two nodes always remain in the combined idle/busy list.
    let tail = pool.tail.get();
    pool.slice_use.set(pool.slice_use.get() + 1);
    pool.head.set(value.next);
    nodes.set_prev(value.next, ptr::null_mut());
    nodes.set_next(tail, node);
    nodes.set_lock(node, 1);
    nodes.set_next(node, ptr::null_mut());
    nodes.set_prev(node, tail);
    pool.tail.set(node);
    node.cast::<u8>()
        .wrapping_add(mem::size_of::<Slice>())
        .cast()
}

/// Returns `data`'s slice to the head of the pool; false if `data` is no allocated
/// slice of it.
pub(crate) fn free(pool: &Fixed, data: *mut c_void) -> bool {
    let nodes = pool.nodes();
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "slice headers are only accessed unaligned (see the slice invariant)"
    )]
    let slice = data
        .cast::<u8>()
        .wrapping_sub(mem::size_of::<Slice>())
        .cast::<Slice>();
    let Some(node) = nodes.at(slice) else {
        return false;
    };
    let Some(value) = nodes.get(node) else {
        return false;
    };
    if value.lock != 1 || pool.slice_use.get() == 0 {
        return false;
    }
    nodes.set_lock(node, 0);
    pool.slice_use.set(pool.slice_use.get() - 1);
    let head = pool.head.get();
    if node != head {
        if node == pool.tail.get() {
            nodes.set_next(value.prev, ptr::null_mut());
            pool.tail.set(value.prev);
        } else {
            nodes.set_next(value.prev, value.next);
            nodes.set_prev(value.next, value.prev);
        }
        nodes.set_next(node, head);
        nodes.set_prev(node, ptr::null_mut());
        nodes.set_prev(head, node);
        pool.head.set(node);
    }
    true
}

#[cfg(test)]
#[path = "../../../tests/modules/memory/fixed.rs"]
pub(crate) mod tests;
