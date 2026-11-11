use std::cell::UnsafeCell;
use std::ffi::c_int;
use std::mem::MaybeUninit;
use swoole_rs_corebase::sys;

/// Layout invariant: `mem` holds `size + maxlen + 4` bytes (see
/// `swr_v1_memory_channel_bytes`) outside the header, which only the holder of the
/// queue's `&mut` touches, so a 4-byte length header plus up to `maxlen` record bytes
/// starting at any cursor below `size` stays inside it. `valid` checks every cursor
/// and bound this relies on. Only `swr_v1_memory_channel_init` (and this module's
/// tests, over their own buffers) make one.
#[repr(C)]
#[derive(Clone, Copy)]
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

impl Channel {
    /// The queue's storage, once `valid` vouches for its bounds.
    fn storage(&mut self) -> Option<&mut [u8]> {
        if !valid(self) {
            return None;
        }
        let length = self.size + self.maxlen.unsigned_abs() as usize + 4;
        // SAFETY: `mem` covers `length` bytes outside the header that only this `&mut`
        // reaches (the layout invariant); `valid` bounds `length` by `isize::MAX`.
        Some(unsafe { std::slice::from_raw_parts_mut(self.mem, length) })
    }
}

// Short channel records should not need a dynamic libc memcpy dispatch. Copy
// fixed-width endpoints, which may overlap each other but stay inside both
// ranges. No speculative byte access outside the record: a one-byte record at the
// end of a mapping needs only one readable byte.
#[inline(always)]
fn copy_record(input: &[u8], output: &mut [u8]) {
    #[inline]
    fn endpoints<const N: usize>(input: &[u8], output: &mut [u8]) {
        if let (Some(to), Some(from)) = (output.first_chunk_mut::<N>(), input.first_chunk::<N>()) {
            *to = *from;
        }
        if let (Some(to), Some(from)) = (output.last_chunk_mut::<N>(), input.last_chunk::<N>()) {
            *to = *from;
        }
    }
    match input.len() {
        65.. => output.copy_from_slice(input),
        32.. => endpoints::<32>(input, output),
        16.. => endpoints::<16>(input, output),
        8.. => endpoints::<8>(input, output),
        4.. => endpoints::<4>(input, output),
        2.. => endpoints::<2>(input, output),
        1 => endpoints::<1>(input, output),
        0 => {}
    }
}

/// `Channel::in`: appends `input` as one record; -1 when the queue is invalid, full or
/// the record longer than `maxlen`.
pub fn swr_v1_memory_channel_in(channel: &mut Channel, input: &[u8]) -> i32 {
    let mut c = *channel;
    let Ok(length) = i32::try_from(input.len()) else {
        return -1;
    };
    let Some(mem) = channel.storage() else {
        return -1;
    };
    if length > c.maxlen || c.num == i32::MAX {
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
    let packet = input.len() + 4;
    if c.tail < c.head && (c.head - c.tail) < signed(packet) {
        return -1;
    }
    let Some(bytes) = c.bytes.checked_add(input.len()) else {
        return -1;
    };
    // `tail < size` and `length <= maxlen` (validated), so the header and record fit in
    // `mem`; the room check above keeps them clear of unread records.
    let start = offset(c.tail);
    let Some((header, record)) = mem
        .get_mut(start..start + packet)
        .and_then(|item| item.split_first_chunk_mut::<4>())
    else {
        return -1;
    };
    *header = length.to_ne_bytes();
    copy_record(input, record);
    c.tail += signed(packet);
    if c.tail >= signed(c.size) {
        c.tail = 0;
        c.tail_tag = 1 - c.tail_tag;
    }
    c.num += 1;
    c.bytes = bytes;
    *channel = c;
    0
}

/// `Channel::out`: copies the front record into `output` and, when `consume`, removes
/// it; its length, or -1 when the queue is invalid or empty or `output` too short.
pub fn swr_v1_memory_channel_out(channel: &mut Channel, output: &mut [u8], consume: bool) -> i32 {
    let mut c = *channel;
    let capacity = i32::try_from(output.len()).unwrap_or(i32::MAX);
    let Some(mem) = channel.storage() else {
        return -1;
    };
    if c.num == 0 {
        return -1;
    }
    // `head < size` (validated), so the 4-byte header is inside `mem`; `num > 0` means
    // a record header was written there.
    let start = offset(c.head);
    let Some((header, rest)) = mem
        .get(start..)
        .and_then(|item| item.split_first_chunk::<4>())
    else {
        return -1;
    };
    let length = i32::from_ne_bytes(*header);
    if length < 0
        || length > c.maxlen
        || length > capacity
        || length.unsigned_abs() as usize > c.bytes
    {
        return -1;
    }
    let size = length.unsigned_abs() as usize;
    let (Some(record), Some(output)) = (rest.get(..size), output.get_mut(..size)) else {
        return -1;
    };
    copy_record(record, output);
    if consume {
        c.head += i64::from(length) + 4;
        if c.head >= signed(c.size) {
            c.head = 0;
            c.head_tag = 1 - c.head_tag;
        }
        c.num -= 1;
        c.bytes -= size;
        *channel = c;
    }
    length
}
const LOCK: i32 = 2;
const NOTIFY: i32 = 4;

/// `swoole::Channel`: the queue state, its process-shared mutex and its notification
/// pipe (read end first). Native allocates `swr_v1_memory_channel_bytes` bytes for it
/// (heap or shared mapping) and frees them after `close`; the queue storage follows
/// the record. Not `Sync`: threads (and processes) that share one serialize the queue
/// with its `SW_CHAN_LOCK` mutex, which [`Record::push`] and [`Record::pop`] take.
#[repr(C)]
pub struct Record {
    queue: UnsafeCell<Channel>,
    /// Initialized when `flags` has `SW_CHAN_LOCK`.
    lock: UnsafeCell<MaybeUninit<libc::pthread_mutex_t>>,
    notify: [c_int; 2],
    flags: i32,
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
    let fresh = Record {
        queue: UnsafeCell::new(queue),
        lock: UnsafeCell::new(MaybeUninit::uninit()),
        notify: [-1, -1],
        flags,
    };
    // SAFETY: the caller lends the record exclusively, aligned and long enough.
    unsafe { record.write(fresh) };
    // SAFETY: as above; written just now.
    let record = unsafe { &mut *record };
    if flags & NOTIFY != 0 {
        let Some(notify) = sys::pipe() else {
            return 1;
        };
        record.notify = notify;
    }
    if flags & LOCK != 0 && init_mutex(record.lock.get_mut()) != 0 {
        record.close_pipe();
        return 2;
    }
    0
}

/// A process-shared mutex, like upstream's `Mutex(true)`, in fresh storage.
fn init_mutex(lock: &mut MaybeUninit<libc::pthread_mutex_t>) -> c_int {
    let mut attr = MaybeUninit::<libc::pthread_mutexattr_t>::uninit();
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
        // SAFETY: `lock` is exclusively borrowed storage that holds no mutex yet (its
        // type says so: nothing reads it as one before this succeeds); `attr` is initialized.
        result = unsafe { libc::pthread_mutex_init(lock.as_mut_ptr(), attr.as_ptr()) };
    }
    // SAFETY: `attr` is initialized.
    unsafe { libc::pthread_mutexattr_destroy(attr.as_mut_ptr()) };
    result
}

/// `Channel::destroy` before native frees the memory: the mutex and the pipe.
/// # Safety
/// `record` came from `swr_v1_memory_channel_init` and has no other user, now or later.
pub unsafe fn swr_v1_memory_channel_close(record: *mut Record) {
    // SAFETY: as the caller promises.
    let record = unsafe { &mut *record };
    if record.flags & LOCK != 0 {
        // SAFETY: the mutex `init` made, which nobody holds or uses again.
        unsafe { libc::pthread_mutex_destroy(record.lock.get_mut().as_mut_ptr()) };
    }
    if record.flags & NOTIFY != 0 {
        record.close_pipe();
    }
}

/// The record's mutex, held from [`Record::hold`] until dropped.
struct Held<'a>(Option<&'a Record>);

impl Drop for Held<'_> {
    fn drop(&mut self) {
        if let Some(record) = self.0 {
            // SAFETY: the mutex this thread locked in `hold`.
            unsafe { libc::pthread_mutex_unlock(record.lock.get().cast()) };
        }
    }
}

impl Record {
    fn close_pipe(&mut self) {
        for fd in std::mem::replace(&mut self.notify, [-1, -1]) {
            if fd >= 0 {
                // The record owns this descriptor.
                sys::close(fd);
            }
        }
    }

    /// The record's mutex, with `SW_CHAN_LOCK`; nothing otherwise.
    fn hold(&self) -> Held<'_> {
        if self.flags & LOCK == 0 {
            return Held(None);
        }
        // SAFETY: a record made with `SW_CHAN_LOCK` holds the mutex `init` made, which
        // lives until `close`, after the record's last use.
        unsafe { libc::pthread_mutex_lock(self.lock.get().cast()) };
        Held(Some(self))
    }

    /// `f` over the queue, under the record's mutex.
    fn queue<R>(&self, f: impl FnOnce(&mut Channel) -> R) -> R {
        let _held = self.hold();
        // SAFETY: the mutex (or, without `SW_CHAN_LOCK`, the one thread a `Record` is
        // confined to) serializes the queue; the borrow ends with `f`.
        f(unsafe { &mut *self.queue.get() })
    }

    /// `Channel::push(data, length)`: 0, or -1 when it doesn't fit.
    pub fn push(&self, data: &[u8]) -> i32 {
        self.queue(|queue| swr_v1_memory_channel_in(queue, data))
    }

    /// `Channel::pop` (`consume`) or `Channel::peek` into `output`: the record's
    /// length, or -1 when the queue is empty or `output` too short.
    pub fn pop(&self, output: &mut [u8], consume: bool) -> i32 {
        self.queue(|queue| swr_v1_memory_channel_out(queue, output, consume))
    }

    /// `Channel::notify` (`true`: write one 8-byte token) or `Channel::wait` (read one,
    /// blocking): 0 done, -1 failed with errno intact.
    #[must_use]
    pub fn signal(&self, notify: bool) -> i32 {
        let [read, write] = self.notify;
        let mut value = u64::from(notify).to_ne_bytes();
        let length = value.len();
        loop {
            let done = if notify {
                sys::write(write, &value)
            } else {
                sys::read(read, &mut value)
            };
            if done < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
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
    pub fn print(&self) {
        let c = self.queue(|queue| *queue);
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
pub fn swr_v1_memory_channel_full(state: Option<&CapacityState>) -> i32 {
    let Some(state) = state else {
        return -1;
    };
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
