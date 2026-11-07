use std::ffi::c_void;
use std::mem::offset_of;
use std::{mem, ptr};

// Slice invariant: every slice pointer reachable from a pool's `head`, `tail` or a
// slice's links is one of `slice_num` slices, each `size_of::<Slice>() + slice_size`
// bytes, laid out inside `memory` by `construct`. Strides may be odd, so slice headers
// are only read and written unaligned, through `field` addresses.

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Slice {
    lock: u8,
    next: *mut Slice,
    prev: *mut Slice,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Fixed {
    pub(crate) memory: *mut u8,
    pub(crate) size: usize,
    head: *mut Slice,
    tail: *mut Slice,
    slice_num: u32,
    slice_use: u32,
    pub(crate) slice_size: u32,
    pub(crate) shared: u8,
    pub(crate) allocated: u8,
}

// The address of one field at `offset` inside `base`, computed without dereferencing it.
fn field<B, T>(base: *mut B, offset: usize) -> *mut T {
    base.wrapping_byte_add(offset).cast()
}
fn next(node: *mut Slice) -> *mut *mut Slice {
    field(node, offset_of!(Slice, next))
}
fn prev(node: *mut Slice) -> *mut *mut Slice {
    field(node, offset_of!(Slice, prev))
}
fn lock(node: *mut Slice) -> *mut u8 {
    field(node, offset_of!(Slice, lock))
}

impl Fixed {
    // Allocation/release are externally serialized, but immutable ABI getters
    // may observe configuration. Never give Rust an exclusive whole-header
    // reference or rewrite those immutable fields during a queue operation.
    unsafe fn publish_queue(&self, target: *mut Fixed) {
        // Callers pass the live, aligned pool header; only the queue fields are written.
        // SAFETY: as above.
        unsafe { field::<_, *mut Slice>(target, offset_of!(Fixed, head)).write(self.head) };
        // SAFETY: as above.
        unsafe { field::<_, *mut Slice>(target, offset_of!(Fixed, tail)).write(self.tail) };
        // SAFETY: as above.
        unsafe { field::<_, u32>(target, offset_of!(Fixed, slice_use)).write(self.slice_use) };
    }
}

impl Fixed {
    /// Spare and total slices, as `get_number_of_spare_slice` and
    /// `get_number_of_total_slice` read them.
    pub(crate) fn slices(&self) -> (u32, u32) {
        (self.slice_num.wrapping_sub(self.slice_use), self.slice_num)
    }
}

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
///
/// # Safety
/// `pool` is a constructed pool; traversal is serialized with pool mutation.
pub(crate) unsafe fn debug_step(
    pool: &Fixed,
    cursor: &mut DebugCursor,
    max_lines: i32,
) -> Option<DebugRecord> {
    let slice = if cursor.line == 0 {
        pool.head
    } else {
        cursor.next
    };
    if slice.is_null() {
        return None;
    }
    // SAFETY: a slice of this pool (see the slice invariant), read unaligned.
    let node = unsafe { slice.read_unaligned() };
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
/// `arena` covers `plan.total` exclusive bytes.
pub(crate) unsafe fn construct(arena: *mut Fixed, plan: Layout, shared: bool, owned: bool) -> bool {
    if !arena.is_aligned() {
        return false;
    }
    // `arena` is aligned (checked above) and covers `plan.total` exclusive bytes, the
    // header and then `plan.usable`. Zero the whole header, padding included, as
    // upstream does: a repr(C) value write can copy uninitialized Rust padding.
    // SAFETY: as above.
    unsafe { ptr::write_bytes(arena.cast::<u8>(), 0, mem::size_of::<Fixed>()) };
    // SAFETY: as above; the header is now initialized.
    let p = unsafe { &mut *arena };
    p.memory = arena.cast::<u8>().wrapping_add(mem::size_of::<Fixed>());
    p.size = plan.usable;
    p.slice_num = plan.slices;
    p.slice_size = plan.width;
    p.shared = shared.into();
    p.allocated = owned.into();
    // Table row sizes may be odd: slice headers require unaligned loads/stores.
    let stride = mem::size_of::<Slice>() + p.slice_size as usize;
    p.tail = p.memory.cast();
    for i in 0..p.slice_num as usize {
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "slice headers are only accessed unaligned (see the slice invariant)"
        )]
        // `slice_num * stride <= usable` (`Layout::new`), so slice `i` lies inside `memory`.
        let node = p.memory.wrapping_add(i * stride).cast::<Slice>();
        // SAFETY: as above; the pool is exclusive during construction.
        unsafe { ptr::write_bytes(node.cast::<u8>(), 0, mem::size_of::<Slice>()) };
        // SAFETY: as above.
        unsafe { next(node).write_unaligned(p.head) };
        if !p.head.is_null() {
            // SAFETY: `head` is the previously initialized slice.
            unsafe { prev(p.head).write_unaligned(node) };
        }
        p.head = node;
    }
    true
}

/// A spare slice, or null.
///
/// # Safety
/// `pool` is a constructed pool; allocation is serialized.
pub(crate) unsafe fn alloc(pool: *mut Fixed) -> *mut c_void {
    // SAFETY: as the caller promises.
    let mut p = unsafe { pool.read() };
    let node = p.head;
    // Every slice touched below is a slice of this pool (see the slice invariant);
    // allocation is externally serialized.
    // SAFETY: as above.
    if node.is_null() || unsafe { node.read_unaligned().lock } != 0 {
        return ptr::null_mut();
    }
    // At least two nodes always remain in the combined idle/busy list.
    // SAFETY: as above.
    let value = unsafe { node.read_unaligned() };
    p.slice_use += 1;
    p.head = value.next;
    // SAFETY: as above.
    unsafe { prev(p.head).write_unaligned(ptr::null_mut()) };
    // SAFETY: as above.
    unsafe { next(p.tail).write_unaligned(node) };
    // SAFETY: as above.
    unsafe { lock(node).write(1) };
    // SAFETY: as above.
    unsafe { next(node).write_unaligned(ptr::null_mut()) };
    // SAFETY: as above.
    unsafe { prev(node).write_unaligned(p.tail) };
    p.tail = node;
    // SAFETY: `pool` is the live, aligned header read above.
    unsafe { p.publish_queue(pool) };
    node.cast::<u8>()
        .wrapping_add(mem::size_of::<Slice>())
        .cast()
}

/// Returns `data`'s slice to the head of the pool; false if `data` is no allocated
/// slice of it.
///
/// # Safety
/// `pool` is a constructed pool; release is serialized.
pub(crate) unsafe fn free(pool: *mut Fixed, data: *mut c_void) -> bool {
    // SAFETY: as the caller promises.
    let mut p = unsafe { pool.read() };
    let Some(offset) = data
        .addr()
        .checked_sub(p.memory.addr() + mem::size_of::<Slice>())
    else {
        return false;
    };
    let stride = mem::size_of::<Slice>() + p.slice_size as usize;
    if offset % stride != 0 || offset / stride >= p.slice_num as usize {
        return false;
    }
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "slice headers are only accessed unaligned (see the slice invariant)"
    )]
    // `offset` is a whole number of strides below `slice_num` (checked above), so `node` is a
    // slice of this pool (see the slice invariant); release is externally serialized.
    let node = p.memory.wrapping_add(offset).cast::<Slice>();
    // SAFETY: as above.
    let value = unsafe { node.read_unaligned() };
    if value.lock != 1 || p.slice_use == 0 {
        return false;
    }
    // Every link below is a slice of this pool (see the slice invariant).
    // SAFETY: as above.
    unsafe { lock(node).write_unaligned(0) };
    p.slice_use -= 1;
    if node != p.head {
        if node == p.tail {
            // SAFETY: as above.
            unsafe { next(value.prev).write_unaligned(ptr::null_mut()) };
            p.tail = value.prev;
        } else {
            // SAFETY: as above.
            unsafe { next(value.prev).write_unaligned(value.next) };
            // SAFETY: as above.
            unsafe { prev(value.next).write_unaligned(value.prev) };
        }
        // SAFETY: as above.
        unsafe { next(node).write_unaligned(p.head) };
        // SAFETY: as above.
        unsafe { prev(node).write_unaligned(ptr::null_mut()) };
        // SAFETY: as above.
        unsafe { prev(p.head).write_unaligned(node) };
        p.head = node;
    }
    // SAFETY: `pool` is the live, aligned header read above.
    unsafe { p.publish_queue(pool) };
    true
}

#[cfg(test)]
#[path = "../../../tests/modules/memory/fixed.rs"]
#[expect(
    clippy::cast_possible_truncation,
    reason = "tests drive the native integer interfaces with literal and loop values"
)]
mod tests;
