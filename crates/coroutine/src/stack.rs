//! Coroutine stacks: allocation (heap, or a mapping with a guard page), the
//! usage markers and their exponential sampling. context.rs switches onto it.

use core::ffi::c_void;
use std::ptr;

const START: usize = 64 * 1024;
const MAGIC: &[u8] = b"swoole_coroutine#5652a7fb2b38be";
/// `SW_CONTEXT_PROTECT_STACK_PAGE`: map the stack and make its lowest page inaccessible.
pub const STACK_PROTECT: u32 = 1;
/// `SW_CONTEXT_DETECT_STACK_USAGE`: write the usage markers.
pub const STACK_MARK: u32 = 2;

fn offsets() -> impl Iterator<Item = usize> {
    std::iter::successors(Some(START), |offset| offset.checked_mul(2))
}

/// A `size`-byte stack, or null when out of memory.
#[must_use]
pub fn swr_v1_context_stack_alloc(size: usize, flags: u32) -> *mut c_void {
    let stack = if flags & STACK_PROTECT == 0 {
        // SAFETY: malloc has no preconditions.
        unsafe { libc::malloc(size) }
    } else {
        // Upstream adds MAP_STACK only on OpenBSD, where it is required.
        #[cfg(target_os = "openbsd")]
        let stack_flag = libc::MAP_STACK;
        #[cfg(not(target_os = "openbsd"))]
        let stack_flag = 0;
        // SAFETY: a fresh anonymous private mapping aliases nothing.
        let mapped = unsafe {
            libc::mmap(
                ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON | stack_flag,
                -1,
                0,
            )
        };
        if mapped == libc::MAP_FAILED {
            ptr::null_mut()
        } else {
            mapped
        }
    };
    if stack.is_null() {
        return stack;
    }
    if flags & STACK_MARK != 0 {
        // SAFETY: `stack` is a fresh `size`-byte writable allocation.
        unsafe { mark(stack.cast(), size) };
    }
    if flags & STACK_PROTECT != 0 {
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        // SAFETY: the mapping starts at `stack`; protecting its first page only
        // removes access to memory nothing references yet.
        unsafe {
            libc::mprotect(
                stack,
                usize::try_from(page).unwrap_or(4096),
                libc::PROT_NONE,
            )
        };
    }
    stack
}

/// # Safety
/// `stack` came from [`swr_v1_context_stack_alloc`] with the same `size` and
/// `flags`, and is freed once.
pub unsafe fn swr_v1_context_stack_free(stack: *mut c_void, size: usize, flags: u32) {
    if flags & STACK_PROTECT == 0 {
        // SAFETY: a live malloc allocation, freed once.
        unsafe { libc::free(stack) };
    } else {
        // SAFETY: a live mapping of `size` bytes, unmapped once.
        unsafe { libc::munmap(stack, size) };
    }
}

/// # Safety
/// `stack` names `size` writable bytes without concurrent access. Bytes outside
/// the marker spans may be uninitialized.
unsafe fn mark(stack: *mut u8, size: usize) {
    for offset in offsets().take_while(|&offset| offset <= size) {
        let index = size - offset + MAGIC.len();
        // SAFETY: `offset >= 64 KiB >= 2 * MAGIC.len()` and `offset <= size`, so the span
        // `index..index + MAGIC.len()` lies inside the caller's `size` writable bytes.
        unsafe { ptr::copy_nonoverlapping(MAGIC.as_ptr(), stack.wrapping_add(index), MAGIC.len()) };
    }
}

/// # Safety
/// `stack` names `size` native-owned bytes without concurrent mutation. Each
/// sampled marker span is initialized, normally by `swr_v1_context_stack_alloc` before
/// native stack execution. No other stack bytes are read; no borrow is retained.
#[must_use]
pub unsafe fn swr_v1_context_stack_usage(stack: *const u8, size: usize) -> usize {
    // SAFETY: forwards the caller's `size` stack bytes with initialized marker spans.
    unsafe { usage(stack, size) }
}

unsafe fn usage(stack: *const u8, size: usize) -> usize {
    let mut usage = START;
    for offset in offsets().take_while(|&offset| offset < size) {
        let index = size - offset + MAGIC.len();
        // SAFETY: as in `mark`, this marker span lies inside `size` and is initialized.
        let marker = unsafe { std::slice::from_raw_parts(stack.wrapping_add(index), MAGIC.len()) };
        let matches = marker == MAGIC;
        if !matches {
            usage = offset * 2;
        }
    }
    usage
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no PROT_NONE guard pages")]
    fn allocated_stacks_are_marked_and_usable() {
        let size = 4 * START;
        for flags in [0, STACK_MARK, STACK_PROTECT | STACK_MARK] {
            let stack = swr_v1_context_stack_alloc(size, flags).cast::<u8>();
            assert!(!stack.is_null());
            // SAFETY: a live `size`-byte stack; its top page is writable either way.
            unsafe {
                stack.add(size - 1).write(1);
                if flags & STACK_MARK != 0 {
                    assert_eq!(usage(stack, size), START);
                }
                swr_v1_context_stack_free(stack.cast(), size, flags);
            }
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
            unsafe { mark(memory.as_mut_ptr(), size) };
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
            assert_eq!(
                unsafe { swr_v1_context_stack_usage(memory.as_ptr(), size) },
                START
            );
            let mut expected = START;
            for offset in offsets().take_while(|&offset| offset <= size) {
                memory[size - offset + MAGIC.len()] ^= 1;
                if offset < size {
                    expected = offset * 2;
                }
                assert_eq!(
                    unsafe { swr_v1_context_stack_usage(memory.as_ptr(), size) },
                    expected
                );
            }
        }
    }
}
