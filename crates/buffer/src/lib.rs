//! Allocation-fallible owned buffer queue. See docs/buffer-abi-v1.md.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
mod typed;
use std::alloc::{Layout, alloc};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::{mem, ptr, slice};
pub use typed::{
    Detached as TypedDetached, Front as TypedFront, swr_v1_buffer_typed_consume,
    swr_v1_buffer_typed_create, swr_v1_buffer_typed_front, swr_v1_buffer_typed_pop,
    swr_v1_buffer_typed_push,
};

const OK: i32 = 0;
const EMPTY: i32 = 1;
const INVALID: i32 = -1;
const OVERFLOW: i32 = -2;
const NOMEM: i32 = -3;
const PENDING_NATIVE: i32 = -4;

/// A data chunk's length: chunks hold at most `chunk_size: u32` bytes (`appendv`).
#[expect(
    clippy::cast_possible_truncation,
    reason = "a data chunk holds at most chunk_size: u32 bytes"
)]
pub(crate) fn chunk_length(bytes: &[u8]) -> u32 {
    bytes.len() as u32
}

#[repr(C)]
pub struct BufferIovec {
    pub data: *const u8,
    pub length: usize,
}

trait Descriptor {
    fn data(&self) -> *const u8;
    fn length(&self) -> usize;
}
impl Descriptor for BufferIovec {
    fn data(&self) -> *const u8 {
        self.data
    }
    fn length(&self) -> usize {
        self.length
    }
}
#[cfg(unix)]
impl Descriptor for libc::iovec {
    fn data(&self) -> *const u8 {
        self.iov_base.cast()
    }
    fn length(&self) -> usize {
        self.iov_len
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct BufferView {
    pub kind: u32,
    pub length: u32,
    pub capacity: u32,
    pub data: *mut u8,
    pub token: *mut c_void,
}
impl BufferView {
    const ZERO: Self = Self {
        kind: 0,
        length: 0,
        capacity: 0,
        data: ptr::null_mut(),
        token: ptr::null_mut(),
    };
}

enum Entry {
    Data(Vec<u8>),
    Native(*mut c_void),
    TypedToken(typed::Token),
}
impl Entry {
    fn view(&self) -> BufferView {
        match self {
            Self::Data(bytes) => BufferView {
                kind: 0,
                length: chunk_length(bytes),
                capacity: chunk_length(bytes),
                data: bytes.as_ptr().cast_mut(),
                token: ptr::null_mut(),
            },
            Self::TypedToken(_) => unreachable!("typed entries never enter the legacy facade"),
            Self::Native(token) => BufferView {
                kind: 1,
                token: *token,
                ..BufferView::ZERO
            },
        }
    }
}

struct Detached {
    token: *mut c_void,
    reset_view: u32,
}

enum Mode {
    // Borrowed identity only; never dereferenced, owned or freed by Rust.
    Legacy(*mut c_void),
    Typed(typed::Cursor),
}

pub struct Buffer {
    entries: VecDeque<Entry>,
    recycled: Vec<Vec<u8>>,
    recycled_bytes: usize,
    chunk_size: u32,
    length: u32,
    mode: Mode,
}

// Validate representability before any slice construction or allocation.
fn slice_fits<T>(count: usize) -> bool {
    count <= (isize::MAX as usize) / mem::size_of::<T>()
}

impl Buffer {
    // One initialized owning value for both ordinary modes. No raw exported C
    // operation is used as an internal constructor or mutates a published owner.
    fn try_new(chunk_size: u32, mode: Mode) -> Result<Box<Self>, i32> {
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "alloc honours Layout::new::<Buffer>()"
        )]
        // SAFETY: `Buffer` is not zero-sized; null is handled below.
        let handle = unsafe { alloc(Layout::new::<Self>()) }.cast::<Self>();
        if handle.is_null() {
            return Err(NOMEM);
        }
        // SAFETY: a fresh, non-null allocation with `Buffer`'s layout.
        unsafe {
            handle.write(Self {
                entries: VecDeque::new(),
                recycled: Vec::new(),
                recycled_bytes: 0,
                chunk_size: if chunk_size == 0 {
                    i32::MAX as u32
                } else {
                    chunk_size
                },
                length: 0,
                mode,
            });
        };
        // SAFETY: a global allocation with `Box`'s layout, initialized above.
        Ok(unsafe { Box::from_raw(handle) })
    }

    // Limit idle retention to 64 KiB and 64 descriptors per queue. Recycling is
    // best-effort and cannot make detach fail or invoke a native disposer.
    fn recycle(&mut self, mut bytes: Vec<u8>) {
        if self.recycled.len() == self.recycled.capacity()
            || bytes.capacity() > 65536 - self.recycled_bytes
            || self.recycled.len() == 64
        {
            return;
        }
        self.recycled_bytes += bytes.capacity();
        bytes.clear();
        self.recycled.push(bytes);
    }
    fn block(&mut self, size: usize) -> Result<Vec<u8>, i32> {
        if let Some(index) = self
            .recycled
            .iter()
            .rposition(|bytes| bytes.capacity() >= size)
        {
            let bytes = self.recycled.swap_remove(index);
            self.recycled_bytes -= bytes.capacity();
            return Ok(bytes);
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(size).map_err(|_| NOMEM)?;
        Ok(bytes)
    }

    fn front_view(&self) -> Option<BufferView> {
        self.entries.front().map(Entry::view)
    }

    // Cache invalidation and accounting precede DATA deallocation. Native
    // identities are returned untouched; no reference spans a Native effect.
    fn detach(&mut self) -> Option<Detached> {
        let entry = self.entries.pop_front()?;
        let Mode::Legacy(view) = &mut self.mode else {
            unreachable!("typed entries detach through the typed operation")
        };
        let reset_view = u32::from(!view.is_null());
        *view = ptr::null_mut();
        let token = match entry {
            Entry::Data(bytes) => {
                self.length -= chunk_length(&bytes);
                self.recycle(bytes);
                ptr::null_mut()
            }
            Entry::Native(token) => token,
            Entry::TypedToken(_) => {
                unreachable!("typed entries detach through the typed operation")
            }
        };
        Some(Detached { token, reset_view })
    }

    // Expose the contiguous export's single descriptor to the optimizer.
    #[inline(always)]
    unsafe fn appendv<T: Descriptor>(&mut self, iov: &[T], offset: u64) -> i32 {
        let mut total = 0usize;
        for part in iov {
            if !slice_fits::<u8>(part.length()) {
                return OVERFLOW;
            }
            if part.length() != 0 && part.data().is_null() {
                return INVALID;
            }
            let Some(sum) = total.checked_add(part.length()) else {
                return OVERFLOW;
            };
            total = sum;
        }
        if offset > total as u64 {
            return INVALID;
        }
        let Ok(offset) = usize::try_from(offset) else {
            return INVALID;
        };
        let Some(remaining) = total.checked_sub(offset) else {
            return INVALID;
        };
        if remaining == 0 {
            return OK;
        }
        let Ok(copied) = u32::try_from(remaining) else {
            return OVERFLOW;
        };
        let Some(new_length) = self.length.checked_add(copied) else {
            return OVERFLOW;
        };
        let chunk_size = self.chunk_size as usize;
        let chunks = remaining / chunk_size + usize::from(remaining % chunk_size != 0);
        let Some(new_count) = self.entries.len().checked_add(chunks) else {
            return OVERFLOW;
        };
        if !slice_fits::<Entry>(new_count) || !slice_fits::<u8>(remaining.min(chunk_size)) {
            return OVERFLOW;
        }
        let repeated = self.entries.capacity() != 0;
        let old_count = self.entries.len();
        if self.entries.try_reserve(chunks).is_err() {
            return NOMEM;
        }
        let mut index = 0;
        let mut skip = offset;
        while index < iov.len() && skip >= iov[index].length() {
            skip -= iov[index].length();
            index += 1;
        }
        let mut left = remaining;
        while left != 0 {
            let size = left.min(chunk_size);
            let mut bytes = match self.block(size) {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.entries.truncate(old_count);
                    return error;
                }
            };
            let mut filled = 0;
            while filled < size {
                let part = &iov[index];
                let count = (part.length() - skip).min(size - filled);
                if count != 0 {
                    // SAFETY: `skip + count <= part.length()`, inside the descriptor's non-null,
                    // readable data (validated above); `filled + count <= size`, inside the
                    // reserved, disjoint `bytes` capacity.
                    unsafe {
                        ptr::copy_nonoverlapping(
                            part.data().wrapping_add(skip),
                            bytes.as_mut_ptr().wrapping_add(filled),
                            count,
                        );
                    }
                }
                filled += count;
                skip += count;
                if skip == part.length() {
                    index += 1;
                    skip = 0;
                }
            }
            // SAFETY: the copy loop initialized exactly `size <= capacity` bytes.
            unsafe { bytes.set_len(size) };
            self.entries.push_back(Entry::Data(bytes));
            left -= size;
        }
        self.length = new_length;
        // Optional cache admission follows successful payload publication. It
        // cannot change append success or make detach allocate.
        if repeated && self.recycled.capacity() == 0 {
            self.recycled.try_reserve(chunks.min(64)).unwrap_or(());
        }
        OK
    }
}

// Admission/output initialization stays ahead of the first allocator effect.
unsafe fn create_buffer(chunk_size: u32, mode: Mode, output: *mut *mut Buffer) -> i32 {
    if output.is_null() {
        return INVALID;
    }
    // SAFETY: non-null (checked above), aligned, writable pointer storage.
    unsafe { output.write(ptr::null_mut()) };
    match Buffer::try_new(chunk_size, mode) {
        Ok(owner) => {
            // SAFETY: as above.
            unsafe { output.write(Box::into_raw(owner)) };
            OK
        }
        Err(status) => status,
    }
}

/// Create an independently owned queue, writing null on failure.
/// # Safety
/// `output` must be writable and aligned; see the frozen buffer ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_buffer_create(chunk_size: u32, output: *mut *mut Buffer) -> i32 {
    // SAFETY: forwards the caller's writable, aligned output.
    unsafe { create_buffer(chunk_size, Mode::Legacy(ptr::null_mut()), output) }
}

/// Drop a queue only after all native tokens have been detached.
/// # Safety
/// A nonnull handle must be live, uniquely owned, and issued by create.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_buffer_release(buffer: *mut Buffer) -> i32 {
    if buffer.is_null() {
        return OK;
    }
    // SAFETY: non-null (checked above); a live, uniquely owned handle from create.
    let queue = unsafe { &*buffer };
    if queue
        .entries
        .iter()
        .any(|e| matches!(e, Entry::Native(_) | Entry::TypedToken(_)))
    {
        return PENDING_NATIVE;
    }
    // SAFETY: as above; the shared borrow ended and the handle is released once.
    unsafe { drop(Box::from_raw(buffer)) };
    OK
}

/// Copy bytes atomically into new queue entries.
/// # Safety
/// Handle and borrowed input satisfy the frozen ABI's validity/disjointness rules.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_buffer_append(
    buffer: *mut Buffer,
    data: *const u8,
    length: usize,
) -> i32 {
    if buffer.is_null() {
        return INVALID;
    }
    // SAFETY: `buffer` is non-null (checked above), live and exclusive.
    let queue = unsafe { &mut *buffer };
    // SAFETY: the borrowed input follows the frozen ABI's validity rules.
    unsafe { queue.appendv(&[BufferIovec { data, length }], 0) }
}

/// Copy the concatenated vector suffix atomically into new queue entries.
/// # Safety
/// Handle, descriptors and borrowed data satisfy the frozen ABI contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_buffer_appendv(
    buffer: *mut Buffer,
    iov: *const BufferIovec,
    count: usize,
    offset: u64,
) -> i32 {
    if buffer.is_null() {
        return INVALID;
    }
    if !slice_fits::<BufferIovec>(count) {
        return OVERFLOW;
    }
    if count != 0 && iov.is_null() {
        return INVALID;
    }
    let parts = if count == 0 {
        &[]
    } else {
        // SAFETY: non-null, `count` fits isize (checked above), readable descriptors.
        unsafe { slice::from_raw_parts(iov, count) }
    };
    // SAFETY: `buffer` is non-null (checked above), live and exclusive.
    let queue = unsafe { &mut *buffer };
    // SAFETY: the borrowed input follows the frozen ABI's validity rules.
    unsafe { queue.appendv(parts, offset) }
}

/// Diagnostics for the actual external C input layout; no conversion credit.
#[cfg(unix)]
#[repr(C)]
pub struct PosixIovecLayout {
    pub size: usize,
    pub alignment: usize,
    pub base_offset: usize,
    pub length_offset: usize,
}
#[cfg(unix)]
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_buffer_iovec_layout() -> PosixIovecLayout {
    PosixIovecLayout {
        size: mem::size_of::<libc::iovec>(),
        alignment: mem::align_of::<libc::iovec>(),
        base_offset: mem::offset_of!(libc::iovec, iov_base),
        length_offset: mem::offset_of!(libc::iovec, iov_len),
    }
}

/// Copy actual target POSIX iovecs without allocating descriptor staging storage.
/// # Safety
/// Handle, target `libc::iovec` array and borrowed initialized data obey the frozen
/// validity/disjointness/stability rules. This is not the frozen POD type. No
/// descriptor reference survives the call; callbacks and native effects remain outside.
#[cfg(unix)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_buffer_append_iovec(
    buffer: *mut Buffer,
    iov: *const libc::iovec,
    count: usize,
    offset: i64,
) -> i32 {
    if buffer.is_null() || offset < 0 || (count != 0 && iov.is_null()) {
        return INVALID;
    }
    if !slice_fits::<libc::iovec>(count) {
        return OVERFLOW;
    }
    let parts = if count == 0 {
        &[]
    } else {
        // SAFETY: non-null, `count` fits isize (checked above), readable descriptors.
        unsafe { slice::from_raw_parts(iov, count) }
    };
    // SAFETY: `buffer` is non-null (checked above), live and exclusive.
    let queue = unsafe { &mut *buffer };
    // SAFETY: the borrowed input follows the frozen ABI's validity rules.
    unsafe { queue.appendv(parts, offset.unsigned_abs()) }
}

/// Transfer one opaque native token to the queue on success.
/// # Safety
/// Handle is live and exclusive; token is unique and owned by the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_buffer_push_native(buffer: *mut Buffer, token: *mut c_void) -> i32 {
    if buffer.is_null() || token.is_null() {
        return INVALID;
    }
    // SAFETY: `buffer` is non-null (checked above), live and exclusive.
    let queue = unsafe { &mut *buffer };
    if !matches!(queue.mode, Mode::Legacy(_)) {
        return INVALID;
    }
    let Some(count) = queue.entries.len().checked_add(1) else {
        return OVERFLOW;
    };
    if !slice_fits::<Entry>(count) {
        return OVERFLOW;
    }
    if queue.entries.try_reserve(1).is_err() {
        return NOMEM;
    }
    queue.entries.push_back(Entry::Native(token));
    OK
}

/// Borrow the current front; appends never invalidate its data pointer.
/// # Safety
/// Handle is live and output is writable, aligned and disjoint from the queue.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_buffer_front(
    buffer: *const Buffer,
    output: *mut BufferView,
) -> i32 {
    if output.is_null() {
        return INVALID;
    }
    // SAFETY: `output` is non-null (checked above), aligned, writable and disjoint.
    unsafe { output.write(BufferView::ZERO) };
    if buffer.is_null() {
        return INVALID;
    }
    // SAFETY: `buffer` is non-null (checked above) and live.
    let queue = unsafe { &*buffer };
    if !matches!(queue.mode, Mode::Legacy(_)) {
        return INVALID;
    }
    let Some(view) = queue.front_view() else {
        return EMPTY;
    };
    // SAFETY: `output` is non-null (checked above), aligned, writable and disjoint.
    unsafe { output.write(view) };
    OK
}

/// Detach one entry, returning any native token to its owner.
/// # Safety
/// Handle is live and exclusive; output is writable, aligned and disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_buffer_pop(
    buffer: *mut Buffer,
    native_token: *mut *mut c_void,
) -> i32 {
    if native_token.is_null() {
        return INVALID;
    }
    // SAFETY: `native_token` is non-null (checked above), aligned, writable and disjoint.
    unsafe { native_token.write(ptr::null_mut()) };
    if buffer.is_null() {
        return INVALID;
    }
    // SAFETY: `buffer` is non-null (checked above), live and exclusive.
    let queue = unsafe { &mut *buffer };
    if !matches!(queue.mode, Mode::Legacy(_)) {
        return INVALID;
    }
    queue.mode = Mode::Legacy(ptr::null_mut());
    match queue.detach() {
        None => EMPTY,
        Some(detached) => {
            if !detached.token.is_null() {
                // SAFETY: `native_token` is non-null (checked above), aligned and writable.
                unsafe { native_token.write(detached.token) };
            }
            OK
        }
    }
}

/// Select the facade front without interpreting the stable native view identity.
/// # Safety
/// The live exclusive handle, nonnull stable facade identity and disjoint output
/// obey the additive facade contract. The native object outlives the handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_buffer_front_plan(
    buffer: *mut Buffer,
    facade_view: *mut c_void,
    output: *mut BufferView,
) -> i32 {
    if output.is_null() {
        return INVALID;
    }
    // SAFETY: `output` is non-null (checked above), aligned, writable and disjoint.
    unsafe { output.write(BufferView::ZERO) };
    if buffer.is_null() || facade_view.is_null() {
        return INVALID;
    }
    // SAFETY: `buffer` is non-null (checked above), live and exclusive.
    let queue = unsafe { &mut *buffer };
    let Mode::Legacy(cached) = queue.mode else {
        return INVALID;
    };
    if !cached.is_null() {
        if cached != facade_view {
            return INVALID;
        }
        // SAFETY: `output` is non-null (checked above), aligned, writable and disjoint.
        unsafe {
            output.write(BufferView {
                kind: 3,
                token: facade_view,
                ..BufferView::ZERO
            });
        };
        return OK;
    }
    let Some(mut view) = queue.front_view() else {
        return EMPTY;
    };
    if view.kind == 0 {
        queue.mode = Mode::Legacy(facade_view);
        view.kind = 2;
        view.token = facade_view;
    }
    // SAFETY: `output` is non-null (checked above), aligned, writable and disjoint.
    unsafe { output.write(view) };
    OK
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct BufferAdvance {
    pub action: u32,
    pub reset_view: u32,
    pub token: *mut c_void,
}
impl BufferAdvance {
    const ZERO: Self = Self {
        action: 0,
        reset_view: 0,
        token: ptr::null_mut(),
    };
}

/// Select removal/drain effects, returning before every native callback/delete.
/// # Safety
/// The live exclusive handle and disjoint output obey the facade contract.
/// `cached_destroy` is a fresh boolean observation of the native cached view;
/// callbacks must complete before another plan and cannot reenter the handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_buffer_advance(
    buffer: *mut Buffer,
    drain: u32,
    cached_destroy: u32,
    output: *mut BufferAdvance,
) -> i32 {
    if output.is_null() {
        return INVALID;
    }
    // SAFETY: `output` is non-null (checked above), aligned, writable and disjoint.
    unsafe { output.write(BufferAdvance::ZERO) };
    if buffer.is_null() || drain > 1 || cached_destroy > 1 {
        return INVALID;
    }
    // SAFETY: `buffer` is non-null (checked above), live and exclusive.
    let queue = unsafe { &mut *buffer };
    let Mode::Legacy(cached) = queue.mode else {
        return INVALID;
    };
    if !cached.is_null() && cached_destroy != 0 {
        // SAFETY: `output` is non-null (checked above), aligned, writable and disjoint.
        unsafe {
            output.write(BufferAdvance {
                action: 1,
                token: cached,
                ..BufferAdvance::ZERO
            });
        };
        return OK;
    }
    let mut result = BufferAdvance::ZERO;
    while let Some(detached) = queue.detach() {
        result.reset_view |= detached.reset_view;
        if !detached.token.is_null() {
            result.action = 2;
            result.token = detached.token;
            break;
        }
        if drain == 0 {
            result.action = 2;
            break;
        }
    }
    // SAFETY: `output` is non-null (checked above), aligned, writable and disjoint.
    unsafe { output.write(result) };
    OK
}

/// Read byte accounting and queue entry count.
/// # Safety
/// Handle is live; both outputs are valid, aligned and mutually disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_buffer_stats(
    buffer: *const Buffer,
    length: *mut u32,
    count: *mut usize,
) -> i32 {
    if buffer.is_null() || length.is_null() || count.is_null() {
        return INVALID;
    }
    // SAFETY: `buffer` is non-null (checked above) and live.
    let queue = unsafe { &*buffer };
    // SAFETY: non-null (checked above), aligned, writable and disjoint outputs.
    unsafe { length.write(queue.length) };
    // SAFETY: as above.
    unsafe { count.write(queue.entries.len()) };
    OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{GlobalAlloc, System};
    use std::cell::Cell;

    // Failure injection affects this test thread only, including actual Vec and
    // VecDeque allocation paths. Production has no allocator wrapper or state.
    thread_local! { pub(super) static FAIL_AFTER: Cell<Option<usize>> = const { Cell::new(None) }; }
    struct FailAllocator;
    fn fail_allocation() -> bool {
        FAIL_AFTER
            .try_with(|counter| match counter.get() {
                None => false,
                Some(0) => true,
                Some(n) => {
                    counter.set(Some(n - 1));
                    false
                }
            })
            .unwrap_or(false)
    }
    unsafe impl GlobalAlloc for FailAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if fail_allocation() {
                ptr::null_mut()
            } else {
                unsafe { System.alloc(layout) }
            }
        }
        unsafe fn realloc(&self, data: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            if fail_allocation() {
                ptr::null_mut()
            } else {
                unsafe { System.realloc(data, layout, size) }
            }
        }
        unsafe fn dealloc(&self, data: *mut u8, layout: Layout) {
            unsafe {
                System.dealloc(data, layout);
            }
        }
    }
    #[global_allocator]
    static ALLOCATOR: FailAllocator = FailAllocator;

    #[test]
    #[expect(
        clippy::cognitive_complexity,
        reason = "one end-to-end facade scenario"
    )]
    fn facade_plans_preserve_identity_live_callback_bytes_and_raw_api_compatibility() {
        let b = Owned::new(2);
        let mut identity = 0u8;
        let identity = (&raw mut identity).cast();
        let mut view = BufferView::ZERO;
        let mut effect = BufferAdvance::ZERO;
        let mut native = 7u8;
        let native = (&raw mut native).cast();
        unsafe {
            assert_eq!(swr_v1_buffer_append(b.0, b"abcde".as_ptr(), 5), OK);
            assert_eq!(swr_v1_buffer_front_plan(b.0, identity, &raw mut view), OK);
            assert_eq!((view.kind, view.length, view.token), (2, 2, identity));
            let data = view.data;
            assert_eq!(swr_v1_buffer_front_plan(b.0, identity, &raw mut view), OK);
            assert_eq!((view.kind, view.token), (3, identity));
            assert_eq!(swr_v1_buffer_append(b.0, b"fg".as_ptr(), 2), OK);
            assert_eq!(swr_v1_buffer_advance(b.0, 2, 1, &raw mut effect), INVALID);
            assert_eq!(effect.action, 0);
            assert_eq!(swr_v1_buffer_advance(b.0, 0, 1, &raw mut effect), OK);
            assert_eq!(
                (effect.action, effect.reset_view, effect.token),
                (1, 0, identity)
            );
            assert_eq!(slice::from_raw_parts(data, 2), b"ab");
            assert_eq!((&*b.0).length, 7); // callback observes data before any detach
            assert_eq!(swr_v1_buffer_advance(b.0, 0, 0, &raw mut effect), OK);
            assert_eq!(
                (effect.action, effect.reset_view, effect.token),
                (2, 1, ptr::null_mut())
            );
            assert_eq!((&*b.0).length, 5);
            assert_eq!(swr_v1_buffer_front_plan(b.0, identity, &raw mut view), OK);
            assert_eq!(view.kind, 2);
            let mut detached = ptr::null_mut();
            assert_eq!(swr_v1_buffer_pop(b.0, &raw mut detached), OK);
            assert!(matches!((&*b.0).mode, Mode::Legacy(view) if view.is_null()));
            assert_eq!(swr_v1_buffer_front_plan(b.0, identity, &raw mut view), OK);
            assert_eq!((view.kind, view.length), (2, 1));
            assert_eq!(swr_v1_buffer_push_native(b.0, native), OK);
            assert_eq!(swr_v1_buffer_release(b.0), PENDING_NATIVE);
            assert!(matches!((&*b.0).mode, Mode::Legacy(view) if view == identity)); // failed release is unchanged
            assert_eq!(swr_v1_buffer_advance(b.0, 1, 0, &raw mut effect), OK);
            assert_eq!(
                (effect.action, effect.reset_view, effect.token),
                (2, 1, native)
            );
            assert_eq!((&*b.0).length, 0);
            assert_eq!(swr_v1_buffer_advance(b.0, 1, 0, &raw mut effect), OK);
            assert_eq!((effect.action, effect.reset_view), (0, 0));
            assert_eq!(
                swr_v1_buffer_front_plan(b.0, identity, &raw mut view),
                EMPTY
            );
            assert!(view.token.is_null());
        }
    }

    pub(super) struct Owned(pub(super) *mut Buffer);
    impl Owned {
        pub(super) fn new(size: u32) -> Self {
            let mut buffer = ptr::null_mut();
            assert_eq!(unsafe { swr_v1_buffer_create(size, &raw mut buffer) }, OK);
            Self(buffer)
        }
        pub(super) fn append(&self, bytes: &[u8]) -> i32 {
            unsafe { swr_v1_buffer_append(self.0, bytes.as_ptr(), bytes.len()) }
        }
        pub(super) fn stats(&self) -> (u32, usize) {
            let (mut length, mut count) = (0, 0);
            assert_eq!(
                unsafe { swr_v1_buffer_stats(self.0, &raw mut length, &raw mut count) },
                OK
            );
            (length, count)
        }
        pub(super) fn view(&self) -> BufferView {
            let mut view = BufferView::ZERO;
            assert_eq!(unsafe { swr_v1_buffer_front(self.0, &raw mut view) }, OK);
            view
        }
        pub(super) fn pop(&self) -> *mut c_void {
            let mut token = ptr::null_mut();
            assert_eq!(unsafe { swr_v1_buffer_pop(self.0, &raw mut token) }, OK);
            token
        }
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            assert_eq!(unsafe { swr_v1_buffer_release(self.0) }, OK);
        }
    }

    #[cfg(unix)]
    #[test]
    fn actual_posix_descriptors_share_validation_copy_and_atomic_rollback() {
        let a: [u8; 3] = [0, 255, 7];
        let b: [u8; 4] = [9, 0, 254, 8];
        let vectors = [
            libc::iovec {
                iov_base: a.as_ptr().cast_mut().cast(),
                iov_len: a.len(),
            },
            libc::iovec {
                iov_base: ptr::null_mut(),
                iov_len: 0,
            },
            libc::iovec {
                iov_base: b.as_ptr().cast_mut().cast(),
                iov_len: b.len(),
            },
        ];
        let expected = [0, 255, 7, 9, 0, 254, 8];
        for chunk in [1, 4, 32] {
            for offset in 0..=expected.len() + 1 {
                let owned = Owned::new(chunk);
                let status = unsafe {
                    swr_v1_buffer_append_iovec(
                        owned.0,
                        vectors.as_ptr(),
                        vectors.len(),
                        i64::try_from(offset).unwrap(),
                    )
                };
                if offset > expected.len() {
                    assert_eq!(status, INVALID);
                    assert_eq!(owned.stats(), (0, 0));
                    continue;
                }
                assert_eq!(status, OK);
                let mut copied = Vec::new();
                while owned.stats().1 != 0 {
                    let view = owned.view();
                    copied.extend_from_slice(unsafe {
                        slice::from_raw_parts(view.data, view.length as usize)
                    });
                    owned.pop();
                }
                assert_eq!(copied, expected[offset..]);
            }
        }
        let mut succeeded = false;
        for budget in 0..8 {
            let owned = Owned::new(2);
            assert_eq!(owned.append(b"old"), OK);
            let before = owned.stats();
            let front = owned.view();
            FAIL_AFTER.with(|counter| counter.set(Some(budget)));
            let status =
                unsafe { swr_v1_buffer_append_iovec(owned.0, vectors.as_ptr(), vectors.len(), 0) };
            FAIL_AFTER.with(|counter| counter.set(None));
            if status == OK {
                succeeded = true;
                break;
            }
            assert_eq!(status, NOMEM);
            assert_eq!(owned.stats(), before);
            assert_eq!(owned.view().data, front.data);
            assert_eq!(
                unsafe { slice::from_raw_parts(front.data, front.length as usize) },
                b"ol"
            );
        }
        assert!(succeeded);
        let owned = Owned::new(4);
        assert_eq!(
            unsafe { swr_v1_buffer_append_iovec(owned.0, vectors.as_ptr(), vectors.len(), -1) },
            INVALID
        );
        assert_eq!(
            unsafe { swr_v1_buffer_append_iovec(owned.0, ptr::null(), 1, 0) },
            INVALID
        );
        assert_eq!(
            unsafe { swr_v1_buffer_append_iovec(owned.0, ptr::null(), 0, 0) },
            OK
        );
        assert_eq!(
            unsafe { swr_v1_buffer_append_iovec(owned.0, vectors.as_ptr(), usize::MAX, 0) },
            OVERFLOW
        );
        assert_eq!(owned.stats(), (0, 0));
    }

    #[test]
    fn vector_offsets_empty_descriptors_and_binary_chunks() {
        let a = [0, 1, 2, 3, 4];
        let b = [255, 0, 253, 252, 251, 250];
        let iov = [
            BufferIovec {
                data: ptr::null(),
                length: 0,
            },
            BufferIovec {
                data: a.as_ptr(),
                length: a.len(),
            },
            BufferIovec {
                data: ptr::null(),
                length: 0,
            },
            BufferIovec {
                data: b.as_ptr(),
                length: b.len(),
            },
            BufferIovec {
                data: ptr::null(),
                length: 0,
            },
        ];
        let combined: Vec<u8> = a.into_iter().chain(b).collect();
        for offset in 0..=combined.len() {
            let buffer = Owned::new(4);
            let contiguous = Owned::new(4);
            assert_eq!(contiguous.append(&combined[offset..]), OK);
            assert_eq!(
                unsafe { swr_v1_buffer_appendv(buffer.0, iov.as_ptr(), iov.len(), offset as u64) },
                OK
            );
            assert_eq!(buffer.stats(), contiguous.stats());
            let mut actual = Vec::new();
            while buffer.stats().1 != 0 {
                let view = buffer.view();
                assert_eq!(view.length, view.capacity);
                assert!(view.length > 0 && view.length <= 4);
                actual.extend_from_slice(unsafe {
                    slice::from_raw_parts(view.data, view.length as usize)
                });
                let direct = contiguous.view();
                assert_eq!(direct.length, view.length);
                assert_eq!(
                    unsafe { slice::from_raw_parts(direct.data, direct.length as usize) },
                    unsafe { slice::from_raw_parts(view.data, view.length as usize) }
                );
                assert!(contiguous.pop().is_null());
                assert!(buffer.pop().is_null());
            }
            assert_eq!(actual, combined[offset..]);
            assert_eq!(buffer.stats(), (0, 0));
            assert_eq!(contiguous.stats(), (0, 0));
        }
    }

    #[test]
    fn append_keeps_views_writable_and_stable_across_queue_growth() {
        let buffer = Owned::new(3);
        assert_eq!(buffer.append(b"abc"), OK);
        let front = buffer.view();
        unsafe {
            front.data.write(b'Z');
        }
        for _ in 0..100 {
            assert_eq!(buffer.append(b"defghi"), OK);
        }
        assert_eq!(buffer.view().data, front.data);
        assert_eq!(unsafe { slice::from_raw_parts(front.data, 3) }, b"Zbc");
        assert_eq!(buffer.stats(), (603, 201));
    }

    #[test]
    fn native_tokens_preserve_order_and_block_release() {
        let buffer = Owned::new(4);
        let mut value = 42u32;
        let token = ptr::from_mut(&mut value).cast();
        assert_eq!(buffer.append(b"abcdef"), OK);
        assert_eq!(unsafe { swr_v1_buffer_push_native(buffer.0, token) }, OK);
        assert_eq!(buffer.append(b"g"), OK);
        assert_eq!(unsafe { swr_v1_buffer_release(buffer.0) }, PENDING_NATIVE);
        assert_eq!(buffer.stats(), (7, 4));
        assert!(buffer.pop().is_null());
        assert!(buffer.pop().is_null());
        assert_eq!(buffer.stats(), (1, 2));
        let view = buffer.view();
        assert_eq!(view.kind, 1);
        assert_eq!(view.token, token);
        assert!(view.data.is_null());
        assert_eq!(buffer.pop(), token);
        assert_eq!(buffer.stats(), (1, 1));
    }

    #[test]
    fn bad_inputs_do_not_change_state_and_outputs_follow_contract() {
        let buffer = Owned::new(4);
        assert_eq!(buffer.append(b"abc"), OK);
        let invalid = BufferIovec {
            data: ptr::null(),
            length: 1,
        };
        assert_eq!(
            unsafe { swr_v1_buffer_appendv(buffer.0, &raw const invalid, 1, 1) },
            INVALID
        );
        assert_eq!(
            unsafe { swr_v1_buffer_appendv(buffer.0, ptr::null(), 0, 1) },
            INVALID
        );
        assert_eq!(
            unsafe { swr_v1_buffer_appendv(buffer.0, ptr::null(), 0, 0) },
            OK
        );
        assert_eq!(
            unsafe { swr_v1_buffer_append(buffer.0, ptr::null(), 0) },
            OK
        );
        assert_eq!(
            unsafe { swr_v1_buffer_append(buffer.0, ptr::dangling(), usize::MAX) },
            OVERFLOW
        );
        assert_eq!(
            unsafe { swr_v1_buffer_appendv(buffer.0, ptr::dangling(), usize::MAX, 0) },
            OVERFLOW
        );
        assert_eq!(
            unsafe { swr_v1_buffer_push_native(buffer.0, ptr::null_mut()) },
            INVALID
        );
        assert_eq!(
            unsafe { swr_v1_buffer_pop(buffer.0, ptr::null_mut()) },
            INVALID
        );
        assert_eq!(buffer.stats(), (3, 1));
        let mut view = buffer.view();
        assert_eq!(
            unsafe { swr_v1_buffer_front(ptr::null(), &raw mut view) },
            INVALID
        );
        assert_eq!(view.length, 0);
        assert!(view.data.is_null());
        let (mut length, mut count) = (123, 456);
        assert_eq!(
            unsafe { swr_v1_buffer_stats(ptr::null(), &raw mut length, &raw mut count) },
            INVALID
        );
        assert_eq!((length, count), (123, 456));
        buffer.pop();
        assert_eq!(
            unsafe { swr_v1_buffer_front(buffer.0, &raw mut view) },
            EMPTY
        );
        let mut token = ptr::dangling_mut();
        assert_eq!(
            unsafe { swr_v1_buffer_pop(buffer.0, &raw mut token) },
            EMPTY
        );
        assert!(token.is_null());
        assert_eq!(unsafe { swr_v1_buffer_release(ptr::null_mut()) }, OK);
    }

    #[test]
    fn total_overflow_rejected_before_reading_input() {
        let buffer = Owned::new(4);
        // Inject accounting internally to exercise UINT32_MAX without 4 GiB.
        unsafe {
            (*buffer.0).length = u32::MAX;
        }
        assert_eq!(
            unsafe { swr_v1_buffer_append(buffer.0, ptr::dangling(), 1) },
            OVERFLOW
        );
        assert_eq!(buffer.stats(), (u32::MAX, 0));
        unsafe {
            (*buffer.0).length = 0;
        }
    }

    #[test]
    fn single_chunk_failure_keeps_front_and_accounting_unchanged() {
        for populated in [false, true] {
            for fail_after in 0..=2 {
                let buffer = Owned::new(4096);
                if populated {
                    assert_eq!(buffer.append(b"old"), OK);
                    // Force queue growth as well as a data allocation.
                    while unsafe { (*buffer.0).entries.len() < (*buffer.0).entries.capacity() } {
                        assert_eq!(buffer.append(b"old"), OK);
                    }
                }
                let before = buffer.stats();
                let front = if populated { Some(buffer.view()) } else { None };
                FAIL_AFTER.set(Some(fail_after));
                let status = buffer.append(b"new");
                FAIL_AFTER.set(None);
                if fail_after < 2 {
                    assert_eq!(status, NOMEM);
                    assert_eq!(buffer.stats(), before);
                } else {
                    assert_eq!(status, OK);
                    assert_eq!(buffer.stats(), (before.0 + 3, before.1 + 1));
                }
                if let Some(front) = front {
                    assert_eq!(buffer.view().data, front.data);
                    assert_eq!(unsafe { slice::from_raw_parts(front.data, 3) }, b"old");
                }
            }
        }
        // A drained queue retains metadata capacity. Only its new bytes should
        // need allocation, so failure of any second allocation must be harmless.
        let reused = Owned::new(4096);
        assert_eq!(reused.append(b"old"), OK);
        reused.pop();
        FAIL_AFTER.set(Some(1));
        let status = reused.append(b"new");
        FAIL_AFTER.set(None);
        assert_eq!(status, OK);
        assert_eq!(reused.stats(), (3, 1));
        assert_eq!(
            unsafe { slice::from_raw_parts(reused.view().data, 3) },
            b"new"
        );
    }

    #[test]
    #[expect(
        clippy::cognitive_complexity,
        reason = "one scenario per allocation failure point"
    )]
    fn every_allocation_failure_is_atomic() {
        let mut handle = ptr::dangling_mut();
        FAIL_AFTER.set(Some(0));
        let status = unsafe { swr_v1_buffer_create(4, &raw mut handle) };
        FAIL_AFTER.set(None);
        assert_eq!(status, NOMEM);
        assert!(handle.is_null());
        let mut failures = 0;
        for nth in 0..32 {
            let buffer = Owned::new(4);
            assert_eq!(buffer.append(b"abc"), OK);
            let view = buffer.view();
            FAIL_AFTER.set(Some(nth));
            let status = buffer.append(b"0123456789abcdefghijklmnop");
            FAIL_AFTER.set(None);
            if status == OK {
                break;
            }
            assert_eq!(status, NOMEM);
            assert_eq!(buffer.stats(), (3, 1));
            assert_eq!(buffer.view().data, view.data);
            assert_eq!(unsafe { slice::from_raw_parts(view.data, 3) }, b"abc");
            failures += 1;
        }
        assert!(
            failures >= 8,
            "must visit metadata, every chunk, and queue-growth allocations"
        );
        // Exercise rollback after metadata growth from a wrapped queue with a
        // native token between existing data. No provisional entry may escape.
        for nth in 0..=8 {
            let buffer = Owned::new(4);
            assert_eq!(buffer.append(b"abcdefghijkl"), OK);
            while unsafe { (*buffer.0).entries.len() < (*buffer.0).entries.capacity() } {
                assert_eq!(buffer.append(b"fill"), OK);
            }
            buffer.pop();
            buffer.pop();
            let mut native = 7u32;
            let token = ptr::from_mut(&mut native).cast();
            assert_eq!(unsafe { swr_v1_buffer_push_native(buffer.0, token) }, OK);
            assert_eq!(buffer.append(b"old"), OK);
            assert!(!unsafe { &*buffer.0 }.entries.as_slices().1.is_empty());
            // Keep the cold-allocation sweep visiting every payload allocation;
            // warm-cache behavior is exercised separately.
            unsafe {
                (*buffer.0).recycled.clear();
                (*buffer.0).recycled_bytes = 0;
            }
            let before = buffer.stats();
            let front = buffer.view();
            FAIL_AFTER.set(Some(nth));
            let status = buffer.append(b"0123456789abcdefghijklmnop");
            FAIL_AFTER.set(None);
            assert_eq!(status, if nth < 8 { NOMEM } else { OK });
            assert_eq!(
                buffer.stats(),
                if status == NOMEM {
                    before
                } else {
                    (before.0 + 26, before.1 + 7)
                }
            );
            assert_eq!(buffer.view().data, front.data);
            assert_eq!(unsafe { slice::from_raw_parts(front.data, 4) }, b"ijkl");
            assert!(matches!(unsafe { &*buffer.0 }.entries.get(before.1 - 2),
                            Some(Entry::Native(value)) if *value == token));
            let mut returned = 0;
            while buffer.stats().1 != 0 {
                let popped = buffer.pop();
                if !popped.is_null() {
                    assert_eq!(popped, token);
                    returned += 1;
                }
            }
            assert_eq!(returned, 1);
        }
        let empty = Owned::new(4);
        let mut native = 7;
        FAIL_AFTER.set(Some(0));
        let status =
            unsafe { swr_v1_buffer_push_native(empty.0, ptr::from_mut(&mut native).cast()) };
        FAIL_AFTER.set(None);
        assert_eq!(status, NOMEM);
        assert_eq!(empty.stats(), (0, 0));
    }
}

pub mod string;

#[cfg(test)]
#[path = "../../../tests/modules/buffer/recycling.rs"]
mod recycling_tests;
