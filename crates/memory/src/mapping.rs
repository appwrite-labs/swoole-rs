//! Checked shared-mapping layouts; mmap/munmap/mprotect remain native syscalls.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_mapping_size(requested: usize, total: *mut usize) -> i32 {
    if total.is_null() {
        return -1;
    }
    // SAFETY: `total` is non-null (checked above), aligned and writable.
    unsafe {
        total.write(0);
    }
    let Some(size) = requested
        .checked_add(7)
        .map(|n| n & !7)
        .and_then(|n| n.checked_add(8))
        .filter(|&n| isize::try_from(n).is_ok())
    else {
        return -1;
    };
    // SAFETY: `total` is non-null (checked above), aligned and writable.
    unsafe {
        total.write(size);
    }
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_mapping_product(
    count: usize,
    width: usize,
    requested: *mut usize,
) -> i32 {
    if requested.is_null() {
        return -1;
    }
    // SAFETY: `requested` is non-null (checked above), aligned and writable.
    unsafe {
        requested.write(0);
    }
    let Some(size) = count.checked_mul(width) else {
        return -1;
    };
    let mut total = 0;
    // SAFETY: `total` is a writable local.
    if unsafe { swr_v1_memory_mapping_size(size, &raw mut total) } != 0 {
        return -1;
    }
    // SAFETY: `requested` is non-null (checked above), aligned and writable.
    unsafe {
        requested.write(size);
    }
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_mapping_copy(
    old_total: usize,
    new_total: usize,
    bytes: *mut usize,
) -> i32 {
    if bytes.is_null() {
        return -1;
    }
    // SAFETY: `bytes` is non-null (checked above), aligned and writable.
    unsafe {
        bytes.write(0);
    }
    let (Some(old), Some(new)) = (old_total.checked_sub(8), new_total.checked_sub(8)) else {
        return -1;
    };
    if old_total > isize::MAX as usize || new_total > isize::MAX as usize {
        return -1;
    }
    // SAFETY: `bytes` is non-null (checked above), aligned and writable.
    unsafe {
        bytes.write(old.min(new));
    }
    0
}

/// Borrowed existing mapping bytes; no native object or owner crosses Rust.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Extent {
    pub(crate) base: *mut std::ffi::c_void,
    pub(crate) total: usize,
}
unsafe fn extent(payload: *const u8) -> Option<Extent> {
    if payload.is_null() || payload.addr() < 8 || !(payload.addr() - 8).is_multiple_of(8) {
        return None;
    }
    // Caller supplies a live initialized mapping header immediately before payload.
    let base = payload.wrapping_sub(8);
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "the mapping header is checked 8-aligned above"
    )]
    // SAFETY: as above; the header is 8-aligned (checked above).
    let total = unsafe { base.cast::<usize>().read() };
    if total < 8
        || !total.is_multiple_of(8)
        || total > isize::MAX as usize
        || base.addr().checked_add(total).is_none()
    {
        return None;
    }
    Some(Extent {
        base: base.cast_mut().cast(),
        total,
    })
}
/// Initialize the actual eight-byte header after native mmap succeeds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_mapping_initialize(base: *mut u8, total: usize) -> *mut u8 {
    if base.is_null()
        || !base.addr().is_multiple_of(8)
        || total < 8
        || !total.is_multiple_of(8)
        || total > isize::MAX as usize
        || base.addr().checked_add(total).is_none()
    {
        return std::ptr::null_mut();
    }
    #[expect(clippy::cast_ptr_alignment, reason = "the mapping header is 8-aligned")]
    // SAFETY: non-null, 8-aligned and covering `total >= 8` bytes (checked above).
    unsafe {
        base.cast::<usize>().write(total);
    };
    base.wrapping_add(8)
}
/// Decode actual header storage for native unmap/protect, without retaining it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_mapping_extent(
    payload: *const u8,
    output: *mut Extent,
) -> i32 {
    if output.is_null() {
        return -1;
    }
    // SAFETY: `output` is non-null (checked above), aligned and writable.
    unsafe {
        output.write(Extent::default());
    }
    // SAFETY: the caller passes a live mapping payload (or null).
    let Some(value) = (unsafe { extent(payload) }) else {
        return -1;
    };
    // SAFETY: `output` is non-null (checked above), aligned and writable.
    unsafe {
        output.write(value);
    }
    0
}
/// Relocate bounded payload bytes between disjoint initialized live mappings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_memory_mapping_relocate(old: *const u8, new: *mut u8) -> i32 {
    // SAFETY: the caller passes live, initialized mapping payloads.
    let source = unsafe { extent(old) };
    // SAFETY: as above.
    let destination = unsafe { extent(new) };
    let (Some(source), Some(destination)) = (source, destination) else {
        return -1;
    };
    let source_end = source.base.addr() + source.total;
    let destination_end = destination.base.addr() + destination.total;
    if source.base.addr() < destination_end && destination.base.addr() < source_end {
        return -1;
    }
    // SAFETY: the mappings don't overlap (checked above), and each payload holds `total - 8`
    // bytes.
    unsafe { std::ptr::copy_nonoverlapping(old, new, source.total.min(destination.total) - 8) };
    0
}

#[cfg(test)]
#[path = "../../../tests/modules/memory/mapping.rs"]
#[expect(
    clippy::cast_possible_truncation,
    reason = "tests drive the native integer interfaces with literal and loop values"
)]
mod tests;
