// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/buffer.cc and include/swoole_buffer.h
// (Swoole contributors).
//! `swoole::Buffer`: the C++ object (one Rust queue handle) and its member functions.
//! With `upstream-symbols` each is defined under its upstream (Itanium) symbol, and the
//! buffer patch deletes `src/core/buffer.cc`.
//!
//! A failed queue status aborts. `buffer.cc` threw `bad_alloc`, `length_error` or
//! `invalid_argument`, which no first-party code catches, so the process ended either way.
//! Nothing here calls C++: entry disposers are the Rust sendfile request's, so no C++
//! exception can unwind into these frames.
use crate::typed::{Detached, Front};
use crate::{
    EMPTY, OK, swr_v1_buffer_append, swr_v1_buffer_release, swr_v1_buffer_stats,
    swr_v1_buffer_typed_consume, swr_v1_buffer_typed_create, swr_v1_buffer_typed_front,
    swr_v1_buffer_typed_pop, swr_v1_buffer_typed_push,
};
use std::ptr;

const TYPE_CLOSE: u32 = 2;

/// `swoole::Buffer`: its only member is the queue handle.
#[repr(C)]
pub struct CppBuffer {
    queue: *mut crate::Buffer,
}

fn check(status: i32) {
    if status != OK {
        std::process::abort();
    }
}

/// # Safety
/// `this` is a live Buffer.
unsafe fn queue(this: *const CppBuffer) -> *mut crate::Buffer {
    // SAFETY: as the caller promises.
    unsafe { (*this).queue }
}

/// Pops the first entry and runs its disposer; false when the queue is empty.
/// # Safety
/// `this` is a live Buffer that only the caller uses.
unsafe fn pop(this: *mut CppBuffer) -> bool {
    let mut effect = Detached::default();
    // SAFETY: as the caller promises.
    let queue = unsafe { queue(this) };
    // SAFETY: as the caller promises; an aligned local output.
    let status = unsafe { swr_v1_buffer_typed_pop(queue, &raw mut effect) };
    if status == EMPTY {
        return false;
    }
    check(status);
    if let Some(dispose) = effect.dispose {
        // SAFETY: the detached token is ours now, handed to its own disposer once.
        unsafe { dispose(effect.token) };
    }
    true
}

/// # Safety
/// `this` is a live Buffer.
unsafe fn stats(this: *const CppBuffer) -> (u32, usize) {
    let (mut length, mut count) = (0, 0);
    // SAFETY: as the caller promises.
    let queue = unsafe { queue(this) };
    // SAFETY: as the caller promises; aligned local outputs.
    check(unsafe { swr_v1_buffer_stats(queue, &raw mut length, &raw mut count) });
    (length, count)
}

// Constructors and destructors return `this`: Apple's arm64 C++ ABI does, and elsewhere
// the caller ignores it. Each has a complete (C1/D1) and a base (C2/D2) symbol.
upstream! {
    "_ZN6swoole6BufferC1Ej", "_ZN6swoole6BufferC1Ej" =>
    /// `Buffer(uint32_t chunk_size)`.
    /// # Safety
    /// `this` is writable storage for a Buffer.
    pub unsafe extern "C" fn swr_cpp_buffer_new(this: *mut CppBuffer, chunk_size: u32) -> *mut CppBuffer {
        let mut queue = ptr::null_mut();
        // SAFETY: an aligned local output.
        check(unsafe { swr_v1_buffer_typed_create(chunk_size, &raw mut queue) });
        // SAFETY: as the caller promises.
        unsafe { this.write(CppBuffer { queue }) };
        this
    }
    "_ZN6swoole6BufferC2Ej", "_ZN6swoole6BufferC2Ej" =>
    /// # Safety
    /// As `swr_cpp_buffer_new`.
    pub unsafe extern "C" fn swr_cpp_buffer_new_base(this: *mut CppBuffer, chunk_size: u32) -> *mut CppBuffer {
        // SAFETY: as the caller promises.
        unsafe { swr_cpp_buffer_new(this, chunk_size) }
    }
    "_ZN6swoole6BufferD1Ev", "_ZN6swoole6BufferD1Ev" =>
    /// `~Buffer()`: pops every entry, then releases the queue.
    /// # Safety
    /// `this` is a live Buffer, not used again.
    pub unsafe extern "C" fn swr_cpp_buffer_drop(this: *mut CppBuffer) -> *mut CppBuffer {
        // SAFETY: as the caller promises.
        while unsafe { pop(this) } {}
        // SAFETY: as the caller promises.
        let queue = unsafe { queue(this) };
        // SAFETY: the drained handle, released once.
        check(unsafe { swr_v1_buffer_release(queue) });
        this
    }
    "_ZN6swoole6BufferD2Ev", "_ZN6swoole6BufferD2Ev" =>
    /// # Safety
    /// As `swr_cpp_buffer_drop`.
    pub unsafe extern "C" fn swr_cpp_buffer_drop_base(this: *mut CppBuffer) -> *mut CppBuffer {
        // SAFETY: as the caller promises.
        unsafe { swr_cpp_buffer_drop(this) }
    }
    "_ZNK6swoole6Buffer11typed_frontEv", "_ZNK6swoole6Buffer11typed_frontEv" =>
    /// `typed_front()`: the first entry; the queue must not be empty.
    /// # Safety
    /// `this` is a live Buffer.
    #[must_use]
    pub unsafe extern "C" fn swr_cpp_buffer_typed_front(this: *const CppBuffer) -> Front {
        let mut view = Front::default();
        // SAFETY: as the caller promises.
        let queue = unsafe { queue(this) };
        // SAFETY: as the caller promises; an aligned local output.
        check(unsafe { swr_v1_buffer_typed_front(queue, &raw mut view) });
        view
    }
    "_ZN6swoole6Buffer13typed_consumeERK32swr_v1_buffer_typed_front_resultj", "_ZN6swoole6Buffer13typed_consumeERK32swr_v1_buffer_typed_front_resultj" =>
    /// `typed_consume(front, count)`: commits `count` sent bytes of `front`'s entry.
    /// # Safety
    /// `this` is a live Buffer that only the caller uses; `front` is readable.
    pub unsafe extern "C" fn swr_cpp_buffer_typed_consume(this: *mut CppBuffer, front: *const Front, count: u32) {
        // SAFETY: as the caller promises.
        let front = unsafe { &*front };
        // SAFETY: as the caller promises.
        let queue = unsafe { queue(this) };
        // SAFETY: as the caller promises.
        check(unsafe { swr_v1_buffer_typed_consume(queue, front.generation, front.offset, count) });
    }
    "_ZN6swoole6Buffer11typed_closeEv", "_ZN6swoole6Buffer11typed_closeEv" =>
    /// `typed_close()`: queues a payload-free CLOSE marker.
    /// # Safety
    /// `this` is a live Buffer that only the caller uses.
    pub unsafe extern "C" fn swr_cpp_buffer_typed_close(this: *mut CppBuffer) {
        // SAFETY: as the caller promises.
        let queue = unsafe { queue(this) };
        // SAFETY: as the caller promises.
        check(unsafe { swr_v1_buffer_typed_push(queue, TYPE_CLOSE, ptr::null_mut(), None) });
    }
    "_ZN6swoole6Buffer3popEv", "_ZN6swoole6Buffer3popEv" =>
    /// `pop()`: drops the first entry, if any.
    /// # Safety
    /// `this` is a live Buffer that only the caller uses.
    pub unsafe extern "C" fn swr_cpp_buffer_pop(this: *mut CppBuffer) {
        // SAFETY: as the caller promises.
        unsafe { pop(this) };
    }
    "_ZN6swoole6Buffer6appendEPKcj", "_ZN6swoole6Buffer6appendEPKcj" =>
    /// `append(data, size)`.
    /// # Safety
    /// `this` is a live Buffer that only the caller uses; `data` holds `size` bytes.
    pub unsafe extern "C" fn swr_cpp_buffer_append(this: *mut CppBuffer, data: *const u8, size: u32) {
        // SAFETY: as the caller promises.
        let queue = unsafe { queue(this) };
        // SAFETY: as the caller promises.
        check(unsafe { swr_v1_buffer_append(queue, data, size as usize) });
    }
    "_ZN6swoole6Buffer6appendEPK5iovecml", "_ZN6swoole6Buffer6appendEPK5iovecmx" =>
    /// `append(iov, iovcnt, offset)`: the bytes past `offset`.
    /// # Safety
    /// `this` is a live Buffer that only the caller uses; `iov` holds `count` readable
    /// descriptors.
    pub unsafe extern "C" fn swr_cpp_buffer_append_iovec(
        this: *mut CppBuffer,
        iov: *const libc::iovec,
        count: usize,
        offset: libc::off_t,
    ) {
        // SAFETY: as the caller promises.
        let queue = unsafe { queue(this) };
        // SAFETY: as the caller promises.
        check(unsafe { crate::swr_v1_buffer_append_iovec(queue, iov, count, offset) });
    }
    "_ZNK6swoole6Buffer6lengthEv", "_ZNK6swoole6Buffer6lengthEv" =>
    /// `length()`: queued bytes.
    /// # Safety
    /// `this` is a live Buffer.
    #[must_use]
    pub unsafe extern "C" fn swr_cpp_buffer_length(this: *const CppBuffer) -> u32 {
        // SAFETY: as the caller promises.
        unsafe { stats(this) }.0
    }
    "_ZNK6swoole6Buffer5countEv", "_ZNK6swoole6Buffer5countEv" =>
    /// `count()`: queued entries.
    /// # Safety
    /// `this` is a live Buffer.
    #[must_use]
    pub unsafe extern "C" fn swr_cpp_buffer_count(this: *const CppBuffer) -> usize {
        // SAFETY: as the caller promises.
        unsafe { stats(this) }.1
    }
    "_ZNK6swoole6Buffer5emptyEv", "_ZNK6swoole6Buffer5emptyEv" =>
    /// `empty()`.
    /// # Safety
    /// `this` is a live Buffer.
    #[must_use]
    pub unsafe extern "C" fn swr_cpp_buffer_empty(this: *const CppBuffer) -> bool {
        // SAFETY: as the caller promises.
        unsafe { stats(this) }.1 == 0
    }
}

/// `new Buffer(chunk_size)`: operator new's storage, so C++ `delete` frees it.
#[must_use]
pub fn new_buffer(chunk_size: u32) -> *mut CppBuffer {
    unsafe extern "C" {
        #[link_name = "_Znwm"]
        fn operator_new(size: usize) -> *mut std::ffi::c_void;
    }
    // SAFETY: `new Buffer`'s storage. A failed operator new throws `bad_alloc`, which
    // aborts at this extern "C" frame, as every queue failure does.
    let storage = unsafe { operator_new(size_of::<CppBuffer>()) }.cast::<CppBuffer>();
    // SAFETY: fresh storage for one Buffer.
    unsafe { swr_cpp_buffer_new(storage, chunk_size) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::MaybeUninit;

    /// Runs `body` over a constructed Buffer, then destroys it.
    fn with_buffer(chunk_size: u32, body: impl FnOnce(*mut CppBuffer)) {
        let mut storage = MaybeUninit::<CppBuffer>::uninit();
        let this = storage.as_mut_ptr();
        unsafe { swr_cpp_buffer_new(this, chunk_size) };
        body(this);
        unsafe { swr_cpp_buffer_drop(this) };
    }

    /// Drains the queue as upstream's tests read `front()`: each entry's bytes past its
    /// cursor, then `pop()`.
    fn drain(this: *mut CppBuffer) -> Vec<u8> {
        let mut out = Vec::new();
        while !unsafe { swr_cpp_buffer_empty(this) } {
            let front = unsafe { swr_cpp_buffer_typed_front(this) };
            out.extend_from_slice(unsafe {
                std::slice::from_raw_parts(
                    front.data.add(front.offset as usize),
                    (front.length - front.offset) as usize,
                )
            });
            unsafe { swr_cpp_buffer_pop(this) };
        }
        out
    }

    /// `swoole_rand(min, max)`, inclusive, from a seeded xorshift.
    fn rand(state: &mut u64, min: usize, max: usize) -> usize {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        min + usize::try_from(*state % (max - min + 1) as u64).unwrap()
    }

    /// core-tests/src/memory/buffer.cpp `TEST(buffer, append_iov)`, 64 seeded runs.
    #[test]
    fn append_iov() {
        for seed in 1..=64u64 {
            let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            let lengths: Vec<usize> = (0..4).map(|_| rand(&mut state, 99, 4095)).collect();
            let total_len: usize = lengths.iter().sum();
            let parts: Vec<Vec<u8>> = lengths
                .iter()
                .zip(b"ABCD")
                .map(|(&n, &c)| vec![c; n])
                .collect();
            let v: Vec<libc::iovec> = parts
                .iter()
                .map(|p| libc::iovec {
                    iov_base: p.as_ptr().cast_mut().cast(),
                    iov_len: p.len(),
                })
                .collect();
            let mut offset = rand(&mut state, lengths[0] + 1, total_len - 1);
            with_buffer(1024, |buf| {
                unsafe { swr_cpp_buffer_append_iovec(buf, v.as_ptr(), 4, 0) };
                assert_eq!(unsafe { swr_cpp_buffer_length(buf) } as usize, total_len);
            });
            let at = i64::try_from(offset).unwrap();
            with_buffer(1024, |buf_for_offset| {
                unsafe {
                    swr_cpp_buffer_append_iovec(buf_for_offset, v.as_ptr(), 4, at);
                }
                assert_eq!(
                    unsafe { swr_cpp_buffer_length(buf_for_offset) } as usize,
                    total_len - offset
                );
                let str = drain(buf_for_offset);
                let mut indent = 0;
                for part in &parts {
                    if offset >= part.len() {
                        offset -= part.len();
                        continue;
                    }
                    assert_eq!(&str[indent..indent + part.len() - offset], &part[offset..]);
                    indent += part.len() - offset;
                    offset = 0;
                }
            });
        }
    }

    #[test]
    fn typed_cursor_close_and_teardown() {
        with_buffer(2, |this| unsafe {
            assert!(swr_cpp_buffer_empty(this));
            swr_cpp_buffer_append(this, b"abc".as_ptr(), 3);
            swr_cpp_buffer_typed_close(this);
            assert_eq!(
                (swr_cpp_buffer_length(this), swr_cpp_buffer_count(this)),
                (3, 3)
            );
            let view = swr_cpp_buffer_typed_front(this);
            assert_eq!((view.kind, view.length, view.offset), (0, 2, 0));
            swr_cpp_buffer_typed_consume(this, &raw const view, 1);
            assert_eq!(swr_cpp_buffer_typed_front(this).offset, 1);
            swr_cpp_buffer_pop(this);
            swr_cpp_buffer_pop(this);
            assert_eq!(swr_cpp_buffer_typed_front(this).kind, TYPE_CLOSE);
            // Teardown pops the queued CLOSE marker.
        });
    }
}
