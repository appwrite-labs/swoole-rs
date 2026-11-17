use std::cell::Cell;
use std::ffi::c_void;
use std::mem::{self, MaybeUninit};
use std::ptr;

// Slice invariant: every slice pointer reachable from a pool's `head`, `tail` or a
// slice's links is one of `slice_num` slices, each `size_of::<Slice>() + slice_size`
// bytes (a multiple of 8), laid out from `memory`, which is 8-aligned, by `construct`,
// which zeroes each header. So every header is an aligned, initialized `Slice`, which
// [`Fixed::slice`] lends after refusing any address that is not a slice's.

/// A slice header: whether it is allocated, and its links in the pool's list.
#[derive(Default)]
pub struct Slice {
    lock: Cell<bool>,
    next: Cell<*mut Slice>,
    prev: Cell<*mut Slice>,
}

/// Upstream's `sizeof(FixedPoolImpl)`, which an arena still reserves ahead of its
/// slices so its slice count and memory accounting stay upstream's.
pub(crate) const HEADER: usize = 48;

/// `swoole::FixedPool`'s state over an arena of slices. It is not `Sync`: threads and
/// processes that share one (a table's, in its shared header) serialize allocation
/// and release with the table's mutex before they reach it.
pub struct Fixed {
    pub(crate) memory: *mut u8,
    head: Cell<*mut Slice>,
    tail: Cell<*mut Slice>,
    slice_num: u32,
    slice_use: Cell<u32>,
    pub(crate) slice_size: u32,
}

impl Fixed {
    pub(crate) fn stride(&self) -> usize {
        mem::size_of::<Slice>() + self.slice_size as usize
    }

    /// The offset of the slice at `node`'s address, if it is one.
    fn offset(&self, node: *mut Slice) -> Option<usize> {
        let offset = node.addr().checked_sub(self.memory.addr())?;
        let stride = self.stride();
        (offset % stride == 0 && offset / stride < self.slice_num as usize).then_some(offset)
    }

    /// The slice header at `node`'s address, rebuilt from the arena, if it is one.
    fn slice(&self, node: *mut Slice) -> Option<&Slice> {
        let at = self.offset(node)?;
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "8-aligned: `memory` is, and so is the stride (the slice invariant)"
        )]
        let header = self.memory.wrapping_add(at).cast::<Slice>();
        // SAFETY: a slice header of the pool (the slice invariant), which is only
        // changed through its cells, by callers the table's mutex serializes.
        Some(unsafe { &*header })
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
    slices: u32,
    width: u32,
}
impl Layout {
    /// An owned arena of `count` slices (`available` 0), or a pool over `available`
    /// borrowed bytes (`count` 0). Widths round up to 8.
    pub(crate) fn new(count: u32, width: u32, available: usize, owned: bool) -> Option<Self> {
        let width = width.checked_add(7)? & !7;
        let stride = mem::size_of::<Slice>() + width as usize;
        let (total, slices) = match owned {
            true if count >= 2 && available == 0 => {
                let usable = stride.checked_mul(count as usize)?;
                (usable.checked_add(HEADER)?, count)
            }
            false if count == 0 => {
                let usable = available.checked_sub(HEADER)?;
                let slices = u32::try_from(usable / stride).ok()?;
                if slices < 2 {
                    return None;
                }
                (available, slices)
            }
            _ => return None,
        };
        isize::try_from(total).is_ok().then_some(Self {
            total,
            slices,
            width,
        })
    }
}

/// Lays a pool out in `arena`: `HEADER` reserved bytes, then `plan.slices` linked
/// slices, whose headers it zeroes. The pool, or `None`, touching nothing, if `arena`
/// is misaligned or shorter than `plan.total`. The pool's links point into `arena`,
/// which its owner keeps in place, and reaches only through the pool, while the pool
/// is used (the slice invariant).
pub(crate) fn construct(arena: &mut [MaybeUninit<u8>], plan: Layout) -> Option<Fixed> {
    let stride = mem::size_of::<Slice>() + plan.width as usize;
    let aligned = arena
        .as_ptr()
        .addr()
        .is_multiple_of(mem::align_of::<Slice>());
    if !aligned || arena.len() < plan.total {
        return None;
    }
    let slices = arena.get_mut(HEADER..)?;
    for i in 0..plan.slices as usize {
        let at = i * stride;
        let node = slices.get_mut(at..at + mem::size_of::<Slice>())?;
        node.fill(MaybeUninit::new(0));
    }
    let pool = Fixed {
        memory: slices.as_mut_ptr().cast(),
        head: Cell::new(ptr::null_mut()),
        tail: Cell::new(ptr::null_mut()),
        slice_num: plan.slices,
        slice_use: Cell::new(0),
        slice_size: plan.width,
    };
    pool.tail.set(pool.memory.cast());
    for i in 0..plan.slices as usize {
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "8-aligned: `memory` is, and so is the stride (the slice invariant)"
        )]
        let node = pool.memory.wrapping_add(i * stride).cast::<Slice>();
        let header = pool.slice(node)?;
        header.next.set(pool.head.get());
        if let Some(head) = pool.slice(pool.head.get()) {
            head.prev.set(node);
        }
        pool.head.set(node);
    }
    Some(pool)
}

/// A spare slice, or null.
pub(crate) fn alloc(pool: &Fixed) -> *mut c_void {
    let node = pool.head.get();
    let Some(at) = pool.offset(node) else {
        return ptr::null_mut();
    };
    let Some(slice) = pool.slice(node).filter(|slice| !slice.lock.get()) else {
        return ptr::null_mut();
    };
    // At least two nodes always remain in the combined idle/busy list.
    let (next, tail) = (slice.next.get(), pool.tail.get());
    pool.slice_use.set(pool.slice_use.get() + 1);
    pool.head.set(next);
    if let Some(next) = pool.slice(next) {
        next.prev.set(ptr::null_mut());
    }
    if let Some(tail) = pool.slice(tail) {
        tail.next.set(node);
    }
    slice.lock.set(true);
    slice.next.set(ptr::null_mut());
    slice.prev.set(tail);
    pool.tail.set(node);
    pool.memory
        .wrapping_add(at + mem::size_of::<Slice>())
        .cast()
}

/// Returns `data`'s slice to the head of the pool; false if `data` is no allocated
/// slice of it.
pub(crate) fn free(pool: &Fixed, data: *mut c_void) -> bool {
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "only an address `Fixed::slice` checks"
    )]
    let node = data
        .cast::<u8>()
        .wrapping_sub(mem::size_of::<Slice>())
        .cast::<Slice>();
    let Some(slice) = pool.slice(node) else {
        return false;
    };
    if !slice.lock.get() || pool.slice_use.get() == 0 {
        return false;
    }
    slice.lock.set(false);
    pool.slice_use.set(pool.slice_use.get() - 1);
    let head = pool.head.get();
    if node != head {
        let (prev, next) = (slice.prev.get(), slice.next.get());
        if node == pool.tail.get() {
            if let Some(prev) = pool.slice(prev) {
                prev.next.set(ptr::null_mut());
            }
            pool.tail.set(prev);
        } else {
            if let Some(prev) = pool.slice(prev) {
                prev.next.set(next);
            }
            if let Some(next) = pool.slice(next) {
                next.prev.set(prev);
            }
        }
        slice.next.set(head);
        slice.prev.set(ptr::null_mut());
        if let Some(head) = pool.slice(head) {
            head.prev.set(node);
        }
        pool.head.set(node);
    }
    true
}

#[cfg(test)]
#[path = "../../../../tests/modules/memory/fixed.rs"]
pub(crate) mod tests;
