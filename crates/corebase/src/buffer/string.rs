// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/string.cc and include/swoole_string.h
// (Swoole contributors).
//! Rust string storage and operations. Native socket records retain opaque pointers;
//! the allocation and its fields are accessed only by Rust.
//!
//! Allocation failure aborts, except for [`OwnedString::try_alloc_exact`]: upstream
//! throws `std::bad_alloc`, which no first-party code catches. Arithmetic wraps,
//! as upstream's `size_t` does, and the casts are upstream's C casts.
use crate::lent::{Spare, lend_spare};
use crate::{CCast, TrustedAllocator};
use std::ffi::{c_int, c_void};
use std::mem::MaybeUninit;
use std::ptr;

/// The runtime string buffer.
///
/// Its fields are private, so safe code keeps what the safe methods trust: `str`
/// is null with `length` and `size` 0, or the live allocation of `size` bytes
/// that `allocator` (then set) made, whose first `length` are initialized.
/// `offset` holds anything (a cursor, or a caller's flags), so a
/// method that indexes by it bounds it first.
pub struct StringBuffer {
    length: usize,
    size: usize,
    offset: i64,
    str: *mut u8,
    /// A nullable `const Allocator *`, which lives for the process.
    allocator: Option<TrustedAllocator>,
}

/// A Rust-owned string allocation, disposed when it leaves scope.
pub struct OwnedString(StringBuffer);

impl OwnedString {
    /// Copy a payload into an exactly sized allocation from the receiver's allocator.
    #[must_use]
    pub fn copy_in(bytes: &[u8], allocator: TrustedAllocator) -> Self {
        if bytes.is_empty() {
            return Self::default();
        }
        let mut owned = Self::try_alloc_exact(bytes.len(), allocator)
            .unwrap_or_else(|| crate::fatal("out of memory"));
        owned.0.allocation().write_copy_of_slice(bytes);
        owned.0.length = bytes.len();
        owned
    }

    /// A fresh allocation of exactly `size` bytes, or `None` when `allocator` fails.
    #[must_use]
    pub fn try_alloc_exact(size: usize, allocator: TrustedAllocator) -> Option<Self> {
        let data = allocator.allocate(size);
        (!data.is_null()).then(|| {
            Self(StringBuffer {
                str: data.cast(),
                size,
                allocator: Some(allocator),
                ..StringBuffer::EMPTY
            })
        })
    }
}

impl Default for OwnedString {
    fn default() -> Self {
        Self(StringBuffer::EMPTY)
    }
}

impl core::ops::Deref for OwnedString {
    type Target = StringBuffer;
    fn deref(&self) -> &StringBuffer {
        &self.0
    }
}

impl core::ops::DerefMut for OwnedString {
    fn deref_mut(&mut self) -> &mut StringBuffer {
        &mut self.0
    }
}

impl Drop for OwnedString {
    fn drop(&mut self) {
        self.0.dispose();
    }
}

/// `(size_t) off_t`.
fn unsigned(value: i64) -> usize {
    value.c_cast::<usize>()
}

/// `(off_t) size_t`.
fn signed(value: usize) -> i64 {
    value.c_cast::<i64>()
}

/// `SW_MEM_ALIGNED_SIZE`.
const fn aligned(size: usize) -> usize {
    size.wrapping_add(7) & !7
}

/// `swoole_size_align(size, swoole_pagesize())`.
fn page_aligned(size: usize) -> usize {
    let page = crate::swr_v1_core_pagesize() as usize;
    size.wrapping_add(page.wrapping_sub(size % page))
}

/// `swoole_strnstr`: the first `needle` in `haystack`, both lengths cut to its `uint32_t`.
fn strnstr(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let cut = |bytes: &[u8]| bytes.len().c_cast::<u32>() as usize;
    let haystack = haystack.get(..cut(haystack)).unwrap_or_default();
    let needle = needle.get(..cut(needle)).unwrap_or_default();
    debug_assert!(!needle.is_empty(), "swoole_strnstr's needle is not empty");
    find_bytes(haystack, needle)
}

/// Byte offset of the first whole needle. An empty needle matches at zero.
#[must_use]
#[inline(always)]
pub fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    match needle {
        // A port's EOF, at most `SW_DATA_EOF_MAXLEN` bytes: a scan for its first byte
        // checks each candidate, which costs far less per search than the
        // substring searcher `memmem::find` builds for each call.
        [first, rest @ ..] if rest.len() < 8 => memchr::memchr_iter(*first, haystack).find(|&at| {
            haystack
                .get(at + 1..)
                .is_some_and(|after| after.starts_with(rest))
        }),
        _ => memchr::memmem::find(haystack, needle),
    }
}

/// Offset of the first delimiter. An empty delimiter never terminates a package.
#[must_use]
pub fn find_delimiter(bytes: &[u8], delimiter: &[u8]) -> Option<usize> {
    if delimiter.is_empty() {
        return None;
    }
    find_bytes(bytes, delimiter)
}

/// An allocator's result, aborting on failure as `std::bad_alloc` would.
fn checked(pointer: *mut c_void) -> *mut u8 {
    if pointer.is_null() {
        crate::fatal("out of memory");
    }
    pointer.cast()
}

impl StringBuffer {
    pub const EMPTY: Self = Self {
        length: 0,
        size: 0,
        offset: 0,
        str: ptr::null_mut(),
        allocator: None,
    };

    /// The content's length.
    #[must_use]
    #[inline]
    pub const fn len(&self) -> usize {
        self.length
    }

    #[must_use]
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// The allocation's size.
    #[must_use]
    #[inline]
    pub const fn size(&self) -> usize {
        self.size
    }

    /// The cursor (or flags) upstream keeps beside the content.
    #[must_use]
    #[inline]
    pub const fn offset(&self) -> i64 {
        self.offset
    }

    #[inline]
    pub const fn set_offset(&mut self, offset: i64) {
        self.offset = offset;
    }

    /// The buffer (null without one), for C to read `len()` bytes from.
    #[must_use]
    #[inline]
    pub const fn as_ptr(&self) -> *const u8 {
        self.str
    }

    /// The buffer (null without one), for C to write up to `size()` bytes to.
    #[must_use]
    #[inline]
    pub const fn as_mut_ptr(&mut self) -> *mut u8 {
        self.str
    }

    /// `length = 0; offset = 0`: forget the content, keep the buffer.
    #[inline]
    pub const fn clear(&mut self) {
        self.length = 0;
        self.offset = 0;
    }

    /// Appends what `fill` writes into the room past the content: what it returned.
    #[inline]
    pub fn fill<R>(&mut self, fill: impl for<'s> FnOnce(&mut Spare<'s>) -> R) -> R {
        self.fill_within(usize::MAX, fill)
    }

    /// [`fill`](Self::fill) of at most `limit` bytes.
    #[inline]
    pub fn fill_within<R>(
        &mut self,
        limit: usize,
        fill: impl for<'s> FnOnce(&mut Spare<'s>) -> R,
    ) -> R {
        let spare = self.spare();
        let room = limit.min(spare.len());
        let (result, filled) = lend_spare(spare.get_mut(..room).unwrap_or_default(), fill);
        self.length += filled;
        result
    }

    /// Counts no more than the first `length` bytes as the content.
    #[inline]
    pub const fn truncate(&mut self, length: usize) {
        if length < self.length {
            self.length = length;
        }
    }

    /// Replaces the content with what `fill` writes from the start of the allocation,
    /// when it returns `Some`; otherwise the content keeps its length (its bytes may be
    /// overwritten).
    pub fn refill<T>(
        &mut self,
        fill: impl for<'s> FnOnce(&mut Spare<'s>) -> Option<T>,
    ) -> Option<T> {
        let (result, filled) = lend_spare(self.allocation(), fill);
        if result.is_some() {
            self.length = filled;
        }
        result
    }

    /// `String::alloc`: a fresh buffer, from the standard allocator when `with` is `None`.
    pub fn alloc(&mut self, size: usize, with: Option<TrustedAllocator>) {
        self.alloc_exact(aligned(size), with);
    }

    /// A fresh allocation of exactly `size` bytes, without String's alignment.
    pub fn alloc_exact(&mut self, size: usize, with: Option<TrustedAllocator>) {
        let with = with.unwrap_or(TrustedAllocator::STD);
        self.length = 0;
        self.size = size;
        self.offset = 0;
        self.str = checked(with.allocate(size));
        self.allocator = Some(with);
    }

    /// The destructor's body, which leaves the String empty.
    pub fn dispose(&mut self) {
        let data = self.release();
        if let Some(free) = self.allocator.and_then(|with| with.get().free)
            && !data.is_null()
        {
            // SAFETY: `data` was this allocator's live allocation.
            unsafe { free(data.cast()) };
        }
    }

    /// `String::copy`.
    pub fn copy_from(&mut self, source: &Self) {
        self.alloc(source.size, source.allocator);
        // The new allocation holds `source.size >= source.length` bytes.
        if let Some(to) = self.spare().get_mut(..source.length) {
            to.write_copy_of_slice(source.view());
            self.length = source.length;
        }
        self.offset = source.offset;
    }

    /// `String::move`.
    pub fn move_from(&mut self, source: &mut Self) {
        self.str = source.str;
        self.length = source.length;
        self.offset = source.offset;
        self.size = source.size;
        self.allocator = source.allocator;
        source.str = ptr::null_mut();
        source.length = 0;
        source.size = 0;
        source.offset = 0;
    }

    /// The allocator that made the buffer, none before one did.
    #[must_use]
    pub const fn allocator(&self) -> Option<TrustedAllocator> {
        self.allocator
    }

    /// The `length` bytes at `str`.
    #[must_use]
    pub fn view(&self) -> &[u8] {
        if self.length == 0 {
            return &[];
        }
        // SAFETY: `str` holds `length` bytes.
        unsafe { std::slice::from_raw_parts(self.str, self.length) }
    }

    /// [`view`](Self::view), writable in place.
    #[must_use]
    pub fn view_mut(&mut self) -> &mut [u8] {
        let length = self.length;
        let content = self.allocation().get_mut(..length).unwrap_or_default();
        // SAFETY: the first `length` bytes are the content, which is initialized.
        unsafe { content.assume_init_mut() }
    }

    /// `String::reserve`, never below the content's length.
    pub fn reserve(&mut self, new_size: usize) {
        self.reserve_exact(aligned(new_size.max(self.length)));
    }

    /// Resize to exactly `new_size` bytes, never discarding initialized content.
    pub fn reserve_exact(&mut self, new_size: usize) {
        let Some(with) = self.allocator.filter(|_| !self.str.is_null()) else {
            self.alloc_exact(new_size, None);
            return;
        };
        let new_size = new_size.max(self.length);
        self.str = checked(with.get().realloc.map_or(ptr::null_mut(), |realloc| {
            // SAFETY: `str` is this allocator's live allocation.
            unsafe { realloc(self.str.cast(), new_size) }
        }));
        self.size = new_size;
    }

    pub fn extend(&mut self, new_size: usize) {
        debug_assert!(new_size > self.size, "String::extend grows");
        self.reserve(new_size);
    }

    /// `String::append(const char *, size_t)`.
    pub fn append(&mut self, data: &[u8]) {
        let new_size = self.length.wrapping_add(data.len());
        if new_size > self.size {
            self.reserve(new_size);
        }
        // The buffer now holds `new_size` bytes.
        if let Some(to) = self.spare().get_mut(..data.len()) {
            to.write_copy_of_slice(data);
            self.length = new_size;
        }
    }

    /// `String::write(off_t, const char *, size_t)`. Where upstream's offset
    /// leaves the buffer, a negative one writes at 0 and one past the content
    /// zeroes the gap.
    pub fn write(&mut self, offset: i64, data: &[u8]) {
        let offset = unsigned(offset.max(0));
        let Some(new_length) = offset.checked_add(data.len()) else {
            crate::fatal("a string write past the address space");
        };
        if new_length > self.size {
            self.reserve(page_aligned(new_length.saturating_mul(2)).max(new_length));
        }
        let length = self.length;
        // The buffer now holds `new_length` bytes.
        let Some(buffer) = self.allocation().get_mut(..new_length) else {
            return;
        };
        if let Some(gap) = buffer.get_mut(length..offset) {
            gap.fill(MaybeUninit::new(0));
        }
        if let Some(to) = buffer.get_mut(offset..) {
            to.write_copy_of_slice(data);
        }
        self.length = self.length.max(new_length);
    }

    pub fn set_null_terminated(&mut self) {
        if self.length == self.size {
            self.extend(self.length.wrapping_add(1));
        }
        if let Some(end) = self.spare().first_mut() {
            end.write(0);
        }
    }

    /// The first `delimiter` at or after `start`.
    fn find(&self, start: usize, delimiter: &[u8]) -> Option<usize> {
        strnstr(self.view().get(start..)?, delimiter)
    }

    /// `String::split`: each `delimiter`-terminated chunk from `offset` to `handler`,
    /// as its start in the buffer and its length, until `handler` returns false
    /// (`None`); otherwise where the unsplit tail starts. An empty `delimiter`
    /// matches everywhere, so only `handler` ends that loop.
    pub fn split(
        &mut self,
        delimiter: &[u8],
        mut handler: impl FnMut(*const u8, usize) -> bool,
    ) -> Option<usize> {
        self.split_with(delimiter, |s, start, length| {
            handler(s.str.wrapping_add(start), length)
        })
    }

    /// [`split`](Self::split), each chunk to `handler` as the string, lent back, with
    /// the chunk's start in it and its length: `None` once `handler` returned false.
    pub fn split_with(
        &mut self,
        delimiter: &[u8],
        mut handler: impl FnMut(&mut Self, usize, usize) -> bool,
    ) -> Option<usize> {
        let delimiter_length = delimiter.len();
        let mut start = unsigned(self.offset);
        let mut first = self.offset;
        let mut found = self.find(start, delimiter);
        while let Some(position) = found {
            let length = position + delimiter_length;
            let chunk = start.wrapping_sub(unsigned(first));
            if !handler(self, chunk, length.wrapping_add(unsigned(first))) {
                return None;
            }
            self.offset = self.offset.wrapping_add(signed(length));
            start = unsigned(self.offset);
            // Over the buffer as the handler left it.
            found = self.find(start, delimiter);
            first = 0;
        }
        if first == self.offset {
            self.offset = signed(self.length.wrapping_sub(delimiter_length));
        }
        Some(start.wrapping_sub(unsigned(first)))
    }

    /// The whole allocation, its first `length` bytes the content. Only the content is
    /// known initialized, so safe code may only write the rest.
    fn allocation(&mut self) -> &mut [MaybeUninit<u8>] {
        if self.str.is_null() {
            return &mut [];
        }
        // SAFETY: `str` holds `size` bytes, borrowed with `self`.
        unsafe { std::slice::from_raw_parts_mut(self.str.cast(), self.size) }
    }

    /// Replaces the content with what `read` writes into the allocation's first
    /// `length` bytes, zeroed first (none when the allocation is shorter): as many as
    /// it returns, at most `length`.
    pub fn read_zeroed(&mut self, length: usize, read: impl FnOnce(&mut [u8]) -> usize) {
        let room = self.allocation().get_mut(..length).map_or(0, |bytes| {
            bytes.fill(MaybeUninit::new(0));
            bytes.len()
        });
        // The zeroed bytes are content while `read` writes them.
        self.length = room;
        self.length = read(self.view_mut()).min(room);
    }

    /// The allocated bytes after the content, for a read to fill before `grow`. They are
    /// uninitialized, so safe code can only write them.
    pub fn spare(&mut self) -> &mut [MaybeUninit<u8>] {
        let length = self.length;
        self.allocation().get_mut(length..).unwrap_or_default()
    }

    pub fn release(&mut self) -> *mut u8 {
        let data = self.str;
        self.str = ptr::null_mut();
        self.size = 0;
        self.length = 0;
        self.offset = 0;
        data
    }

    pub fn repeat(&mut self, data: &[u8], count: usize) {
        debug_assert!(count > 0, "String::repeat repeats");
        if let &[byte] = data {
            if self.size < self.length.wrapping_add(count) {
                self.reserve(self.length.wrapping_add(count));
            }
            if let Some(to) = self.spare().get_mut(..count) {
                to.fill(MaybeUninit::new(byte));
                self.length += count;
            }
        } else {
            for _ in 0..count {
                self.append(data);
            }
        }
    }

    pub fn append_random_bytes(&mut self, length: usize, base64: bool) -> bool {
        let encoded = length.wrapping_add(2) / 3 * 4 + 1;
        let new_size =
            self.length
                .wrapping_add(length)
                .wrapping_add(if base64 { encoded } else { 0 });
        if new_size > self.size {
            self.reserve(page_aligned(new_size.wrapping_mul(2)));
        }
        let Some(spare) = self.spare().get_mut(..length) else {
            return false;
        };
        if !base64 {
            if crate::random_bytes(spare) != length {
                return false;
            }
            self.length = self.length.wrapping_add(length);
            return true;
        }
        // Upstream draws the raw bytes into the free space and encodes them over it; they
        // are scratch, so they go in a buffer of their own.
        let mut raw = vec![0u8; length];
        if crate::random_bytes(raw.as_mut_slice()) != length {
            return false;
        }
        let mut out = vec![0u8; encoded];
        let n = swoole_rs_base64::encode(&raw, &mut out);
        // The buffer's free space holds the encoding.
        let Some(at) = self.spare().get_mut(..n) else {
            return false;
        };
        at.write_copy_of_slice(out.get(..n).unwrap_or_default());
        self.length = self.length.wrapping_add(n);
        true
    }

    pub fn pop(&mut self, init_size: usize) -> *mut u8 {
        self.pop_owned(init_size)
            .map_or(ptr::null_mut(), |mut buffer| buffer.release())
    }

    /// Give up this allocation while preserving the unread tail in a fresh one.
    /// A failed allocation leaves this string unchanged.
    pub fn pop_owned(&mut self, init_size: usize) -> Option<OwnedString> {
        debug_assert!(
            self.length >= unsigned(self.offset),
            "String::pop's offset is inside"
        );
        let offset = unsigned(self.offset).min(self.length);
        // The bytes the String keeps.
        let tail = self.view().get(offset..).unwrap_or_default();
        let length = tail.len();
        let size = aligned(if length == 0 {
            init_size
        } else {
            length.max(init_size)
        });
        // An empty String has no allocator yet; the buffer then names the one that made it.
        let with = self.allocator.unwrap_or(TrustedAllocator::STD);
        let new_value: *mut u8 = with.allocate(size).cast();
        if new_value.is_null() {
            return None;
        }
        // The new buffer, that allocator's `size >= length` bytes, empty until the copy.
        let mut kept = Self {
            length: 0,
            size,
            offset: 0,
            str: new_value,
            allocator: Some(with),
        };
        if let Some(to) = kept.spare().get_mut(..length) {
            to.write_copy_of_slice(tail);
            kept.length = length;
        }
        Some(OwnedString(std::mem::replace(self, kept)))
    }

    pub fn reduce(&mut self, offset: i64) {
        debug_assert!(
            offset >= 0 && unsigned(offset) <= self.length,
            "String::reduce's offset is inside"
        );
        if offset == 0 {
            return;
        }
        let offset = unsigned(offset).min(self.length);
        let end = self.length;
        self.length -= offset;
        self.offset = 0;
        self.allocation().copy_within(offset..end, 0);
    }

    pub fn print(&self, value: bool) {
        let head = format!(
            "String[length={},size={},offset={}]=",
            self.length, self.size, self.offset
        );
        crate::sys::print(head.as_bytes());
        if value {
            // `%.*s` of the content: `print` stops at a NUL, as upstream's does.
            crate::sys::print(self.view());
            crate::sys::print(b"\n");
        } else {
            crate::sys::print(format!("{}\n", pointer(self.str)).as_bytes());
        }
    }
}

/// `%p` of `at`: `0x` and lowercase hex, and for null what this libc prints.
fn pointer(at: *const u8) -> String {
    if !at.is_null() {
        return format!("{at:p}");
    }
    if cfg!(all(target_os = "linux", target_env = "gnu")) {
        "(nil)".to_owned()
    } else if cfg!(target_env = "musl") {
        "0".to_owned()
    } else {
        "0x0".to_owned()
    }
}

/// `String(size_t, const Allocator *)`.
pub fn swr_string_new(
    this: &mut MaybeUninit<StringBuffer>,
    size: usize,
    with: Option<TrustedAllocator>,
) -> *mut StringBuffer {
    let string = this.write(StringBuffer::EMPTY);
    string.alloc(size, with);
    string
}

/// `String(const String &)`.
pub fn swr_string_copy(
    this: &mut MaybeUninit<StringBuffer>,
    source: &StringBuffer,
) -> *mut StringBuffer {
    let string = this.write(StringBuffer::EMPTY);
    string.copy_from(source);
    string
}

/// `String(String &&)`.
pub fn swr_string_move(
    this: &mut MaybeUninit<StringBuffer>,
    source: &mut StringBuffer,
) -> *mut StringBuffer {
    let string = this.write(StringBuffer::EMPTY);
    string.move_from(source);
    string
}

/// A copied substring, or an empty string when the range is outside the input.
#[must_use]
pub fn swr_string_substr(this: &StringBuffer, offset: usize, length: usize) -> StringBuffer {
    let mut result = StringBuffer::EMPTY;
    // Checked: upstream's `offset + len > length` wraps, and this fn is safe.
    if let Some(end) = offset.checked_add(length).filter(|&end| end <= this.length) {
        result.alloc(length, None);
        result.append(this.view().get(offset..end).unwrap_or_default());
    }
    result
}

/// `append(int)`: its decimal text.
pub fn swr_string_append_int(this: &mut StringBuffer, value: c_int) {
    let mut buffer = itoa::Buffer::new();
    this.append(buffer.format(value).as_bytes());
}

pub fn swr_string_write_string(this: &mut StringBuffer, offset: i64, source: &StringBuffer) {
    this.write(offset, source.view());
}

/// `new String(data.data(), data.size())`, owned: [`StringBuffer::dispose`] frees its bytes.
#[must_use]
pub fn boxed_string(data: &[u8]) -> Box<StringBuffer> {
    // `EMPTY` is all zeros, and LLVM turns a fresh allocation stored with zeros into
    // `calloc`, which costs more per string than `malloc`; opaque, it stays `malloc`.
    let mut string = Box::new(core::hint::black_box(StringBuffer::EMPTY));
    fill(&mut string, data);
    string
}

/// An empty `string`'s NUL-terminated copy of `data`.
fn fill<'a>(string: &'a mut StringBuffer, data: &[u8]) -> &'a mut StringBuffer {
    let length = data.len();
    string.alloc(length.wrapping_add(1), None);
    // The allocation holds `length + 1` bytes.
    if let Some((end, to)) = string
        .spare()
        .get_mut(..=length)
        .and_then(<[MaybeUninit<u8>]>::split_last_mut)
    {
        to.write_copy_of_slice(data);
        end.write(0);
        string.length = length;
    }
    string
}

/// `new String(size, with)`, owned: [`StringBuffer::dispose`] frees its bytes.
#[must_use]
pub fn boxed_sized_string(size: usize, with: Option<TrustedAllocator>) -> Box<StringBuffer> {
    // `EMPTY` is all zeros, and LLVM turns a fresh allocation stored with zeros into
    // `calloc`, which costs more per string than `malloc`; opaque, it stays `malloc`.
    let mut string = Box::new(core::hint::black_box(StringBuffer::EMPTY));
    string.alloc(size, with);
    string
}

/// `f` over this thread's String (`sw_tg_buffer()`), lent to `f` alone. A thread
/// `swoole_thread_init` didn't set up has none, which aborts.
#[inline]
pub fn with_thread_buffer<R>(f: impl FnOnce(&mut StringBuffer) -> R) -> R {
    try_with_thread_buffer(f).unwrap_or_else(|| crate::fatal("a thread without its buffer"))
}

/// [`with_thread_buffer`], or `None` on a thread without a buffer.
#[inline]
pub fn try_with_thread_buffer<R>(f: impl FnOnce(&mut StringBuffer) -> R) -> Option<R> {
    crate::lend_thread_buffer(|| {
        // SAFETY: null, or the calling thread's live buffer, which
        // `swoole_thread_init` set up, lent to `f` alone.
        unsafe { crate::thread_buffer().cast::<StringBuffer>().as_mut() }.map(f)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn new(text: &[u8]) -> StringBuffer {
        let mut s = MaybeUninit::uninit();
        fill(s.write(StringBuffer::EMPTY), text);
        unsafe { s.assume_init() }
    }

    #[test]
    fn append_grows_and_reduce_moves() {
        let mut s = new(b"abc");
        assert_eq!((s.length, s.size), (3, 8));
        s.append(b"defghijkl");
        assert_eq!((s.view(), s.size), (&b"abcdefghijkl"[..], 16));
        swr_string_append_int(&mut s, -42);
        s.reduce(3);
        assert_eq!(s.view(), b"defghijkl-42");
        let copy = s.view().to_vec();
        s.append(&copy);
        assert_eq!(s.view(), b"defghijkl-42defghijkl-42");
        s.set_null_terminated();
        assert_eq!(unsafe { *s.str.add(s.length) }, 0);
        s.dispose();
    }

    #[test]
    fn split_hands_each_chunk_and_keeps_the_tail() {
        let mut s = new(b"a\r\nbc\r\nd\r");
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        let ret = s.split(b"\r\n", |data, length| {
            chunks.push(unsafe { std::slice::from_raw_parts(data, length) }.to_vec());
            true
        });
        assert_eq!(chunks, [b"a\r\n".to_vec(), b"bc\r\n".to_vec()]);
        assert_eq!((ret, s.offset), (Some(7), 7));
        // No delimiter after the offset: it moves to the last place one could start.
        assert_eq!(s.split(b"\r\n", |_, _| false), Some(0));
        assert_eq!(s.offset, 7);
        s.dispose();
    }

    #[test]
    fn eof_search_finds_the_first_match() {
        assert_eq!(find_delimiter(b"ab\r\ncd\r\n", b"\r\n"), Some(2), "first");
        assert_eq!(find_delimiter(b"abc", b"\r\n"), None, "absent");
        assert_eq!(find_delimiter(b"\r", b"\r\n"), None, "shorter than the EOF");
        assert_eq!(find_delimiter(b"abc", b""), None, "no EOF");
    }

    #[test]
    fn strnpos_finds_the_needle_anywhere() {
        assert_eq!(
            find_delimiter(b"x; form-data; name=a", b"form-data;"),
            Some(3)
        );
        assert_eq!(find_delimiter(b"inline", b"attachment;"), None);
    }

    #[test]
    fn first_header_delimiter() {
        let end = |data: &[u8]| find_bytes(data, b"\r\n\r\n");
        assert_eq!(end(b"GET / HTTP/1.1\r\n\r\nbody"), Some(14));
        assert_eq!(end(b"GET / HTTP/1.1\r\n"), None);
        assert_eq!(end(b"a\r\n\r\nb"), Some(1));
        assert_eq!(end(b"\r\r\n\r\n\r\n"), Some(1));
        assert_eq!(end(b"body"), None);
        assert_eq!(find_bytes(b"body", b""), Some(0));
    }

    /// A short delimiter's first byte alone is no match, and a long one is still found.
    #[test]
    fn strnstr_finds_the_first_whole_needle() {
        assert_eq!(strnstr(b"a\r\rb\r\n", b"\r\n"), Some(4));
        assert_eq!(strnstr(b"ab\r", b"\r\n"), None);
        assert_eq!(strnstr(b"xx12345678x", b"12345678"), Some(2));
        assert_eq!(strnstr(b"1234567812345678x9", b"12345678x"), Some(8));
        assert_eq!(strnstr(b"12345678", b"123456789"), None);
    }

    /// A write's offset stays inside the buffer, and a disposed String is empty.
    #[test]
    fn write_offsets_stay_inside_the_buffer() {
        let mut s = new(b"abc");
        s.write(5, b"z");
        assert_eq!(s.view(), b"abc\0\0z");
        s.write(-1, b"A");
        assert_eq!(s.view(), b"Abc\0\0z");
        s.dispose();
        assert!(s.as_ptr().is_null() && s.size() == 0 && s.is_empty());
    }

    /// `pop` on an empty String gives its new buffer the standard allocator, so a later
    /// `reserve` keeps the content and `dispose` frees it.
    #[test]
    fn pop_on_an_empty_string_records_its_allocator() {
        let mut s = StringBuffer::EMPTY;
        assert!(s.pop(8).is_null(), "no old buffer to hand back");
        s.append(b"123");
        s.reserve(64);
        assert_eq!(s.view(), b"123");
        s.dispose();
        assert!(s.as_ptr().is_null());
    }

    #[test]
    fn substr_copy_move_and_release() {
        let mut s = new(b"hello world");
        let mut sub = swr_string_substr(&s, 6, 5);
        assert_eq!(sub.view(), b"world");
        assert!(swr_string_substr(&s, 6, 6).str.is_null());
        assert!(
            swr_string_substr(&s, usize::MAX, 2).str.is_null(),
            "no wrap past the end"
        );
        let mut moved = StringBuffer::EMPTY;
        moved.move_from(&mut sub);
        assert!(sub.str.is_null() && moved.view() == b"world");
        let mut copied = StringBuffer::EMPTY;
        copied.copy_from(&moved);
        assert!(strnstr(copied.view(), b"orl").is_some());
        assert!(copied.view().ends_with(b"ld"));
        unsafe { libc::free(moved.release().cast()) };
        copied.dispose();
        s.dispose();
    }

    #[test]
    fn view_mut_writes_the_content_alone() {
        let mut empty = StringBuffer::EMPTY;
        assert_eq!(empty.view_mut(), b"");
        let mut s = boxed_string(b"abc");
        s.view_mut().make_ascii_uppercase();
        assert_eq!(s.view(), b"ABC");
        s.truncate(1);
        assert_eq!(s.view_mut(), b"A");
        s.dispose();
    }
}

#[cfg(test)]
#[path = "../../../../tests/modules/buffer/string/originals.rs"]
mod originals;

#[cfg(test)]
#[path = "../../../../tests/modules/buffer/thread_buffer.rs"]
mod thread_buffer;

#[cfg(test)]
#[path = "../../../../tests/modules/buffer/owned_string.rs"]
mod owned_tests;
