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
use std::ffi::{c_char, c_int, c_void};
use std::mem::{MaybeUninit, offset_of};
use std::ptr;

/// `swoole::Allocator`.
#[repr(C)]
pub struct Allocator {
    pub malloc: unsafe extern "C" fn(usize) -> *mut c_void,
    pub calloc: unsafe extern "C" fn(usize, usize) -> *mut c_void,
    pub realloc: unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void,
    pub free: unsafe extern "C" fn(*mut c_void),
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
#[expect(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    reason = "upstream's C cast"
)]
const fn unsigned(value: i64) -> usize {
    value as usize
}

/// `(off_t) size_t`.
#[expect(clippy::cast_possible_wrap, reason = "upstream's C cast")]
const fn signed(value: usize) -> i64 {
    value as i64
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

/// # Safety
/// `data` is readable for `length` bytes, or `length` is zero.
unsafe fn bytes<'a>(data: *const u8, length: usize) -> &'a [u8] {
    if length == 0 {
        return &[];
    }
    // SAFETY: the caller's readable range.
    unsafe { std::slice::from_raw_parts(data, length) }
}

/// `swoole_strnstr`: the first `needle` in `haystack`, both lengths cut to its `uint32_t`.
///
/// # Safety
/// Both are readable for their cut lengths.
#[expect(
    clippy::cast_possible_truncation,
    reason = "swoole_strnstr's uint32_t lengths"
)]
unsafe fn strnstr(
    haystack: *const u8,
    haystack_length: usize,
    needle: *const u8,
    needle_length: usize,
) -> Option<usize> {
    let (haystack_length, needle_length) = (
        haystack_length as u32 as usize,
        needle_length as u32 as usize,
    );
    debug_assert!(needle_length > 0, "swoole_strnstr's needle is not empty");
    if needle_length > haystack_length {
        return None;
    }
    // SAFETY: the caller's readable range.
    let haystack = unsafe { bytes(haystack, haystack_length) };
    // SAFETY: as above.
    let needle = unsafe { bytes(needle, needle_length) };
    memchr::memmem::find(haystack, needle)
}

/// An allocator's result, aborting on failure as `std::bad_alloc` would.
fn checked(pointer: *mut c_void) -> *mut u8 {
    if pointer.is_null() {
        std::process::abort();
    }
    pointer.cast()
}

/// The C++ side of `String::split`: calls the handler it lends.
pub type SplitCall = unsafe extern "C" fn(*const c_void, *const c_char, usize) -> bool;

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
        // SAFETY: an allocator's malloc.
        self.str = checked(unsafe { (with.malloc)(size) });
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
        // SAFETY: `str` holds `length` bytes.
        unsafe { bytes(self.str, self.length) }
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
    ///
    /// # Safety
    /// `data` is readable for `length` bytes after any reallocation.
    pub unsafe fn append(&mut self, data: *const u8, length: usize) {
        let new_size = self.length.wrapping_add(length);
        if new_size > self.size {
            self.reserve(new_size);
        }
        // SAFETY: the buffer holds `new_size` bytes; `data` is readable.
        unsafe { ptr::copy(data, self.str.wrapping_add(self.length), length) };
        self.length = new_size;
    }

    /// `String::write(off_t, const char *, size_t)`. Where upstream's offset
    /// leaves the buffer, a negative one writes at 0 and one past the content
    /// zeroes the gap.
    ///
    /// # Safety
    /// `data` is readable for `length` bytes.
    pub unsafe fn write(&mut self, offset: i64, data: *const u8, length: usize) {
        let offset = unsigned(offset.max(0));
        let Some(new_length) = offset.checked_add(length) else {
            std::process::abort();
        };
        if new_length > self.size {
            self.reserve(page_aligned(new_length.saturating_mul(2)).max(new_length));
        }
        if offset > self.length {
            // SAFETY: the gap lies inside the buffer, which holds `new_length` bytes.
            unsafe {
                ptr::write_bytes(self.str.wrapping_add(self.length), 0, offset - self.length);
            }
        }
        // SAFETY: the buffer holds `new_length` bytes; `data` is readable.
        unsafe { ptr::copy(data, self.str.wrapping_add(offset), length) };
        if new_length > self.length {
            self.length = new_length;
        }
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
    ///
    /// # Safety
    /// `delimiter` holds `delimiter_length` bytes.
    unsafe fn find(
        &self,
        start: usize,
        delimiter: *const u8,
        delimiter_length: usize,
    ) -> Option<usize> {
        // SAFETY: `str` holds `length` bytes, so `length - start` from `start`; the delimiter is readable.
        unsafe {
            strnstr(
                self.str.wrapping_add(start),
                self.length.wrapping_sub(start),
                delimiter,
                delimiter_length,
            )
        }
    }

    /// `String::split`: each `delimiter`-terminated chunk from `offset` to `handler`.
    ///
    /// # Safety
    /// `delimiter` holds `delimiter_length > 0` bytes; `call` is safe with `handler`;
    /// `0 <= offset() <= len()`.
    pub unsafe fn split(
        &mut self,
        delimiter: *const u8,
        delimiter_length: usize,
        call: SplitCall,
        handler: *const c_void,
    ) -> isize {
        let mut start = unsigned(self.offset);
        let mut first = self.offset;
        // SAFETY: as the caller promises.
        let mut found = unsafe { self.find(start, delimiter, delimiter_length) };
        while let Some(position) = found {
            let length = position + delimiter_length;
            let chunk = self.str.wrapping_add(start.wrapping_sub(unsigned(first)));
            // SAFETY: the chunk lies inside `str`; `call` accepts `handler`.
            if !unsafe { call(handler, chunk.cast(), length.wrapping_add(unsigned(first))) } {
                return -1;
            }
            self.offset = self.offset.wrapping_add(signed(length));
            start = unsigned(self.offset);
            // SAFETY: as the caller promises, over the buffer as the handler left it.
            found = unsafe { self.find(start, delimiter, delimiter_length) };
            first = 0;
        }
        if first == self.offset {
            self.offset = signed(self.length.wrapping_sub(delimiter_length));
        }
        start.wrapping_sub(unsigned(first)).cast_signed()
    }

    /// `String::grow`: counts `increment` more bytes, doubling a buffer they fill.
    ///
    /// # Safety
    /// As [`Self::set_len`] of `len() + increment`.
    pub unsafe fn grow(&mut self, increment: usize) {
        self.length = self.length.wrapping_add(increment);
        if self.length == self.size {
            self.reserve(self.size.wrapping_mul(2));
        }
    }

    /// The allocated bytes after the content, for a read to fill before `grow`. They are
    /// uninitialized, so safe code can only write them.
    pub fn spare(&mut self) -> &mut [MaybeUninit<u8>] {
        if self.str.is_null() {
            return &mut [];
        }
        // SAFETY: `str` holds `size >= length` bytes, borrowed with `self`.
        unsafe {
            std::slice::from_raw_parts_mut(
                self.str.wrapping_add(self.length).cast(),
                self.size - self.length,
            )
        }
    }

    pub fn release(&mut self) -> *mut u8 {
        let data = self.str;
        self.str = ptr::null_mut();
        self.size = 0;
        self.length = 0;
        self.offset = 0;
        data
    }

    /// # Safety
    /// `data` holds `length` bytes.
    pub unsafe fn repeat(&mut self, data: *const u8, length: usize, count: usize) {
        debug_assert!(count > 0, "String::repeat repeats");
        if length == 1 {
            if self.size < self.length.wrapping_add(count) {
                self.reserve(self.length.wrapping_add(count));
            }
            // SAFETY: `data` holds one byte.
            let byte = unsafe { *data };
            if let Some(to) = self.spare().get_mut(..count) {
                for slot in to {
                    slot.write(byte);
                }
                self.length += count;
            }
        } else {
            for _ in 0..count {
                // SAFETY: as the caller promises.
                unsafe { self.append(data, length) };
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
        let value = self.str;
        let offset = unsigned(self.offset).min(self.length);
        let length = self.length - offset;
        let size = aligned(if length == 0 {
            init_size
        } else {
            length.max(init_size)
        });
        // An empty String has no allocator yet; the buffer then names the one that made it.
        let with = self.allocator.unwrap_or(&STD_ALLOCATOR);
        // SAFETY: an allocator's malloc.
        let new_value: *mut u8 = unsafe { (with.malloc)(size) }.cast();
        if new_value.is_null() {
            return ptr::null_mut();
        }
        self.allocator = Some(with);
        self.str = new_value;
        self.size = size;
        self.length = length;
        if length > 0 {
            let from = value.wrapping_add(offset);
            // SAFETY: the old buffer holds `length` bytes from `offset`; the new one `size >= length`.
            unsafe { ptr::copy_nonoverlapping(from, new_value, length) };
        }
        self.offset = 0;
        value
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
        self.length -= offset;
        self.offset = 0;
        if self.length == 0 {
            return;
        }
        let from = self.str.wrapping_add(offset);
        // SAFETY: both ranges lie inside `str`.
        unsafe { ptr::copy(from, self.str, self.length) };
    }

    pub fn print(&self, value: bool) {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_possible_wrap,
            reason = "upstream's (int) length"
        )]
        let precision = self.length as c_int;
        if value {
            // SAFETY: the arguments match the format; `str` holds `length` bytes.
            unsafe {
                libc::printf(
                    c"String[length=%zu,size=%zu,offset=%jd]=%.*s\n".as_ptr(),
                    self.length,
                    self.size,
                    self.offset,
                    precision,
                    self.str,
                )
            };
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
// complete-object (C1/D1) symbols, and the one base-object constructor a delegating
// constructor in the header calls.
// A constructor's `this` is uninitialized storage; every other `this` is a live String.
upstream! {
    "_ZN6swoole6StringC1EmPKNS_9AllocatorE", "_ZN6swoole6StringC1EmPKNS_9AllocatorE" =>
    /// `String(size_t, const Allocator *)`.
    pub extern "C" fn swr_string_new(this: &mut MaybeUninit<CppString>, size: usize, with: Option<&'static Allocator>) -> *mut CppString {
        let string = this.write(CppString::EMPTY);
        string.alloc(size, with);
        string
    }
    "_ZN6swoole6StringC1EPKcm", "_ZN6swoole6StringC1EPKcm" =>
    /// `String(const char *, size_t)`: a NUL-terminated copy.
    /// # Safety
    /// `data` holds `length` bytes.
    pub unsafe extern "C" fn swr_string_from(this: &mut MaybeUninit<CppString>, data: *const u8, length: usize) -> *mut CppString {
        let string = this.write(CppString::EMPTY);
        string.alloc(length.wrapping_add(1), None);
        // SAFETY: as the caller promises.
        let data = unsafe { bytes(data, length) };
        // The allocation holds `length + 1` bytes.
        if let Some((end, to)) = string.spare().get_mut(..=length).and_then(<[MaybeUninit<u8>]>::split_last_mut) {
            to.write_copy_of_slice(data);
            end.write(0);
            string.length = length;
        }
        string
    }
    "_ZN6swoole6StringC2EPKcm", "_ZN6swoole6StringC2EPKcm" =>
    /// The base-object `String(const char *, size_t)`: the header's
    /// `String(const std::string &)` delegates to it when clang emits no
    /// constructor aliases (as under `ASan`, `tools/asan.py`'s `string-originals`).
    /// # Safety
    /// As `swr_string_from`.
    pub unsafe extern "C" fn swr_string_from_base(this: &mut MaybeUninit<CppString>, data: *const u8, length: usize) -> *mut CppString {
        // SAFETY: as the caller promises.
        unsafe { swr_string_from(this, data, length) }
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
    "_ZN6swoole6StringaSERKS0_", "_ZN6swoole6StringaSERKS0_" =>
    /// `String &operator=(const String &)`, where `source` may be `this`.
    /// # Safety
    /// Both are live.
    pub unsafe extern "C" fn swr_string_assign_copy(this: *mut CppString, source: *const CppString) -> *mut CppString {
        if !ptr::eq(this, source) {
            // SAFETY: as the caller promises, and they differ.
            let string = unsafe { &mut *this };
            string.dispose();
            // SAFETY: as above.
            string.copy_from(unsafe { &*source });
        }
        this
    }
    "_ZN6swoole6StringaSEOS0_", "_ZN6swoole6StringaSEOS0_" =>
    /// `String &operator=(String &&)`, where `source` may be `this`.
    /// # Safety
    /// Both are live.
    pub unsafe extern "C" fn swr_string_assign_move(this: *mut CppString, source: *mut CppString) -> *mut CppString {
        if !ptr::eq(this, source) {
            // SAFETY: as the caller promises, and they differ.
            let string = unsafe { &mut *this };
            string.dispose();
            // SAFETY: as above.
            string.move_from(unsafe { &mut *source });
        }
        this
    }
    "_ZNK6swoole6String8containsEPKcm", "_ZNK6swoole6String8containsEPKcm" =>
    /// # Safety
    /// `needle` holds `length > 0` bytes.
    #[must_use]
    pub unsafe extern "C" fn swr_string_contains(this: &CppString, needle: *const u8, length: usize) -> bool {
        // SAFETY: `str` holds `length` bytes; the caller's needle is readable.
        unsafe { strnstr(this.str, this.length, needle, length) }.is_some()
    }
    "_ZNK6swoole6String11starts_withEPKcm", "_ZNK6swoole6String11starts_withEPKcm" =>
    /// # Safety
    /// `needle` holds `length` bytes.
    #[must_use]
    pub unsafe extern "C" fn swr_string_starts_with(this: &CppString, needle: *const u8, length: usize) -> bool {
        // SAFETY: as the caller promises.
        this.view().starts_with(unsafe { bytes(needle, length) })
    }
    "_ZNK6swoole6String9ends_withEPKcm", "_ZNK6swoole6String9ends_withEPKcm" =>
    /// # Safety
    /// `needle` holds `length` bytes.
    #[must_use]
    pub unsafe extern "C" fn swr_string_ends_with(this: &CppString, needle: *const u8, length: usize) -> bool {
        // SAFETY: as the caller promises.
        this.view().ends_with(unsafe { bytes(needle, length) })
    }
    "_ZNK6swoole6String6equalsEPKcm", "_ZNK6swoole6String6equalsEPKcm" =>
    /// # Safety
    /// `data` holds `length` bytes.
    #[must_use]
    pub unsafe extern "C" fn swr_string_equals(this: &CppString, data: *const u8, length: usize) -> bool {
        // SAFETY: as the caller promises.
        this.view() == unsafe { bytes(data, length) }
    }
    "_ZN6swoole6String4growEm", "_ZN6swoole6String4growEm" =>
    /// # Safety
    /// As `CppString::grow`.
    pub unsafe extern "C" fn swr_string_grow(this: &mut CppString, increment: usize) {
        // SAFETY: as the caller promises.
        unsafe { this.grow(increment) };
    }
    "_ZNK6swoole6String6substrEmm", "_ZNK6swoole6String6substrEmm" =>
    /// `String substr(size_t, size_t) const`. The C++ ABI returns a class with a
    /// destructor in caller memory (`sret`), as C does a 40-byte struct.
    #[must_use]
    pub extern "C" fn swr_string_substr(this: &CppString, offset: usize, length: usize) -> CppString {
        let mut result = CppString::EMPTY;
        // Checked: upstream's `offset + len > length` wraps, and this fn is safe.
        if offset.checked_add(length).is_some_and(|end| end <= this.length) {
            result.alloc(length, None);
            // SAFETY: the range lies inside `str`.
            unsafe { result.append(this.str.wrapping_add(offset), length) };
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
    "_ZN6swoole6String6repeatEPKcmm", "_ZN6swoole6String6repeatEPKcmm" =>
    /// # Safety
    /// `data` holds `length` bytes.
    pub unsafe extern "C" fn swr_string_repeat(this: &mut CppString, data: *const u8, length: usize, count: usize) {
        // SAFETY: as the caller promises.
        unsafe { this.repeat(data, length, count) };
    }
    "_ZN6swoole6String6appendEPKcm", "_ZN6swoole6String6appendEPKcm" =>
    /// # Safety
    /// `data` holds `length` bytes.
    pub unsafe extern "C" fn swr_string_append(this: &mut CppString, data: *const u8, length: usize) {
        // SAFETY: as the caller promises.
        unsafe { this.append(data, length) };
    }
    "_ZN6swoole6String6appendEi", "_ZN6swoole6String6appendEi" =>
    /// `append(int)`: its decimal text.
    pub extern "C" fn swr_string_append_int(this: &mut CppString, value: c_int) {
        let mut buffer = itoa::Buffer::new();
        let text = buffer.format(value);
        // SAFETY: `text` is local.
        unsafe { this.append(text.as_ptr(), text.len()) };
    }
    "_ZN6swoole6String6appendERKS0_", "_ZN6swoole6String6appendERKS0_" =>
    /// `append(const String &)`, which may be `this`.
    /// # Safety
    /// Both are live.
    pub unsafe extern "C" fn swr_string_append_string(this: *mut CppString, source: *const CppString) {
        // SAFETY: as the caller promises.
        let length = unsafe { &*source }.length;
        // SAFETY: as above; `source` is read again only when it isn't `this`.
        let string = unsafe { &mut *this };
        let new_size = string.length.wrapping_add(length);
        if new_size > string.size {
            string.reserve(new_size);
        }
        // Read after any reallocation; a `source` that is `this` is read through `string`,
        // whose borrow would otherwise end the caller's `source` pointer.
        let data = if ptr::eq(this, source) {
            string.str
        } else {
            // SAFETY: as the caller promises; distinct from `string`.
            unsafe { &*source }.str
        };
        // SAFETY: the buffer holds `new_size` bytes; `source` holds `length`.
        unsafe { ptr::copy(data, string.str.wrapping_add(string.length), length) };
        string.length = new_size;
    }
    "_ZN6swoole6String19append_random_bytesEmb", "_ZN6swoole6String19append_random_bytesEmb" =>
    pub extern "C" fn swr_string_append_random_bytes(this: &mut CppString, length: usize, base64: bool) -> bool {
        this.append_random_bytes(length, base64)
    }
    "_ZN6swoole6String5writeElRKS0_", "_ZN6swoole6String5writeExRKS0_" =>
    pub extern "C" fn swr_string_write_string(this: &mut CppString, offset: i64, source: &CppString) {
        // SAFETY: `source` holds `length` bytes.
        unsafe { this.write(offset, source.str, source.length) };
    }
    "_ZN6swoole6String5writeElPKcm", "_ZN6swoole6String5writeExPKcm" =>
    /// # Safety
    /// `data` holds `length` bytes.
    pub unsafe extern "C" fn swr_string_write(this: &mut CppString, offset: i64, data: *const u8, length: usize) {
        // SAFETY: as the caller promises.
        unsafe { this.write(offset, data, length) };
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

/// `String::split`, for the C++ method that lends its handler.
///
/// # Safety
/// As `CppString::split`, with `this` live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_string_split(
    this: *mut CppString,
    delimiter: *const u8,
    delimiter_length: usize,
    call: SplitCall,
    handler: *const c_void,
) -> isize {
    // SAFETY: as the caller promises.
    let string = unsafe { &mut *this };
    // SAFETY: as above.
    unsafe { string.split(delimiter, delimiter_length, call, handler) }
}

/// Fresh boxed storage for one String, which [`delete_string`] frees.
fn new_storage() -> &'static mut MaybeUninit<CppString> {
    Box::leak(Box::new_uninit())
}

/// `new String(data, length)`.
///
/// # Safety
/// `data` holds `length` readable bytes.
#[must_use]
pub unsafe fn new_string(data: *const u8, length: usize) -> *mut CppString {
    // SAFETY: the caller's bytes.
    unsafe { swr_string_from(new_storage(), data, length) }
}

/// `new String(size, with)`.
#[must_use]
pub fn new_sized_string(size: usize, with: Option<&'static Allocator>) -> *mut CppString {
    swr_string_new(new_storage(), size, with)
}

/// `delete string`.
///
/// # Safety
/// `string` came from [`new_string`] or [`new_sized_string`] and is not used again.
pub unsafe fn delete_string(string: *mut CppString) {
    // SAFETY: as the caller promises.
    unsafe { Box::from_raw(string) }.dispose();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new(text: &[u8]) -> CppString {
        let mut s = MaybeUninit::uninit();
        unsafe { swr_string_from(&mut s, text.as_ptr(), text.len()) };
        unsafe { s.assume_init() }
    }

    #[test]
    fn append_grows_and_reduce_moves() {
        let mut s = new(b"abc");
        assert_eq!((s.length, s.size), (3, 8));
        unsafe { swr_string_append(&mut s, b"defghijkl".as_ptr(), 9) };
        assert_eq!((s.view(), s.size), (&b"abcdefghijkl"[..], 16));
        swr_string_append_int(&mut s, -42);
        s.reduce(3);
        assert_eq!(s.view(), b"defghijkl-42");
        unsafe { swr_string_append_string(&raw mut s, &raw const s) };
        assert_eq!(s.view(), b"defghijkl-42defghijkl-42");
        s.set_null_terminated();
        assert_eq!(unsafe { *s.str.add(s.length) }, 0);
        s.dispose();
    }

    unsafe extern "C" fn collect(
        handler: *const c_void,
        data: *const c_char,
        length: usize,
    ) -> bool {
        let chunks = unsafe { &mut *handler.cast_mut().cast::<Vec<Vec<u8>>>() };
        chunks.push(unsafe { bytes(data.cast(), length) }.to_vec());
        true
    }

    #[test]
    fn split_hands_each_chunk_and_keeps_the_tail() {
        let mut s = new(b"a\r\nbc\r\nd\r");
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        let ret = unsafe { s.split(b"\r\n".as_ptr(), 2, collect, (&raw mut chunks).cast()) };
        assert_eq!(chunks, [b"a\r\n".to_vec(), b"bc\r\n".to_vec()]);
        assert_eq!((ret, s.offset), (7, 7));
        // No delimiter after the offset: it moves to the last place one could start.
        assert_eq!(
            unsafe { s.split(b"\r\n".as_ptr(), 2, collect, (&raw mut chunks).cast()) },
            0
        );
        assert_eq!((chunks.len(), s.offset), (2, 7));
        s.dispose();
    }

    /// A write's offset stays inside the buffer, and a disposed String is empty.
    #[test]
    fn write_offsets_stay_inside_the_buffer() {
        let mut s = new(b"abc");
        unsafe { s.write(5, b"z".as_ptr(), 1) };
        assert_eq!(s.view(), b"abc\0\0z");
        unsafe { s.write(-1, b"A".as_ptr(), 1) };
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
        unsafe { swr_string_append(&mut s, b"123".as_ptr(), 3) };
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
        assert!(unsafe { swr_string_contains(&copied, b"orl".as_ptr(), 3) });
        assert!(unsafe { swr_string_ends_with(&copied, b"ld".as_ptr(), 2) });
        unsafe { libc::free(moved.release().cast()) };
        copied.dispose();
        s.dispose();
    }

    #[test]
    fn new_and_delete_string() {
        let s = unsafe { new_string(b"abc".as_ptr(), 3) };
        assert_eq!(unsafe { &*s }.view(), b"abc");
        unsafe { delete_string(s) };
        let s = new_sized_string(16, None);
        assert_eq!(unsafe { ((*s).length, (*s).size) }, (0, 16));
        unsafe { delete_string(s) };
    }
}
