use std::ffi::c_void;
use std::ptr;

/// Layout invariant: `mem` holds `size + maxlen + 4` bytes (see `StoragePlan`), so a
/// 4-byte length header plus up to `maxlen` record bytes starting at any cursor below
/// `size` stays inside it. `valid` checks every cursor and bound this relies on.
#[repr(C)]
pub struct Channel {
    head: i64,
    tail: i64,
    size: usize,
    head_tag: u8,
    tail_tag: u8,
    num: i32,
    max_num: i32,
    bytes: usize,
    flags: i32,
    maxlen: i32,
    mem: *mut u8,
}
/// A cursor `valid` has bounded to `0..size`, with `size <= isize::MAX`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "valid() bounds cursors to 0..size <= isize::MAX"
)]
const fn offset(cursor: i64) -> usize {
    cursor as usize
}
/// A size or packet length `valid` has bounded to `isize::MAX`.
#[expect(
    clippy::cast_possible_wrap,
    reason = "valid() bounds sizes to isize::MAX"
)]
const fn signed(size: usize) -> i64 {
    size as i64
}
#[expect(
    clippy::needless_bitwise_bool,
    reason = "branch-free validation of scalar POD fields on the push/pop path; see comment"
)]
fn valid(c: &Channel) -> bool {
    // These predicates only read scalar POD fields. Combine them eagerly so
    // native callers do not pay a dependent branch for every invariant. Keep
    // pointer dereferences and queue writes behind the combined result.
    (!c.mem.is_null())
        & (c.size > 0)
        & (c.size <= (isize::MAX as usize).saturating_sub(c.maxlen.max(0).unsigned_abs() as usize + 4))
        // A negative cursor fails the conversion, so these check both bounds.
        & usize::try_from(c.head).is_ok_and(|head| head < c.size)
        & usize::try_from(c.tail).is_ok_and(|tail| tail < c.size)
        & ((c.head_tag | c.tail_tag) <= 1)
        & (c.num >= 0)
        & (c.maxlen >= 0) & (c.maxlen.unsigned_abs() as usize <= c.size)
}
// Short channel records should not need a dynamic libc memcpy dispatch. Copy
// fixed-width endpoints, which may overlap each other but stay inside both
// disjoint borrowed ranges. No speculative byte access outside the record: a
// one-byte record at the end of a mapping needs only one readable byte.
#[inline(always)]
unsafe fn copy_record(input: *const u8, output: *mut u8, length: usize) {
    // Callers pass `length` readable input bytes and `length` writable, disjoint output
    // bytes; every copy below stays inside both.
    #[inline]
    unsafe fn endpoints<const N: usize>(input: *const u8, output: *mut u8, length: usize) {
        // SAFETY: as above, with `N <= length`.
        unsafe { ptr::copy_nonoverlapping(input, output, N) };
        // SAFETY: as above; the last `N` bytes start at `length - N`.
        unsafe {
            ptr::copy_nonoverlapping(
                input.wrapping_add(length - N),
                output.wrapping_add(length - N),
                N,
            );
        };
    }
    if length > 64 {
        // SAFETY: as above.
        unsafe { ptr::copy_nonoverlapping(input, output, length) };
    } else if length >= 32 {
        // SAFETY: as above.
        unsafe { endpoints::<32>(input, output, length) };
    } else if length >= 16 {
        // SAFETY: as above.
        unsafe { endpoints::<16>(input, output, length) };
    } else if length >= 8 {
        // SAFETY: as above.
        unsafe { endpoints::<8>(input, output, length) };
    } else if length >= 4 {
        // SAFETY: as above.
        unsafe { endpoints::<4>(input, output, length) };
    } else if length >= 2 {
        // SAFETY: as above.
        unsafe { endpoints::<2>(input, output, length) };
    } else if length == 1 {
        // SAFETY: as above.
        unsafe { ptr::copy_nonoverlapping(input, output, 1) };
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_channel_in(
    channel: *mut Channel,
    input: *const c_void,
    length: i32,
) -> i32 {
    if channel.is_null() || length < 0 || length > 0 && input.is_null() {
        return -1;
    }
    // SAFETY: `channel` is non-null (checked above), aligned and exclusive per the caller contract.
    let c = unsafe { &mut *channel };
    if !valid(c) || length > c.maxlen || c.num == i32::MAX {
        return -1;
    }
    let Some(used) = (c.num.unsigned_abs() as usize)
        .checked_mul(4)
        .and_then(|n| c.bytes.checked_add(n))
    else {
        return -1;
    };
    if c.head == c.tail && c.tail_tag != c.head_tag || used == c.size {
        return -1;
    }
    let packet = length.unsigned_abs() as usize + 4;
    if c.tail < c.head && (c.head - c.tail) < signed(packet) {
        return -1;
    }
    let Some(bytes) = c.bytes.checked_add(length.unsigned_abs() as usize) else {
        return -1;
    };
    // `tail < size` and `length <= maxlen` (validated), so the header and record fit in
    // `mem` (see `Channel`); the room check above keeps them clear of unread records.
    let item = c.mem.wrapping_add(offset(c.tail));
    // SAFETY: as above.
    unsafe { item.cast::<i32>().write_unaligned(length) };
    // SAFETY: as above; `input` lends `length` readable bytes, disjoint from `mem`.
    unsafe {
        copy_record(
            input.cast::<u8>(),
            item.wrapping_add(4),
            length.unsigned_abs() as usize,
        );
    };
    c.tail += signed(packet);
    if c.tail >= signed(c.size) {
        c.tail = 0;
        c.tail_tag = 1 - c.tail_tag;
    }
    c.num += 1;
    c.bytes = bytes;
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_channel_out(
    channel: *mut Channel,
    output: *mut c_void,
    capacity: i32,
    consume: u32,
) -> i32 {
    if channel.is_null() || capacity < 0 || consume > 1 {
        return -1;
    }
    // SAFETY: `channel` is non-null (checked above), aligned and initialized.
    let state = unsafe { &*channel };
    // SAFETY: `output` lends `capacity` writable bytes, disjoint from the channel memory.
    let length = unsafe { read_record(state, output, capacity) };
    if length >= 0 && consume != 0 {
        // SAFETY: `channel` is non-null (checked above), aligned and exclusive per the caller contract.
        let c = unsafe { &mut *channel };
        c.head += i64::from(length) + 4;
        if c.head >= signed(c.size) {
            c.head = 0;
            c.head_tag = 1 - c.head_tag;
        }
        c.num -= 1;
        c.bytes -= length.unsigned_abs() as usize;
    }
    length
}
unsafe fn read_record(c: &Channel, output: *mut c_void, capacity: i32) -> i32 {
    if capacity < 0 || !valid(c) || c.num == 0 {
        return -1;
    }
    // `head < size` (validated), so the 4-byte header is inside `mem` (see `Channel`).
    let item = c.mem.wrapping_add(offset(c.head));
    // SAFETY: as above; `num > 0` means a record header was written there.
    let length = unsafe { item.cast::<i32>().read_unaligned() };
    if length < 0
        || length > c.maxlen
        || length > capacity
        || length.unsigned_abs() as usize > c.bytes
        || length > 0 && output.is_null()
    {
        return -1;
    }
    // SAFETY: `length <= maxlen` record bytes follow the header inside `mem`; `output` has
    // `capacity >= length` writable, disjoint bytes.
    unsafe {
        copy_record(
            item.wrapping_add(4),
            output.cast::<u8>(),
            length.unsigned_abs() as usize,
        );
    };
    length
}
/// Independent descriptor, not a C++ object or prefix. Its disjoint typed targets
/// are live and serialized for this call; native Lock/Pipe objects never enter Rust.
#[repr(C)]
pub struct Fields {
    head: *mut i64,
    tail: *mut i64,
    size: *mut usize,
    head_tag: *mut u8,
    tail_tag: *mut u8,
    num: *mut i32,
    max_num: *mut i32,
    bytes: *mut usize,
    flags: *mut i32,
    maxlen: *mut i32,
    mem: *mut *mut u8,
}
impl Fields {
    fn valid(&self) -> bool {
        !self.head.is_null()
            && self.head.is_aligned()
            && !self.tail.is_null()
            && self.tail.is_aligned()
            && !self.size.is_null()
            && self.size.is_aligned()
            && !self.head_tag.is_null()
            && self.head_tag.is_aligned()
            && !self.tail_tag.is_null()
            && self.tail_tag.is_aligned()
            && !self.num.is_null()
            && self.num.is_aligned()
            && !self.max_num.is_null()
            && self.max_num.is_aligned()
            && !self.bytes.is_null()
            && self.bytes.is_aligned()
            && !self.flags.is_null()
            && self.flags.is_aligned()
            && !self.maxlen.is_null()
            && self.maxlen.is_aligned()
            && !self.mem.is_null()
            && self.mem.is_aligned()
    }
    // Callers check `valid` first: every target is non-null, aligned, live and serialized
    // for the call.
    unsafe fn load(&self) -> Channel {
        Channel {
            // SAFETY: as above.
            head: unsafe { self.head.read() },
            // SAFETY: as above.
            tail: unsafe { self.tail.read() },
            // SAFETY: as above.
            size: unsafe { self.size.read() },
            // SAFETY: as above.
            head_tag: unsafe { self.head_tag.read() },
            // SAFETY: as above.
            tail_tag: unsafe { self.tail_tag.read() },
            // SAFETY: as above.
            num: unsafe { self.num.read() },
            // SAFETY: as above.
            max_num: unsafe { self.max_num.read() },
            // SAFETY: as above.
            bytes: unsafe { self.bytes.read() },
            // SAFETY: as above.
            flags: unsafe { self.flags.read() },
            // SAFETY: as above.
            maxlen: unsafe { self.maxlen.read() },
            // SAFETY: as above.
            mem: unsafe { self.mem.read() },
        }
    }
    // Same precondition as `load`.
    unsafe fn publish(&self, state: &Channel, producer: bool) {
        if producer {
            // SAFETY: as above.
            unsafe { self.tail.write(state.tail) };
            // SAFETY: as above.
            unsafe { self.tail_tag.write(state.tail_tag) };
        } else {
            // SAFETY: as above.
            unsafe { self.head.write(state.head) };
            // SAFETY: as above.
            unsafe { self.head_tag.write(state.head_tag) };
        }
        // SAFETY: as above.
        unsafe { self.num.write(state.num) };
        // SAFETY: as above.
        unsafe { self.bytes.write(state.bytes) };
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct StoragePlan {
    total: usize,
    size: usize,
    header_size: usize,
    maxlen: i32,
    flags: i32,
}
impl StoragePlan {
    fn new(size: usize, length: usize, flags: i32, header_size: usize) -> Option<Self> {
        let maxlen = i32::try_from(length).ok()?;
        if size < length || header_size == 0 {
            return None;
        }
        let total = size
            .checked_add(length)?
            .checked_add(4)?
            .checked_add(header_size)?;
        isize::try_from(total).is_ok().then_some(Self {
            total,
            size,
            header_size,
            maxlen,
            flags,
        })
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_channel_storage_plan(
    size: usize,
    maxlen: usize,
    flags: i32,
    header_size: usize,
    output: *mut StoragePlan,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    // SAFETY: `output` is non-null (checked above), aligned and writable.
    unsafe {
        output.write(StoragePlan::default());
    }
    let Some(plan) = StoragePlan::new(size, maxlen, flags, header_size) else {
        return -1;
    };
    // SAFETY: `output` is non-null (checked above), aligned and writable.
    unsafe {
        output.write(plan);
    }
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_channel_storage_init(
    fields: *const Fields,
    plan: *const StoragePlan,
    memory: *mut c_void,
) -> i32 {
    if fields.is_null() || plan.is_null() || memory.is_null() {
        return -1;
    }
    // SAFETY: `fields` is non-null (checked above), aligned and initialized per the caller contract.
    let fields = unsafe { &*fields };
    // SAFETY: `plan` is non-null (checked above), aligned and initialized per the caller contract.
    let plan = unsafe { plan.read() };
    if !fields.valid()
        // A negative maxlen fails the conversion and is refused like any other mismatch.
        || usize::try_from(plan.maxlen)
            .ok()
            .and_then(|maxlen| StoragePlan::new(plan.size, maxlen, plan.flags, plan.header_size))
            != Some(plan)
        || memory
            .addr()
            .checked_add(plan.total - plan.header_size)
            .is_none()
    {
        return -1;
    }
    // Every field target passed `valid` (above): non-null, aligned, live, serialized.
    // SAFETY: as above.
    unsafe { fields.head.write(0) };
    // SAFETY: as above.
    unsafe { fields.tail.write(0) };
    // SAFETY: as above.
    unsafe { fields.size.write(plan.size) };
    // SAFETY: as above.
    unsafe { fields.head_tag.write(0) };
    // SAFETY: as above.
    unsafe { fields.tail_tag.write(0) };
    // SAFETY: as above.
    unsafe { fields.num.write(0) };
    // SAFETY: as above.
    unsafe { fields.max_num.write(0) };
    // SAFETY: as above.
    unsafe { fields.bytes.write(0) };
    // SAFETY: as above.
    unsafe { fields.flags.write(plan.flags) };
    // SAFETY: as above.
    unsafe { fields.maxlen.write(plan.maxlen) };
    // SAFETY: as above.
    unsafe { fields.mem.write(memory.cast()) };
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_channel_storage_in(
    fields: *const Fields,
    input: *const c_void,
    length: i32,
) -> i32 {
    if fields.is_null() {
        return -1;
    }
    // SAFETY: `fields` is non-null (checked above), aligned and initialized per the caller contract.
    let fields = unsafe { &*fields };
    if !fields.valid() {
        return -1;
    }
    // SAFETY: `fields` passed `valid` (above).
    let mut state = unsafe { fields.load() };
    // SAFETY: `state` is a local snapshot; the caller's buffer follows swr_v1_memory_channel_in's contract.
    let result = unsafe { swr_v1_memory_channel_in(&raw mut state, input, length) };
    if result == 0 {
        // SAFETY: `fields` passed `valid` (above).
        unsafe { fields.publish(&state, true) };
    }
    result
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_channel_storage_out(
    fields: *const Fields,
    output: *mut c_void,
    capacity: i32,
) -> i32 {
    if fields.is_null() {
        return -1;
    }
    // SAFETY: `fields` is non-null (checked above), aligned and initialized per the caller contract.
    let fields = unsafe { &*fields };
    if !fields.valid() {
        return -1;
    }
    // SAFETY: `fields` passed `valid` (above).
    let mut state = unsafe { fields.load() };
    // SAFETY: `state` is a local snapshot; the caller's buffer follows swr_v1_memory_channel_out's contract.
    let result = unsafe { swr_v1_memory_channel_out(&raw mut state, output, capacity, 1) };
    if result >= 0 {
        // SAFETY: `fields` passed `valid` (above).
        unsafe { fields.publish(&state, false) };
    }
    result
}

/// Only the initialized scalar fields read by public `Channel::full` cross ABI.
#[repr(C)]
pub struct CapacityState {
    head: i64,
    tail: i64,
    size: usize,
    bytes: usize,
    num: i32,
    head_tag: u8,
    tail_tag: u8,
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_channel_full(state: *const CapacityState) -> i32 {
    if state.is_null() {
        return -1;
    }
    // SAFETY: `state` is non-null (checked above), aligned and initialized per the caller contract.
    let state = unsafe { &*state };
    #[expect(
        clippy::cast_sign_loss,
        reason = "native size_t arithmetic on the int record count"
    )]
    let occupied = state
        .bytes
        .wrapping_add((state.num as usize).wrapping_mul(4));
    i32::from(
        state.head == state.tail && state.tail_tag != state.head_tag || occupied == state.size,
    )
}

/// Native effects execute only after this independent action plan returns.
#[repr(C)]
#[derive(Default)]
pub struct Lifecycle {
    count: u32,
    actions: [u32; 4],
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_channel_lifecycle(
    flags: i32,
    phase: u32,
    output: *mut Lifecycle,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    // SAFETY: `output` is non-null (checked above), aligned and writable.
    unsafe {
        output.write(Lifecycle::default());
    }
    if phase > 1 {
        return -1;
    }
    let mut plan = Lifecycle::default();
    let mut push = |action| {
        if let Some(slot) = plan.actions.get_mut(plan.count as usize) {
            *slot = action;
            plan.count += 1;
        }
    };
    if phase == 0 {
        push(if flags & 8 == 0 { 1 } else { 2 }); // allocate heap/shared
        push(3); // initialize scalar storage
        if flags & 2 != 0 {
            push(4);
        } // create native lock
        if flags & 4 != 0 {
            push(5);
        } // create native notification pipe
    } else {
        if flags & 2 != 0 {
            push(6);
        } // delete native lock
        if flags & 4 != 0 {
            push(7);
        } // close/delete native pipe
        push(if flags & 8 == 0 { 8 } else { 9 }); // release heap/shared
    }
    // SAFETY: `output` is non-null (checked above), aligned and writable.
    unsafe {
        output.write(plan);
    }
    0
}

#[cfg(test)]
#[path = "../../../tests/modules/memory/channel.rs"]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "tests drive the native integer interfaces with literal and loop values"
)]
mod tests;
