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
use std::mem::offset_of;
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
#[repr(C)]
pub struct CppString {
    pub length: usize,
    pub size: usize,
    pub offset: i64,
    pub str: *mut u8,
    pub allocator: *const Allocator,
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

/// # Safety
/// `allocator` is live.
unsafe fn allocator<'a>(allocator: *const Allocator) -> &'a Allocator {
    // SAFETY: as the caller promises.
    unsafe { &*allocator }
}

/// The C++ side of `String::split`: calls the handler it lends.
pub type SplitCall = unsafe extern "C" fn(*const c_void, *const c_char, usize) -> bool;

impl CppString {
    pub const EMPTY: Self = Self {
        length: 0,
        size: 0,
        offset: 0,
        str: ptr::null_mut(),
        allocator: ptr::null(),
    };

    /// `String::alloc`: a fresh buffer, from the standard allocator when `with` is null.
    pub fn alloc(&mut self, size: usize, with: *const Allocator) {
        let with = if with.is_null() {
            &raw const STD_ALLOCATOR
        } else {
            with
        };
        let size = aligned(size);
        self.length = 0;
        self.size = size;
        self.offset = 0;
        // SAFETY: `with` is null (replaced above) or a live allocator, as every String's is.
        let malloc = unsafe { allocator(with) }.malloc;
        // SAFETY: an allocator's malloc.
        self.str = checked(unsafe { malloc(size) });
        self.allocator = with;
    }

    /// The destructor's body.
    pub fn dispose(&mut self) {
        if !self.allocator.is_null() && !self.str.is_null() {
            // SAFETY: a String's allocator is live.
            let free = unsafe { allocator(self.allocator) }.free;
            // SAFETY: `str` is this allocator's live allocation.
            unsafe { free(self.str.cast()) };
        }
    }

    /// `String::copy`.
    pub fn copy_from(&mut self, source: &Self) {
        self.alloc(source.size, source.allocator);
        // SAFETY: the new allocation holds `source.size >= source.length` bytes.
        unsafe { ptr::copy_nonoverlapping(source.str, self.str, source.length) };
        self.length = source.length;
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

    pub fn reserve(&mut self, new_size: usize) {
        if self.size == 0 {
            self.alloc(new_size, ptr::null());
            return;
        }
        let new_size = aligned(new_size);
        // SAFETY: a String with a buffer has a live allocator.
        let realloc = unsafe { allocator(self.allocator) }.realloc;
        // SAFETY: `str` is this allocator's live allocation.
        self.str = checked(unsafe { realloc(self.str.cast(), new_size) });
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

    /// `String::write(off_t, const char *, size_t)`.
    ///
    /// # Safety
    /// `data` is readable for `length` bytes.
    pub unsafe fn write(&mut self, offset: i64, data: *const u8, length: usize) {
        let new_length = unsigned(offset).wrapping_add(length);
        if new_length > self.size {
            self.reserve(page_aligned(new_length.wrapping_mul(2)));
        }
        // SAFETY: the buffer holds `new_length` bytes; `data` is readable.
        unsafe { ptr::copy(data, self.str.wrapping_add(unsigned(offset)), length) };
        if new_length > self.length {
            self.length = new_length;
        }
    }

    pub fn set_null_terminated(&mut self) {
        if self.length == self.size {
            self.extend(self.length.wrapping_add(1));
        }
        // SAFETY: `length < size`.
        unsafe { self.str.wrapping_add(self.length).write(0) };
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
    /// `delimiter` holds `delimiter_length > 0` bytes; `call` is safe with `handler`.
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
    pub fn grow(&mut self, increment: usize) {
        self.length = self.length.wrapping_add(increment);
        if self.length == self.size {
            self.reserve(self.size.wrapping_mul(2));
        }
    }

    /// The allocated bytes after the content, for a read to fill before `grow`.
    pub fn spare(&mut self) -> &mut [u8] {
        if self.str.is_null() {
            return &mut [];
        }
        // SAFETY: `str` holds `size >= length` bytes, borrowed with `self`.
        unsafe {
            std::slice::from_raw_parts_mut(
                self.str.wrapping_add(self.length),
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
            // SAFETY: the buffer holds `length + count` bytes.
            unsafe { ptr::write_bytes(self.str.wrapping_add(self.length), byte, count) };
            self.length = self.length.wrapping_add(count);
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
        let Some(raw) = self.spare().get_mut(..length) else {
            return false;
        };
        let mut n = swoole_rs_corebase::random_bytes(raw);
        if n != length {
            return false;
        }
        if base64 {
            let mut out = vec![0u8; encoded];
            n = swoole_rs_base64::encode(raw, &mut out);
            // The buffer's free space holds the encoding.
            let Some(at) = self.spare().get_mut(..n) else {
                return false;
            };
            at.copy_from_slice(out.get(..n).unwrap_or_default());
        }
        self.length = self.length.wrapping_add(n);
        true
    }

    pub fn pop(&mut self, init_size: usize) -> *mut u8 {
        debug_assert!(
            self.length >= unsigned(self.offset),
            "String::pop's offset is inside"
        );
        let value = self.str;
        let length = self.length.wrapping_sub(unsigned(self.offset));
        let size = aligned(if length == 0 {
            init_size
        } else {
            length.max(init_size)
        });
        // SAFETY: a String that pops has a live allocator.
        let malloc = unsafe { allocator(self.allocator) }.malloc;
        // SAFETY: an allocator's malloc.
        let new_value: *mut u8 = unsafe { malloc(size) }.cast();
        if new_value.is_null() {
            return ptr::null_mut();
        }
        self.str = new_value;
        self.size = size;
        self.length = length;
        if length > 0 {
            let from = value.wrapping_add(unsigned(self.offset));
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
        self.length = self.length.wrapping_sub(unsigned(offset));
        self.offset = 0;
        if self.length == 0 {
            return;
        }
        let from = self.str.wrapping_add(unsigned(offset));
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

/// # Safety
/// `this` is a live String that only the caller uses.
unsafe fn of<'a>(this: *mut CppString) -> &'a mut CppString {
    // SAFETY: as the caller promises.
    unsafe { &mut *this }
}

/// # Safety
/// `this` is a live String.
unsafe fn read<'a>(this: *const CppString) -> &'a CppString {
    // SAFETY: as the caller promises.
    unsafe { &*this }
}

// Constructors and destructors return `this`: Apple's arm64 C++ ABI does, and
// elsewhere the caller ignores it. Each has a complete (C1/D1) and base (C2/D2) symbol.
upstream! {
    "_ZN6swoole6StringC1EmPKNS_9AllocatorE", "_ZN6swoole6StringC1EmPKNS_9AllocatorE" =>
    /// `String(size_t, const Allocator *)`.
    /// # Safety
    /// `this` is writable; `with` is null or live.
    pub unsafe extern "C" fn swr_string_new(this: *mut CppString, size: usize, with: *const Allocator) -> *mut CppString {
        // SAFETY: as the caller promises.
        unsafe { of(this) }.alloc(size, with);
        this
    }
    "_ZN6swoole6StringC2EmPKNS_9AllocatorE", "_ZN6swoole6StringC2EmPKNS_9AllocatorE" =>
    /// # Safety
    /// As `swr_string_new`.
    pub unsafe extern "C" fn swr_string_new_base(this: *mut CppString, size: usize, with: *const Allocator) -> *mut CppString {
        // SAFETY: as the caller promises.
        unsafe { swr_string_new(this, size, with) }
    }
    "_ZN6swoole6StringC1EPKcm", "_ZN6swoole6StringC1EPKcm" =>
    /// `String(const char *, size_t)`: a NUL-terminated copy.
    /// # Safety
    /// `this` is writable; `data` holds `length` bytes.
    pub unsafe extern "C" fn swr_string_from(this: *mut CppString, data: *const u8, length: usize) -> *mut CppString {
        // SAFETY: as the caller promises.
        let string = unsafe { of(this) };
        string.alloc(length.wrapping_add(1), ptr::null());
        // SAFETY: the allocation holds `length + 1` bytes; `data` is readable.
        unsafe { ptr::copy_nonoverlapping(data, string.str, length) };
        // SAFETY: as above.
        unsafe { string.str.wrapping_add(length).write(0) };
        string.length = length;
        this
    }
    "_ZN6swoole6StringC2EPKcm", "_ZN6swoole6StringC2EPKcm" =>
    /// # Safety
    /// As `swr_string_from`.
    pub unsafe extern "C" fn swr_string_from_base(this: *mut CppString, data: *const u8, length: usize) -> *mut CppString {
        // SAFETY: as the caller promises.
        unsafe { swr_string_from(this, data, length) }
    }
    "_ZN6swoole6StringC1ERKS0_", "_ZN6swoole6StringC1ERKS0_" =>
    /// `String(const String &)`.
    /// # Safety
    /// `this` is writable; `source` is live.
    pub unsafe extern "C" fn swr_string_copy(this: *mut CppString, source: *const CppString) -> *mut CppString {
        // SAFETY: as the caller promises.
        let source = unsafe { read(source) };
        // SAFETY: as above.
        unsafe { of(this) }.copy_from(source);
        this
    }
    "_ZN6swoole6StringC2ERKS0_", "_ZN6swoole6StringC2ERKS0_" =>
    /// # Safety
    /// As `swr_string_copy`.
    pub unsafe extern "C" fn swr_string_copy_base(this: *mut CppString, source: *const CppString) -> *mut CppString {
        // SAFETY: as the caller promises.
        unsafe { swr_string_copy(this, source) }
    }
    "_ZN6swoole6StringC1EOS0_", "_ZN6swoole6StringC1EOS0_" =>
    /// `String(String &&)`.
    /// # Safety
    /// `this` is writable; `source` is another live String.
    pub unsafe extern "C" fn swr_string_move(this: *mut CppString, source: *mut CppString) -> *mut CppString {
        // SAFETY: as the caller promises.
        let source = unsafe { of(source) };
        // SAFETY: as above.
        unsafe { of(this) }.move_from(source);
        this
    }
    "_ZN6swoole6StringC2EOS0_", "_ZN6swoole6StringC2EOS0_" =>
    /// # Safety
    /// As `swr_string_move`.
    pub unsafe extern "C" fn swr_string_move_base(this: *mut CppString, source: *mut CppString) -> *mut CppString {
        // SAFETY: as the caller promises.
        unsafe { swr_string_move(this, source) }
    }
    "_ZN6swoole6StringD1Ev", "_ZN6swoole6StringD1Ev" =>
    /// `~String()`.
    /// # Safety
    /// `this` is live.
    pub unsafe extern "C" fn swr_string_drop(this: *mut CppString) -> *mut CppString {
        // SAFETY: as the caller promises.
        unsafe { of(this) }.dispose();
        this
    }
    "_ZN6swoole6StringD2Ev", "_ZN6swoole6StringD2Ev" =>
    /// # Safety
    /// As `swr_string_drop`.
    pub unsafe extern "C" fn swr_string_drop_base(this: *mut CppString) -> *mut CppString {
        // SAFETY: as the caller promises.
        unsafe { swr_string_drop(this) }
    }
    "_ZN6swoole6StringaSERKS0_", "_ZN6swoole6StringaSERKS0_" =>
    /// `String &operator=(const String &)`.
    /// # Safety
    /// Both are live.
    pub unsafe extern "C" fn swr_string_assign_copy(this: *mut CppString, source: *const CppString) -> *mut CppString {
        if !ptr::eq(this, source) {
            // SAFETY: as the caller promises, and they differ.
            let string = unsafe { of(this) };
            string.dispose();
            // SAFETY: as above.
            string.copy_from(unsafe { read(source) });
        }
        this
    }
    "_ZN6swoole6StringaSEOS0_", "_ZN6swoole6StringaSEOS0_" =>
    /// `String &operator=(String &&)`.
    /// # Safety
    /// Both are live.
    pub unsafe extern "C" fn swr_string_assign_move(this: *mut CppString, source: *mut CppString) -> *mut CppString {
        if !ptr::eq(this, source) {
            // SAFETY: as the caller promises, and they differ.
            let string = unsafe { of(this) };
            string.dispose();
            // SAFETY: as above.
            string.move_from(unsafe { of(source) });
        }
        this
    }
    "_ZNK6swoole6String8containsEPKcm", "_ZNK6swoole6String8containsEPKcm" =>
    /// # Safety
    /// `this` is live; `needle` holds `length > 0` bytes.
    #[must_use]
    pub unsafe extern "C" fn swr_string_contains(this: *const CppString, needle: *const u8, length: usize) -> bool {
        // SAFETY: as the caller promises.
        let string = unsafe { read(this) };
        // SAFETY: as above.
        unsafe { strnstr(string.str, string.length, needle, length) }.is_some()
    }
    "_ZNK6swoole6String11starts_withEPKcm", "_ZNK6swoole6String11starts_withEPKcm" =>
    /// # Safety
    /// `this` is live; `needle` holds `length` bytes.
    #[must_use]
    pub unsafe extern "C" fn swr_string_starts_with(this: *const CppString, needle: *const u8, length: usize) -> bool {
        // SAFETY: as the caller promises.
        let needle = unsafe { bytes(needle, length) };
        // SAFETY: as above.
        unsafe { read(this) }.view().starts_with(needle)
    }
    "_ZNK6swoole6String9ends_withEPKcm", "_ZNK6swoole6String9ends_withEPKcm" =>
    /// # Safety
    /// `this` is live; `needle` holds `length` bytes.
    #[must_use]
    pub unsafe extern "C" fn swr_string_ends_with(this: *const CppString, needle: *const u8, length: usize) -> bool {
        // SAFETY: as the caller promises.
        let needle = unsafe { bytes(needle, length) };
        // SAFETY: as above.
        unsafe { read(this) }.view().ends_with(needle)
    }
    "_ZNK6swoole6String6equalsEPKcm", "_ZNK6swoole6String6equalsEPKcm" =>
    /// # Safety
    /// `this` is live; `data` holds `length` bytes.
    #[must_use]
    pub unsafe extern "C" fn swr_string_equals(this: *const CppString, data: *const u8, length: usize) -> bool {
        // SAFETY: as the caller promises.
        let data = unsafe { bytes(data, length) };
        // SAFETY: as above.
        unsafe { read(this) }.view() == data
    }
    "_ZN6swoole6String4growEm", "_ZN6swoole6String4growEm" =>
    /// # Safety
    /// `this` is live.
    pub unsafe extern "C" fn swr_string_grow(this: *mut CppString, increment: usize) {
        // SAFETY: as the caller promises.
        unsafe { of(this) }.grow(increment);
    }
    "_ZNK6swoole6String6substrEmm", "_ZNK6swoole6String6substrEmm" =>
    /// `String substr(size_t, size_t) const`. The C++ ABI returns a class with a
    /// destructor in caller memory (`sret`), as C does a 40-byte struct.
    /// # Safety
    /// `this` is live.
    #[must_use]
    pub unsafe extern "C" fn swr_string_substr(this: *const CppString, offset: usize, length: usize) -> CppString {
        // SAFETY: as the caller promises.
        let string = unsafe { read(this) };
        let mut result = CppString::EMPTY;
        if offset.wrapping_add(length) <= string.length {
            result.alloc(length, ptr::null());
            // SAFETY: the range lies inside `str`.
            unsafe { result.append(string.str.wrapping_add(offset), length) };
        }
        result
    }
    "_ZN6swoole6String6extendEv", "_ZN6swoole6String6extendEv" =>
    /// # Safety
    /// `this` is live.
    pub unsafe extern "C" fn swr_string_extend_double(this: *mut CppString) {
        // SAFETY: as the caller promises.
        let string = unsafe { of(this) };
        string.extend(string.size.wrapping_mul(2));
    }
    "_ZN6swoole6String6extendEm", "_ZN6swoole6String6extendEm" =>
    /// # Safety
    /// `this` is live.
    pub unsafe extern "C" fn swr_string_extend(this: *mut CppString, new_size: usize) {
        // SAFETY: as the caller promises.
        unsafe { of(this) }.extend(new_size);
    }
    "_ZN6swoole6String7reserveEm", "_ZN6swoole6String7reserveEm" =>
    /// # Safety
    /// `this` is live.
    pub unsafe extern "C" fn swr_string_reserve(this: *mut CppString, new_size: usize) {
        // SAFETY: as the caller promises.
        unsafe { of(this) }.reserve(new_size);
    }
    "_ZN6swoole6String7releaseEv", "_ZN6swoole6String7releaseEv" =>
    /// # Safety
    /// `this` is live.
    pub unsafe extern "C" fn swr_string_release(this: *mut CppString) -> *mut u8 {
        // SAFETY: as the caller promises.
        unsafe { of(this) }.release()
    }
    "_ZN6swoole6String6repeatEPKcmm", "_ZN6swoole6String6repeatEPKcmm" =>
    /// # Safety
    /// `this` is live; `data` holds `length` bytes.
    pub unsafe extern "C" fn swr_string_repeat(this: *mut CppString, data: *const u8, length: usize, count: usize) {
        // SAFETY: as the caller promises.
        let string = unsafe { of(this) };
        // SAFETY: as above.
        unsafe { string.repeat(data, length, count) };
    }
    "_ZN6swoole6String6appendEPKcm", "_ZN6swoole6String6appendEPKcm" =>
    /// # Safety
    /// `this` is live; `data` holds `length` bytes.
    pub unsafe extern "C" fn swr_string_append(this: *mut CppString, data: *const u8, length: usize) {
        // SAFETY: as the caller promises.
        let string = unsafe { of(this) };
        // SAFETY: as above.
        unsafe { string.append(data, length) };
    }
    "_ZN6swoole6String6appendEi", "_ZN6swoole6String6appendEi" =>
    /// `append(int)`: its decimal text.
    /// # Safety
    /// `this` is live.
    pub unsafe extern "C" fn swr_string_append_int(this: *mut CppString, value: c_int) {
        let mut buffer = itoa::Buffer::new();
        let text = buffer.format(value);
        // SAFETY: as the caller promises.
        let string = unsafe { of(this) };
        // SAFETY: `text` is local.
        unsafe { string.append(text.as_ptr(), text.len()) };
    }
    "_ZN6swoole6String6appendERKS0_", "_ZN6swoole6String6appendERKS0_" =>
    /// `append(const String &)`, which may be `this`.
    /// # Safety
    /// Both are live.
    pub unsafe extern "C" fn swr_string_append_string(this: *mut CppString, source: *const CppString) {
        // SAFETY: as the caller promises.
        let length = unsafe { read(source) }.length;
        // SAFETY: as above; `source` is read again only through its pointer.
        let string = unsafe { of(this) };
        let new_size = string.length.wrapping_add(length);
        if new_size > string.size {
            string.reserve(new_size);
        }
        // SAFETY: read after any reallocation, which `source` may share.
        let data = unsafe { read(source) }.str;
        // SAFETY: the buffer holds `new_size` bytes; `source` holds `length`.
        unsafe { ptr::copy(data, string.str.wrapping_add(string.length), length) };
        string.length = new_size;
    }
    "_ZN6swoole6String19append_random_bytesEmb", "_ZN6swoole6String19append_random_bytesEmb" =>
    /// # Safety
    /// `this` is live.
    pub unsafe extern "C" fn swr_string_append_random_bytes(this: *mut CppString, length: usize, base64: bool) -> bool {
        // SAFETY: as the caller promises.
        unsafe { of(this) }.append_random_bytes(length, base64)
    }
    "_ZN6swoole6String5writeElRKS0_", "_ZN6swoole6String5writeExRKS0_" =>
    /// # Safety
    /// Both are live.
    pub unsafe extern "C" fn swr_string_write_string(this: *mut CppString, offset: i64, source: *const CppString) {
        // SAFETY: as the caller promises.
        let source = unsafe { read(source) };
        // SAFETY: as above.
        let string = unsafe { of(this) };
        // SAFETY: `source` holds `length` bytes.
        unsafe { string.write(offset, source.str, source.length) };
    }
    "_ZN6swoole6String5writeElPKcm", "_ZN6swoole6String5writeExPKcm" =>
    /// # Safety
    /// `this` is live; `data` holds `length` bytes.
    pub unsafe extern "C" fn swr_string_write(this: *mut CppString, offset: i64, data: *const u8, length: usize) {
        // SAFETY: as the caller promises.
        let string = unsafe { of(this) };
        // SAFETY: as above.
        unsafe { string.write(offset, data, length) };
    }
    "_ZN6swoole6String19set_null_terminatedEv", "_ZN6swoole6String19set_null_terminatedEv" =>
    /// # Safety
    /// `this` is live.
    pub unsafe extern "C" fn swr_string_set_null_terminated(this: *mut CppString) {
        // SAFETY: as the caller promises.
        unsafe { of(this) }.set_null_terminated();
    }
    "_ZNK6swoole6String5printEb", "_ZNK6swoole6String5printEb" =>
    /// `print(bool)`, through C's stdout as upstream.
    /// # Safety
    /// `this` is live.
    pub unsafe extern "C" fn swr_string_print(this: *const CppString, value: bool) {
        // SAFETY: as the caller promises.
        unsafe { read(this) }.print(value);
    }
    "_ZN6swoole6String3popEm", "_ZN6swoole6String3popEm" =>
    /// `pop(size_t)`: hands over the buffer and keeps the bytes after `offset` in a new one.
    /// # Safety
    /// `this` is live with an allocator.
    pub unsafe extern "C" fn swr_string_pop(this: *mut CppString, init_size: usize) -> *mut u8 {
        // SAFETY: as the caller promises.
        unsafe { of(this) }.pop(init_size)
    }
    "_ZN6swoole6String6reduceEl", "_ZN6swoole6String6reduceEx" =>
    /// `reduce(off_t)`: moves the bytes after `offset` to the head.
    /// # Safety
    /// `this` is live.
    pub unsafe extern "C" fn swr_string_reduce(this: *mut CppString, offset: i64) {
        // SAFETY: as the caller promises.
        unsafe { of(this) }.reduce(offset);
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
    let string = unsafe { of(this) };
    // SAFETY: as above.
    unsafe { string.split(delimiter, delimiter_length, call, handler) }
}

// SAFETY: the C++ runtime's allocation functions.
unsafe extern "C" {
    #[link_name = "_Znwm"]
    fn operator_new(size: usize) -> *mut c_void;
    #[link_name = "_ZdlPv"]
    fn operator_delete(storage: *mut c_void);
}

/// `new String(data, length)`: operator new's storage, so C++ `delete` frees it.
///
/// # Safety
/// `data` holds `length` readable bytes.
#[must_use]
pub unsafe fn new_string(data: *const u8, length: usize) -> *mut CppString {
    // SAFETY: `new String`'s storage; a failed operator new aborts at this frame.
    let storage = unsafe { operator_new(size_of::<CppString>()) }.cast::<CppString>();
    // SAFETY: fresh storage for one String, and the caller's bytes.
    unsafe { swr_string_from(storage, data, length) }
}

/// `new String(size, with)`: operator new's storage, so C++ `delete` frees it.
///
/// # Safety
/// `with` is null or a live allocator.
#[must_use]
pub unsafe fn new_sized_string(size: usize, with: *const Allocator) -> *mut CppString {
    // SAFETY: `new String`'s storage; a failed operator new aborts at this frame.
    let storage = unsafe { operator_new(size_of::<CppString>()) }.cast::<CppString>();
    // SAFETY: fresh storage for one String, and the caller's allocator.
    unsafe { swr_string_new(storage, size, with) }
}

/// `delete string`.
///
/// # Safety
/// `string` came from `new String` (C++ or [`new_string`]) and is not used again.
pub unsafe fn delete_string(string: *mut CppString) {
    // SAFETY: as the caller promises.
    unsafe { of(string) }.dispose();
    // SAFETY: operator new's storage, destroyed above.
    unsafe { operator_delete(string.cast()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new(text: &[u8]) -> CppString {
        let mut s = CppString::EMPTY;
        unsafe { swr_string_from(&raw mut s, text.as_ptr(), text.len()) };
        s
    }

    #[test]
    fn append_grows_and_reduce_moves() {
        let mut s = new(b"abc");
        assert_eq!((s.length, s.size), (3, 8));
        unsafe { swr_string_append(&raw mut s, b"defghijkl".as_ptr(), 9) };
        assert_eq!((s.view(), s.size), (&b"abcdefghijkl"[..], 16));
        unsafe { swr_string_append_int(&raw mut s, -42) };
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

    #[test]
    fn substr_copy_move_and_release() {
        let mut s = new(b"hello world");
        let mut sub = unsafe { swr_string_substr(&raw const s, 6, 5) };
        assert_eq!(sub.view(), b"world");
        assert!(
            unsafe { swr_string_substr(&raw const s, 6, 6) }
                .str
                .is_null()
        );
        let mut moved = CppString::EMPTY;
        moved.move_from(&mut sub);
        assert!(sub.str.is_null() && moved.view() == b"world");
        let mut copied = CppString::EMPTY;
        copied.copy_from(&moved);
        assert!(unsafe { swr_string_contains(&raw const copied, b"orl".as_ptr(), 3) });
        assert!(unsafe { swr_string_ends_with(&raw const copied, b"ld".as_ptr(), 2) });
        unsafe { libc::free(moved.release().cast()) };
        copied.dispose();
        s.dispose();
    }
}
