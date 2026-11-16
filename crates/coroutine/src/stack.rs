//! Coroutine stacks: allocation (heap, or a mapping with a guard page), the
//! usage markers and their exponential sampling. context.rs switches onto it.

use core::ffi::{c_int, c_void};
use core::mem::MaybeUninit;
use std::ptr::{self, NonNull};
use swoole_rs_corebase::sys;

const START: usize = 64 * 1024;
const MAGIC: &[u8] = b"swoole_coroutine#5652a7fb2b38be";
/// `SW_CONTEXT_PROTECT_STACK_PAGE`: map the stack and make its lowest page inaccessible.
pub const STACK_PROTECT: u32 = 1;
/// `SW_CONTEXT_DETECT_STACK_USAGE`: write the usage markers.
pub const STACK_MARK: u32 = 2;

fn offsets() -> impl Iterator<Item = usize> {
    std::iter::successors(Some(START), |offset| offset.checked_mul(2))
}

/// A coroutine's C stack, which it owns: `size` bytes at `at`, from malloc or (under
/// [`STACK_PROTECT`]) a mapping whose lowest page is inaccessible, freed when dropped.
/// A context without a stack of its own (a thread's) holds none: `at` is null. The
/// fields are private, so a non-null `at` is always what [`Stack::alloc`] made with
/// this `size` and these `flags`.
pub struct Stack {
    at: *mut u8,
    size: u32,
    /// The context's flags, of which the stack's are [`STACK_PROTECT`] and [`STACK_MARK`].
    flags: u32,
}

impl Stack {
    /// No stack, for a context of `flags`.
    #[must_use]
    pub const fn none(flags: u32) -> Self {
        Self {
            at: ptr::null_mut(),
            size: 0,
            flags,
        }
    }

    /// A `size`-byte stack, or none when out of memory.
    #[must_use]
    pub fn alloc(size: u32, flags: u32) -> Option<Self> {
        let bytes = size as usize;
        let stack = if flags & STACK_PROTECT == 0 {
            sys::malloc(bytes)
        } else {
            // Upstream adds MAP_STACK only on OpenBSD, where it is required.
            sys::map_anonymous(bytes, false, true)
        };
        if stack.is_null() {
            return None;
        }
        if flags & STACK_MARK != 0 {
            // SAFETY: `stack` is a fresh `bytes`-byte writable allocation, borrowed only here.
            mark(unsafe { std::slice::from_raw_parts_mut(stack.cast::<MaybeUninit<u8>>(), bytes) });
        }
        if flags & STACK_PROTECT != 0 {
            /// The mapping just made, which nothing references: only this block makes one.
            #[repr(transparent)]
            struct Fresh(*mut c_void);
            /// `PROT_NONE`, the one protection given.
            #[repr(transparent)]
            struct NoAccess(c_int);
            unsafe extern "C" {
                /// Makes the first `len` bytes of a fresh mapping, rounded up to its
                /// pages, inaccessible: memory nothing references, within the mapping
                /// when `len` is at most its size.
                safe fn mprotect(at: Fresh, len: usize, protection: NoAccess) -> c_int;
            }
            let page = usize::try_from(sys::sysconf(libc::_SC_PAGESIZE)).unwrap_or(4096);
            mprotect(Fresh(stack), page.min(bytes), NoAccess(libc::PROT_NONE));
        }
        Some(Self {
            at: stack.cast(),
            size,
            flags,
        })
    }

    /// The stack's lowest address; null without one.
    #[must_use]
    pub const fn base(&self) -> *mut u8 {
        self.at
    }

    /// Its size in bytes; 0 without one.
    #[must_use]
    pub const fn size(&self) -> usize {
        self.size as usize
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        /// The allocation a dropped stack held, as [`Stack::alloc`] made it: only this
        /// drop makes one, once per stack.
        #[repr(transparent)]
        struct Released(NonNull<u8>);
        #[expect(
            suspicious_runtime_symbol_definitions,
            reason = "`free`'s C signature; declared safe because it takes only a \
                      `Released`, the one malloc'd block a stack owned"
        )]
        unsafe extern "C" {
            safe fn free(block: Released);
            /// Unmaps a stack's mapping, given the size it was made with.
            safe fn munmap(mapping: Released, len: usize) -> c_int;
        }
        let Some(stack) = NonNull::new(self.at) else {
            return;
        };
        if self.flags & STACK_PROTECT == 0 {
            free(Released(stack));
        } else {
            munmap(Released(stack), self.size());
        }
    }
}

/// Writes the markers; bytes outside their spans stay as they were.
fn mark(stack: &mut [MaybeUninit<u8>]) {
    let size = stack.len();
    for offset in offsets().take_while(|&offset| offset <= size) {
        // `offset >= 64 KiB >= 2 * MAGIC.len()` and `offset <= size`: the span is inside.
        let span = stack.iter_mut().skip(size - offset + MAGIC.len());
        for (byte, &magic) in span.zip(MAGIC) {
            byte.write(magic);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Upstream's `Context::get_stack_usage`, which only its trace log read:
    /// twice the last sampled marker that changed, or 64 KiB.
    unsafe fn usage(stack: *const u8, size: usize) -> usize {
        let mut usage = START;
        for offset in offsets().take_while(|&offset| offset < size) {
            let index = size - offset + MAGIC.len();
            let marker =
                unsafe { std::slice::from_raw_parts(stack.wrapping_add(index), MAGIC.len()) };
            if marker != MAGIC {
                usage = offset * 2;
            }
        }
        usage
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no PROT_NONE guard pages")]
    fn allocated_stacks_are_marked_and_usable() {
        let size = 4 * START;
        for flags in [0, STACK_MARK, STACK_PROTECT | STACK_MARK] {
            let owned = Stack::alloc(u32::try_from(size).unwrap(), flags).unwrap();
            let stack = owned.base();
            assert!(!stack.is_null());
            // SAFETY: a live `size`-byte stack; its top page is writable either way.
            unsafe {
                stack.add(size - 1).write(1);
                if flags & STACK_MARK != 0 {
                    assert_eq!(usage(stack, size), START);
                }
            }
            drop(owned);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "every byte of 16 stacks is too slow to interpret")]
    fn marker_boundaries_and_last_changed_sample_preserve_original_accounting() {
        for size in [
            0,
            1,
            START - 1,
            START,
            START + 1,
            START * 2,
            START * 2 + 1,
            START * 16,
        ] {
            let mut memory = vec![0xa5; size];
            mark(unsafe { &mut *(ptr::from_mut::<[u8]>(&mut memory) as *mut [MaybeUninit<u8>]) });
            for (index, &byte) in memory.iter().enumerate() {
                let expected = offsets()
                    .take_while(|&offset| offset <= size)
                    .find_map(|offset| {
                        let start = size - offset + MAGIC.len();
                        (start..start + MAGIC.len())
                            .contains(&index)
                            .then(|| MAGIC[index - start])
                    })
                    .unwrap_or(0xa5);
                assert_eq!(byte, expected);
            }
            assert_eq!(unsafe { usage(memory.as_ptr(), size) }, START);
            let mut expected = START;
            for offset in offsets().take_while(|&offset| offset <= size) {
                memory[size - offset + MAGIC.len()] ^= 1;
                if offset < size {
                    expected = offset * 2;
                }
                assert_eq!(unsafe { usage(memory.as_ptr(), size) }, expected);
            }
        }
    }
}
