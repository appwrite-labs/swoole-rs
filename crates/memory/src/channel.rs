use std::ffi::{c_int, c_void};
use std::ptr;
use swoole_rs_corebase::sys;

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
pub unsafe fn swr_v1_memory_channel_in(
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
pub unsafe fn swr_v1_memory_channel_out(
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
const LOCK: i32 = 2;
const NOTIFY: i32 = 4;

/// `swoole::Channel`: the queue state, then its process-shared mutex and its
/// notification pipe (read end first). Native allocates `swr_v1_memory_channel_bytes`
/// bytes for it (heap or shared mapping) and frees them after `close`; the queue
/// storage follows the record.
#[repr(C)]
pub struct Record {
    queue: Channel,
    lock: libc::pthread_mutex_t,
    notify: [c_int; 2],
}

/// Bytes `Channel::make` allocates for a channel of `size` bytes and `maxlen`-byte
/// records: the record plus `size + maxlen + 4` of storage. 0 for an invalid or
/// unrepresentable configuration.
#[must_use]
pub fn swr_v1_memory_channel_bytes(size: usize, maxlen: usize) -> usize {
    if size < maxlen || i32::try_from(maxlen).is_err() {
        return 0;
    }
    size_of::<Record>()
        .checked_add(size)
        .and_then(|n| n.checked_add(maxlen))
        .and_then(|n| n.checked_add(4))
        .filter(|&n| isize::try_from(n).is_ok())
        .unwrap_or(0)
}

/// Initialize a fresh record and create its mutex (`SW_CHAN_LOCK`) and pipe
/// (`SW_CHAN_NOTIFY`): 0 ready, -1 invalid configuration, 1 `pipe()` failed with
/// errno intact, 2 the mutex failed. Nothing is left to release on failure.
/// # Safety
/// `record` is exclusively lent, aligned, and `swr_v1_memory_channel_bytes(size, maxlen)`
/// bytes long.
pub unsafe fn swr_v1_memory_channel_init(
    record: *mut Record,
    size: usize,
    maxlen: usize,
    flags: i32,
) -> i32 {
    let Ok(length) = i32::try_from(maxlen) else {
        return -1;
    };
    if record.is_null() || swr_v1_memory_channel_bytes(size, maxlen) == 0 {
        return -1;
    }
    let queue = Channel {
        head: 0,
        tail: 0,
        size,
        head_tag: 0,
        tail_tag: 0,
        num: 0,
        max_num: 0,
        bytes: 0,
        flags,
        maxlen: length,
        mem: record.wrapping_add(1).cast(),
    };
    // SAFETY: the caller lends the record exclusively; the queue is its first field.
    unsafe { record.cast::<Channel>().write(queue) };
    // SAFETY: as above.
    let notify = unsafe { &raw mut (*record).notify };
    // SAFETY: as above.
    unsafe { notify.write([-1, -1]) };
    // SAFETY: `notify` has room for the two descriptors.
    if flags & NOTIFY != 0 && unsafe { libc::pipe(notify.cast()) } != 0 {
        return 1;
    }
    // SAFETY: as above.
    let lock = unsafe { &raw mut (*record).lock };
    // SAFETY: `lock` is unused mutex storage inside the lent record.
    if flags & LOCK != 0 && unsafe { init_mutex(lock) } != 0 {
        // SAFETY: the record is still exclusively lent.
        unsafe { close_pipe(record) };
        return 2;
    }
    0
}

/// A process-shared mutex, like upstream's `Mutex(true)`.
unsafe fn init_mutex(lock: *mut libc::pthread_mutex_t) -> c_int {
    let mut attr = std::mem::MaybeUninit::<libc::pthread_mutexattr_t>::uninit();
    // SAFETY: `attr` is writable attribute storage.
    let mut result = unsafe { libc::pthread_mutexattr_init(attr.as_mut_ptr()) };
    if result != 0 {
        return result;
    }
    // SAFETY: `attr` was initialized above.
    result = unsafe {
        libc::pthread_mutexattr_setpshared(attr.as_mut_ptr(), libc::PTHREAD_PROCESS_SHARED)
    };
    if result == 0 {
        // SAFETY: `lock` is writable, unused mutex storage; `attr` is initialized.
        result = unsafe { libc::pthread_mutex_init(lock, attr.as_ptr()) };
    }
    // SAFETY: `attr` is initialized.
    unsafe { libc::pthread_mutexattr_destroy(attr.as_mut_ptr()) };
    result
}

unsafe fn close_pipe(record: *mut Record) {
    // SAFETY: the caller's record is live.
    for fd in unsafe { (*record).notify } {
        if fd >= 0 {
            // The record owns this descriptor.
            sys::close(fd);
        }
    }
}

/// `Channel::destroy` before native frees the memory: the mutex and the pipe.
/// # Safety
/// `record` came from `swr_v1_memory_channel_init` and has no other user.
pub unsafe fn swr_v1_memory_channel_close(record: *mut Record) {
    // SAFETY: a live record.
    let flags = unsafe { (*record).queue.flags };
    if flags & LOCK != 0 {
        // SAFETY: a live record.
        let lock = unsafe { &raw mut (*record).lock };
        // SAFETY: the mutex was initialized and nobody holds it.
        unsafe { libc::pthread_mutex_destroy(lock) };
    }
    if flags & NOTIFY != 0 {
        // SAFETY: as above.
        unsafe { close_pipe(record) };
    }
}

/// Lock (`true`) or unlock the channel's mutex.
/// # Safety
/// `record` is live with `SW_CHAN_LOCK`; an unlock follows this thread's lock.
pub unsafe fn swr_v1_memory_channel_lock(record: *mut Record, lock: bool) {
    // SAFETY: a live record with an initialized mutex.
    let mutex = unsafe { &raw mut (*record).lock };
    if lock {
        // SAFETY: as above.
        unsafe { libc::pthread_mutex_lock(mutex) };
    } else {
        // SAFETY: as above; this thread holds it.
        unsafe { libc::pthread_mutex_unlock(mutex) };
    }
}

/// `Channel::notify` (`true`: write one 8-byte token) or `Channel::wait` (read one,
/// blocking): 0 done, -1 failed with errno intact.
/// # Safety
/// `record` is live with `SW_CHAN_NOTIFY`.
#[must_use]
pub unsafe fn swr_v1_memory_channel_signal(record: *const Record, notify: bool) -> i32 {
    // SAFETY: a live record.
    let [read, write] = unsafe { (*record).notify };
    let mut value = u64::from(notify).to_ne_bytes();
    let length = value.len();
    loop {
        let done = if notify {
            sys::write(write, &value)
        } else {
            sys::read(read, &mut value)
        };
        if done < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        let complete = if notify {
            done.cast_unsigned() == length
        } else {
            done > 0
        };
        return if complete { 0 } else { -1 };
    }
}

/// `Channel::print`, through C stdio like upstream (which labels the tags swapped).
/// # Safety
/// `record` is live.
pub unsafe fn swr_v1_memory_channel_print(record: *const Record) {
    // SAFETY: a live record.
    let c = unsafe { &(*record).queue };
    // SAFETY: the format's conversions match the arguments.
    unsafe {
        libc::printf(
            c"Channel\n{\n    off_t head = %ld;\n    off_t tail = %ld;\n    size_t size = %ld;\n    char head_tag = %d;\n    char tail_tag = %d;\n    int num = %d;\n    size_t bytes = %ld;\n    int flag = %d;\n    int maxlen = %d;\n\n}\n".as_ptr(),
            c.head,
            c.tail,
            c.size,
            c_int::from(c.tail_tag),
            c_int::from(c.head_tag),
            c.num,
            c.bytes,
            c.flags,
            c.maxlen,
        )
    };
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
#[must_use]
pub unsafe fn swr_v1_memory_channel_full(state: *const CapacityState) -> i32 {
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

#[cfg(test)]
#[path = "../../../tests/modules/memory/channel.rs"]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "tests drive the native integer interfaces with literal and loop values"
)]
mod tests;
