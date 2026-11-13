// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/string.cc and include/swoole_string.h
// (Swoole contributors).
//! `swoole::String`: the C++ object itself (its layout pinned on both sides), and
//! every method upstream gives a body. With `upstream-symbols` (the hybrid's build)
//! each is defined under its upstream (Itanium) symbol and the C++ sources no
//! longer define it.
//!
//! An allocation failure aborts: upstream throws `std::bad_alloc`, which no
//! first-party code catches. Arithmetic wraps, as upstream's `size_t` does, and
//! the casts are upstream's C casts.
use std::ffi::{c_int, c_void};
use std::mem::{MaybeUninit, offset_of};
use std::ptr;
use swoole_rs_corebase::CCast;

/// `swoole::Allocator`.
#[repr(C)]
pub struct Allocator {
    pub malloc: unsafe extern "C" fn(usize) -> *mut c_void,
    pub calloc: unsafe extern "C" fn(usize, usize) -> *mut c_void,
    pub realloc: unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void,
    pub free: unsafe extern "C" fn(*mut c_void),
}

impl Allocator {
    /// `malloc(size)`: `size` bytes of this allocator's, or null.
    #[inline]
    fn allocate(&self, size: usize) -> *mut c_void {
        // SAFETY: an allocator's malloc, which takes any size.
        unsafe { (self.malloc)(size) }
    }
}

/// `SwooleG.std_allocator`, which only ever holds libc's allocator.
pub static STD_ALLOCATOR: Allocator = Allocator {
    malloc: libc::malloc,
    calloc: libc::calloc,
    realloc: libc::realloc,
    free: libc::free,
};

/// `swoole::String` (`string.cc` asserts the same layout).
///
/// Its fields are private, so safe code keeps what the safe methods trust: `str`
/// is null with `length` and `size` 0, or the live allocation of `size` bytes
/// that `allocator` (then set) made, whose first `length` are initialized.
/// `offset` holds anything (a cursor, or a caller's flags), so a
/// method that indexes by it bounds it first. C++ writes the fields directly: a
/// `&CppString` made from a C++ pointer is that pointer's `unsafe` promise.
#[repr(C)]
pub struct CppString {
    length: usize,
    size: usize,
    offset: i64,
    str: *mut u8,
    /// A nullable `const Allocator *`, which lives for the process.
    allocator: Option<&'static Allocator>,
}

const _: () = assert!(
    size_of::<CppString>() == 40
        && offset_of!(CppString, offset) == 16
        && offset_of!(CppString, str) == 24
        && offset_of!(CppString, allocator) == 32
        && size_of::<Allocator>() == 32,
    "swoole::String's layout"
);

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
    let page = swoole_rs_corebase::swr_v1_core_pagesize() as usize;
    size.wrapping_add(page.wrapping_sub(size % page))
}

/// `swoole_strnstr`: the first `needle` in `haystack`, both lengths cut to its `uint32_t`.
fn strnstr(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let cut = |bytes: &[u8]| bytes.len().c_cast::<u32>() as usize;
    let haystack = haystack.get(..cut(haystack)).unwrap_or_default();
    let needle = needle.get(..cut(needle)).unwrap_or_default();
    debug_assert!(!needle.is_empty(), "swoole_strnstr's needle is not empty");
    memchr::memmem::find(haystack, needle)
}

/// An allocator's result, aborting on failure as `std::bad_alloc` would.
fn checked(pointer: *mut c_void) -> *mut u8 {
    if pointer.is_null() {
        swoole_rs_corebase::fatal("out of memory");
    }
    pointer.cast()
}

impl CppString {
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

    /// Count the buffer's first `length` bytes as the content.
    ///
    /// # Safety
    /// `length <= size()`, and those bytes are initialized.
    #[inline]
    pub const unsafe fn set_len(&mut self, length: usize) {
        self.length = length;
    }

    /// `String::alloc`: a fresh buffer, from the standard allocator when `with` is `None`.
    pub fn alloc(&mut self, size: usize, with: Option<&'static Allocator>) {
        let with = with.unwrap_or(&STD_ALLOCATOR);
        let size = aligned(size);
        self.length = 0;
        self.size = size;
        self.offset = 0;
        self.str = checked(with.allocate(size));
        self.allocator = Some(with);
    }

    /// The destructor's body, which leaves the String empty.
    pub fn dispose(&mut self) {
        let data = self.release();
        if let Some(with) = self.allocator
            && !data.is_null()
        {
            // SAFETY: `data` was this allocator's live allocation.
            unsafe { (with.free)(data.cast()) };
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

    /// The `length` bytes at `str`.
    #[must_use]
    pub fn view(&self) -> &[u8] {
        if self.length == 0 {
            return &[];
        }
        // SAFETY: `str` holds `length` bytes.
        unsafe { std::slice::from_raw_parts(self.str, self.length) }
    }

    /// `String::reserve`, never below the content's length.
    pub fn reserve(&mut self, new_size: usize) {
        let Some(with) = self.allocator.filter(|_| self.size != 0) else {
            self.alloc(new_size, None);
            return;
        };
        let new_size = aligned(new_size.max(self.length));
        // SAFETY: `str` is this allocator's live allocation.
        self.str = checked(unsafe { (with.realloc)(self.str.cast(), new_size) });
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
            swoole_rs_corebase::fatal("a string write past the address space");
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
    /// as its start in the buffer and its length, until `handler` returns false. An
    /// empty `delimiter` matches everywhere, so only `handler` ends that loop.
    pub fn split(
        &mut self,
        delimiter: &[u8],
        mut handler: impl FnMut(*const u8, usize) -> bool,
    ) -> isize {
        let delimiter_length = delimiter.len();
        let mut start = unsigned(self.offset);
        let mut first = self.offset;
        let mut found = self.find(start, delimiter);
        while let Some(position) = found {
            let length = position + delimiter_length;
            let chunk = self.str.wrapping_add(start.wrapping_sub(unsigned(first)));
            if !handler(chunk, length.wrapping_add(unsigned(first))) {
                return -1;
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
        start.wrapping_sub(unsigned(first)).cast_signed()
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

    /// The allocation's first `length` bytes, zeroed, for a read to fill before
    /// `set_len`; `None` when the allocation is shorter. Content among them is zeroed
    /// too, so every byte lent is initialized and stays so whatever is written.
    pub fn zeroed(&mut self, length: usize) -> Option<&mut [u8]> {
        let bytes = self.allocation().get_mut(..length)?;
        bytes.fill(MaybeUninit::new(0));
        // SAFETY: every byte was just written.
        Some(unsafe { bytes.assume_init_mut() })
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
            if swoole_rs_corebase::random_bytes(spare) != length {
                return false;
            }
            self.length = self.length.wrapping_add(length);
            return true;
        }
        // Upstream draws the raw bytes into the free space and encodes them over it; they
        // are scratch, so they go in a buffer of their own.
        let mut raw = vec![0u8; length];
        if swoole_rs_corebase::random_bytes(raw.as_mut_slice()) != length {
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
        let with = self.allocator.unwrap_or(&STD_ALLOCATOR);
        let new_value: *mut u8 = with.allocate(size).cast();
        if new_value.is_null() {
            return ptr::null_mut();
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
        std::mem::replace(self, kept).str
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
        if value {
            let head = format!(
                "String[length={},size={},offset={}]=",
                self.length, self.size, self.offset
            );
            // `%.*s` of the content: `print` stops at a NUL, as upstream's does.
            swoole_rs_corebase::sys::print(head.as_bytes());
            swoole_rs_corebase::sys::print(self.view());
            swoole_rs_corebase::sys::print(b"\n");
        } else {
            // SAFETY: the arguments match the format.
            unsafe {
                libc::printf(
                    c"String[length=%zu,size=%zu,offset=%jd]=%p\n".as_ptr(),
                    self.length,
                    self.size,
                    self.offset,
                    self.str,
                )
            };
        }
    }
}

// Constructors and destructors return `this`: Apple's arm64 C++ ABI does, and
// elsewhere the caller ignores it. Nothing derives from String, so only the
// complete-object (C1/D1) symbols.
// A constructor's `this` is uninitialized storage; every other `this` is a live String.
upstream! {
    "_ZN6swoole6StringC1EmPKNS_9AllocatorE", "_ZN6swoole6StringC1EmPKNS_9AllocatorE" =>
    /// `String(size_t, const Allocator *)`.
    pub extern "C" fn swr_string_new(this: &mut MaybeUninit<CppString>, size: usize, with: Option<&'static Allocator>) -> *mut CppString {
        let string = this.write(CppString::EMPTY);
        string.alloc(size, with);
        string
    }
    "_ZN6swoole6StringC1ERKS0_", "_ZN6swoole6StringC1ERKS0_" =>
    /// `String(const String &)`.
    pub extern "C" fn swr_string_copy(this: &mut MaybeUninit<CppString>, source: &CppString) -> *mut CppString {
        let string = this.write(CppString::EMPTY);
        string.copy_from(source);
        string
    }
    "_ZN6swoole6StringC1EOS0_", "_ZN6swoole6StringC1EOS0_" =>
    /// `String(String &&)`.
    pub extern "C" fn swr_string_move(this: &mut MaybeUninit<CppString>, source: &mut CppString) -> *mut CppString {
        let string = this.write(CppString::EMPTY);
        string.move_from(source);
        string
    }
    "_ZN6swoole6StringD1Ev", "_ZN6swoole6StringD1Ev" =>
    /// `~String()`.
    pub extern "C" fn swr_string_drop(this: &mut CppString) -> *mut CppString {
        this.dispose();
        this
    }
    "_ZNK6swoole6String6substrEmm", "_ZNK6swoole6String6substrEmm" =>
    /// `String substr(size_t, size_t) const`. The C++ ABI returns a class with a
    /// destructor in caller memory (`sret`), as C does a 40-byte struct.
    #[must_use]
    pub extern "C" fn swr_string_substr(this: &CppString, offset: usize, length: usize) -> CppString {
        let mut result = CppString::EMPTY;
        // Checked: upstream's `offset + len > length` wraps, and this fn is safe.
        if let Some(end) = offset.checked_add(length).filter(|&end| end <= this.length) {
            result.alloc(length, None);
            result.append(this.view().get(offset..end).unwrap_or_default());
        }
        result
    }
    "_ZN6swoole6String6extendEv", "_ZN6swoole6String6extendEv" =>
    pub extern "C" fn swr_string_extend_double(this: &mut CppString) {
        this.extend(this.size.wrapping_mul(2));
    }
    "_ZN6swoole6String6extendEm", "_ZN6swoole6String6extendEm" =>
    pub extern "C" fn swr_string_extend(this: &mut CppString, new_size: usize) {
        this.extend(new_size);
    }
    "_ZN6swoole6String7reserveEm", "_ZN6swoole6String7reserveEm" =>
    pub extern "C" fn swr_string_reserve(this: &mut CppString, new_size: usize) {
        this.reserve(new_size);
    }
    "_ZN6swoole6String7releaseEv", "_ZN6swoole6String7releaseEv" =>
    pub extern "C" fn swr_string_release(this: &mut CppString) -> *mut u8 {
        this.release()
    }
    "_ZN6swoole6String6appendEi", "_ZN6swoole6String6appendEi" =>
    /// `append(int)`: its decimal text.
    pub extern "C" fn swr_string_append_int(this: &mut CppString, value: c_int) {
        let mut buffer = itoa::Buffer::new();
        this.append(buffer.format(value).as_bytes());
    }
    "_ZN6swoole6String19append_random_bytesEmb", "_ZN6swoole6String19append_random_bytesEmb" =>
    pub extern "C" fn swr_string_append_random_bytes(this: &mut CppString, length: usize, base64: bool) -> bool {
        this.append_random_bytes(length, base64)
    }
    "_ZN6swoole6String5writeElRKS0_", "_ZN6swoole6String5writeExRKS0_" =>
    pub extern "C" fn swr_string_write_string(this: &mut CppString, offset: i64, source: &CppString) {
        this.write(offset, source.view());
    }
    "_ZN6swoole6String19set_null_terminatedEv", "_ZN6swoole6String19set_null_terminatedEv" =>
    pub extern "C" fn swr_string_set_null_terminated(this: &mut CppString) {
        this.set_null_terminated();
    }
    "_ZNK6swoole6String5printEb", "_ZNK6swoole6String5printEb" =>
    /// `print(bool)`, through C's stdout as upstream.
    pub extern "C" fn swr_string_print(this: &CppString, value: bool) {
        this.print(value);
    }
    "_ZN6swoole6String3popEm", "_ZN6swoole6String3popEm" =>
    /// `pop(size_t)`: hands over the buffer and keeps the bytes after `offset` in a new one.
    pub extern "C" fn swr_string_pop(this: &mut CppString, init_size: usize) -> *mut u8 {
        this.pop(init_size)
    }
    "_ZN6swoole6String6reduceEl", "_ZN6swoole6String6reduceEx" =>
    /// `reduce(off_t)`: moves the bytes after `offset` to the head.
    pub extern "C" fn swr_string_reduce(this: &mut CppString, offset: i64) {
        this.reduce(offset);
    }
}

/// `String(data.data(), data.size())` in `this`: a NUL-terminated copy.
fn from_bytes<'a>(this: &'a mut MaybeUninit<CppString>, data: &[u8]) -> &'a mut CppString {
    let length = data.len();
    let string = this.write(CppString::EMPTY);
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

/// `new String(data.data(), data.size())`, in boxed storage [`delete_string`] frees.
#[must_use]
pub fn new_string(data: &[u8]) -> *mut CppString {
    from_bytes(Box::leak(Box::new_uninit()), data)
}

/// `new String(size, with)`, in boxed storage [`delete_string`] frees.
#[must_use]
pub fn new_sized_string(size: usize, with: Option<&'static Allocator>) -> *mut CppString {
    swr_string_new(Box::leak(Box::new_uninit()), size, with)
}

/// `delete string`.
///
/// # Safety
/// `string` came from [`new_string`] or [`new_sized_string`] and is not used again.
pub unsafe fn delete_string(string: *mut CppString) {
    // SAFETY: as the caller promises.
    unsafe { Box::from_raw(string) }.dispose();
}

/// `f` over this thread's String (`sw_tg_buffer()`), lent to `f` alone.
pub fn with_thread_buffer<R>(f: impl FnOnce(&mut CppString) -> R) -> R {
    swoole_rs_corebase::lend_thread_buffer(|| {
        // SAFETY: the calling thread's live buffer, which `swoole_thread_init` set
        // up, lent to `f` alone.
        f(unsafe { &mut *swoole_rs_corebase::thread_buffer().cast::<CppString>() })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn new(text: &[u8]) -> CppString {
        let mut s = MaybeUninit::uninit();
        from_bytes(&mut s, text);
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
        assert_eq!((ret, s.offset), (7, 7));
        // No delimiter after the offset: it moves to the last place one could start.
        assert_eq!(s.split(b"\r\n", |_, _| false), 0);
        assert_eq!(s.offset, 7);
        s.dispose();
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
        let mut s = CppString::EMPTY;
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
        let mut moved = CppString::EMPTY;
        moved.move_from(&mut sub);
        assert!(sub.str.is_null() && moved.view() == b"world");
        let mut copied = CppString::EMPTY;
        copied.copy_from(&moved);
        assert!(strnstr(copied.view(), b"orl").is_some());
        assert!(copied.view().ends_with(b"ld"));
        unsafe { libc::free(moved.release().cast()) };
        copied.dispose();
        s.dispose();
    }

    #[test]
    fn new_and_delete_string() {
        let s = new_string(b"abc");
        assert_eq!(unsafe { &*s }.view(), b"abc");
        unsafe { delete_string(s) };
        let s = new_sized_string(16, None);
        assert_eq!(unsafe { ((*s).length, (*s).size) }, (0, 16));
        unsafe { delete_string(s) };
    }
}

#[cfg(test)]
#[path = "../../../tests/modules/buffer/string/originals.rs"]
mod originals;
