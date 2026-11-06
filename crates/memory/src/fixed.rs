use std::ffi::c_void;
use std::mem::offset_of;
use std::{mem, ptr};

// Slice invariant: every slice pointer reachable from a pool's `head`, `tail` or a
// slice's links is one of `slice_num` slices, each `size_of::<Slice>() + slice_size`
// bytes, laid out inside `memory` by `fixed_init`. Strides may be odd, so slice headers
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

#[repr(C)]
#[derive(Default)]
pub struct Usage {
    spare: u32,
    total: u32,
    width: u32,
}
/// Independent read-only snapshot; serialized with pool mutation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_fixed_usage(pool: *const Fixed, output: *mut Usage) -> i32 {
    if output.is_null() {
        return -1;
    }
    // SAFETY: `output` is non-null (checked above), aligned and writable per the caller contract.
    unsafe { output.write(Usage::default()) };
    if pool.is_null() {
        return -1;
    }
    // SAFETY: `pool` is non-null (checked above), aligned and initialized per the caller contract.
    let pool = unsafe { &*pool };
    // SAFETY: `output` is non-null (checked above), aligned and writable.
    unsafe {
        output.write(Usage {
            spare: pool.slice_num.wrapping_sub(pool.slice_use),
            total: pool.slice_num,
            width: pool.slice_size,
        });
    };
    0
}

/// Shared arena ownership actions:0 no effect,1/2 heap/shared allocation,
///3/4 heap/shared release. Native effects execute only after this call returns.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_memory_arena_lifecycle(shared: u32, owned: u32, phase: u32) -> i32 {
    if shared > 1 || owned > 1 || phase > 1 {
        -1
    } else if owned == 0 {
        0
    } else {
        (1 + shared + 2 * phase).cast_signed()
    }
}

#[repr(C)]
#[derive(Default)]
pub struct DebugCursor {
    next: *mut Slice,
    line: u64,
}
#[repr(C)]
#[derive(Default)]
pub struct DebugRecord {
    pub(crate) slice: *mut Slice,
    pub(crate) prev: *mut Slice,
    pub(crate) next: *mut Slice,
    pub(crate) data: *mut u8,
    pub(crate) line: u64,
    pub(crate) lock: u8,
    pub(crate) self_link: u8,
}

/// Serialized diagnostic traversal of actual linked storage, including odd widths.
/// Independent cursor/output are disjoint from pool and arena; no native callbacks.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_fixed_debug_step(
    pool: *const Fixed,
    cursor: *mut DebugCursor,
    max_lines: i32,
    output: *mut DebugRecord,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    // SAFETY: `output` is non-null (checked above), aligned and writable per the caller contract.
    unsafe { output.write(DebugRecord::default()) };
    if pool.is_null() || cursor.is_null() {
        return -1;
    }
    // SAFETY: `cursor` is non-null (checked above), aligned and exclusive per the caller contract.
    let cursor = unsafe { &mut *cursor };
    let slice = if cursor.line == 0 {
        // SAFETY: `pool` is non-null (checked above), aligned and initialized per the caller contract.
        unsafe { (*pool).head }
    } else {
        cursor.next
    };
    if slice.is_null() {
        return 0;
    }
    // SAFETY: a slice of this pool (see the slice invariant), read unaligned.
    let node = unsafe { slice.read_unaligned() };
    let Some(next_line) = cursor.line.checked_add(1) else {
        return -1;
    };
    let record = DebugRecord {
        slice,
        prev: node.prev,
        next: node.next,
        data: slice.cast::<u8>().wrapping_add(mem::size_of::<Slice>()),
        line: cursor.line,
        lock: node.lock,
        self_link: u8::from(node.next == slice),
    };
    // Preserve the original post-print `line++ > max_lines` bound: max0 emits2.
    cursor.next = if max_lines < 0 || u64::try_from(max_lines).is_ok_and(|max| cursor.line > max) {
        ptr::null_mut()
    } else {
        node.next
    };
    cursor.line = next_line;
    // SAFETY: `output` is non-null (checked above), aligned and writable per the caller contract.
    unsafe { output.write(record) };
    1
}

/// Complete arena plan; the native facade only supplies allocation effects.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Layout {
    pub(crate) total: usize,
    usable: usize,
    slices: u32,
    width: u32,
}
impl Layout {
    fn new(count: u32, width: u32, available: usize, owned: u32) -> Option<Self> {
        let (total, usable, slices, width) = match owned {
            1 if count >= 2 && available == 0 => {
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
            0 if count == 0 => {
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_fixed_layout(
    count: u32,
    width: u32,
    available: usize,
    owned: u32,
    output: *mut Layout,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    // SAFETY: `output` is non-null (checked above), aligned and writable.
    unsafe {
        output.write(Layout::default());
    }
    let Some(layout) = Layout::new(count, width, available, owned) else {
        return -1;
    };
    // SAFETY: `output` is non-null (checked above), aligned and writable.
    unsafe {
        output.write(layout);
    }
    0
}
/// Initialize actual arena header and links after native allocation has returned.
/// `arena` covers layout.total exclusive bytes; layout is separate initialized storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_fixed_construct(
    arena: *mut Fixed,
    layout: *const Layout,
    shared: u32,
    owned: u32,
) -> i32 {
    if arena.is_null()
        || layout.is_null()
        || !arena.addr().is_multiple_of(mem::align_of::<Fixed>())
        || shared > 1
    {
        return -1;
    }
    // SAFETY: `layout` is non-null (checked above), aligned and initialized per the caller contract.
    let plan = unsafe { layout.read() };
    let expected = Layout::new(
        if owned == 1 { plan.slices } else { 0 },
        plan.width,
        if owned == 0 { plan.total } else { 0 },
        owned,
    );
    if expected != Some(plan) || arena.addr().checked_add(plan.total).is_none() {
        return -1;
    }
    // `arena` is non-null, aligned and covers `plan.total` exclusive bytes (checked above).
    // Preserve original whole-header zeroing, including ABI padding.
    // Whole repr(C) value writes can copy uninitialized Rust padding.
    // SAFETY: as above.
    unsafe { ptr::write_bytes(arena.cast::<u8>(), 0, mem::size_of::<Fixed>()) };
    let memory = arena.cast::<u8>().wrapping_add(mem::size_of::<Fixed>());
    // SAFETY: as above.
    unsafe { field::<_, *mut u8>(arena, offset_of!(Fixed, memory)).write(memory) };
    // SAFETY: as above.
    unsafe { field::<_, usize>(arena, offset_of!(Fixed, size)).write(plan.usable) };
    // SAFETY: as above.
    unsafe { field::<_, u32>(arena, offset_of!(Fixed, slice_num)).write(plan.slices) };
    // SAFETY: as above.
    unsafe { field::<_, u32>(arena, offset_of!(Fixed, slice_size)).write(plan.width) };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "native uint8_t flag fields"
    )]
    // SAFETY: as above.
    unsafe {
        field::<_, u8>(arena, offset_of!(Fixed, shared)).write(shared as u8);
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "native uint8_t flag fields"
    )]
    // SAFETY: as above.
    unsafe {
        field::<_, u8>(arena, offset_of!(Fixed, allocated)).write(owned as u8);
    };
    // SAFETY: the header is fully initialized and `memory` holds `plan.usable` bytes.
    unsafe { swr_v1_memory_fixed_init(arena) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_fixed_init(pool: *mut Fixed) -> i32 {
    if pool.is_null() {
        return -1;
    }
    // SAFETY: `pool` is non-null (checked above), aligned and exclusive per the caller contract.
    let p = unsafe { &mut *pool };
    let Some(stride) = mem::size_of::<Slice>().checked_add(p.slice_size as usize) else {
        return -1;
    };
    // Table row sizes may be odd: slice headers require unaligned loads/stores.
    if p.memory.is_null()
        || p.slice_num < 2
        || (p.slice_num as usize)
            .checked_mul(stride)
            .is_none_or(|n| n > p.size)
    {
        return -1;
    }
    p.head = ptr::null_mut();
    p.tail = p.memory.cast();
    for i in 0..p.slice_num as usize {
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "slice headers are only accessed unaligned (see the slice invariant)"
        )]
        // `slice_num * stride <= size` (checked above), so slice `i` lies inside `memory`.
        let node = p.memory.wrapping_add(i * stride).cast::<Slice>();
        // SAFETY: as above; the pool is exclusive during init.
        unsafe { ptr::write_bytes(node.cast::<u8>(), 0, mem::size_of::<Slice>()) };
        // SAFETY: as above.
        unsafe { next(node).write_unaligned(p.head) };
        if !p.head.is_null() {
            // SAFETY: `head` is the previously initialized slice.
            unsafe { prev(p.head).write_unaligned(node) };
        }
        p.head = node;
    }
    p.slice_use = 0;
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_fixed_alloc(pool: *mut Fixed) -> *mut c_void {
    if pool.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: `pool` is non-null (checked above), aligned and initialized per the caller contract.
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_fixed_free(pool: *mut Fixed, data: *mut c_void) -> i32 {
    if pool.is_null() || data.is_null() {
        return -1;
    }
    // SAFETY: `pool` is non-null (checked above), aligned and initialized per the caller contract.
    let mut p = unsafe { pool.read() };
    let Some(offset) = data
        .addr()
        .checked_sub(p.memory.addr() + mem::size_of::<Slice>())
    else {
        return -1;
    };
    let stride = mem::size_of::<Slice>() + p.slice_size as usize;
    if offset % stride != 0 || offset / stride >= p.slice_num as usize {
        return -1;
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
        return -1;
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
    0
}

#[cfg(test)]
#[path = "../../../tests/modules/memory/fixed.rs"]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "tests drive the native integer interfaces with literal and loop values"
)]
mod tests;
