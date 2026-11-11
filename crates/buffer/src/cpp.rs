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
    swr_v1_buffer_typed_pop,
};
use std::mem::MaybeUninit;
use std::ptr;

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

impl CppBuffer {
    /// Runs `body` over the queue. A `CppBuffer` reachable by reference was built by
    /// `swr_cpp_buffer_new` (its field is private) and not yet destroyed
    /// (`swr_cpp_buffer_drop` is unsafe and takes it out of use), so its queue is live.
    fn with<R>(&self, body: impl FnOnce(&mut crate::Buffer) -> R) -> R {
        // SAFETY: as above; nothing in `body` reaches this queue another way, and the
        // borrow ends with it.
        body(unsafe { &mut *self.queue })
    }

    /// Pops the first entry and runs its disposer; false when the queue is empty.
    fn pop(&self) -> bool {
        let mut effect = Detached::default();
        let status = self.with(|queue| swr_v1_buffer_typed_pop(Some(queue), Some(&mut effect)));
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

    fn stats(&self) -> (u32, usize) {
        let (mut length, mut count) = (0, 0);
        check(
            self.with(|queue| {
                swr_v1_buffer_stats(Some(queue), Some(&mut length), Some(&mut count))
            }),
        );
        (length, count)
    }
}

// Constructors and destructors return `this`: Apple's arm64 C++ ABI does, and elsewhere
// the caller ignores it. Only the complete-object (C1/D1) symbols: nothing derives from
// Buffer, so no caller names a base-object (C2/D2) one.
upstream! {
    "_ZN6swoole6BufferC1Ej", "_ZN6swoole6BufferC1Ej" =>
    /// `Buffer(uint32_t chunk_size)`, in uninitialized storage.
    pub extern "C" fn swr_cpp_buffer_new(this: &mut MaybeUninit<CppBuffer>, chunk_size: u32) -> *mut CppBuffer {
        let mut queue = ptr::null_mut();
        check(swr_v1_buffer_typed_create(chunk_size, Some(&mut queue)));
        this.write(CppBuffer { queue })
    }
    "_ZN6swoole6BufferD1Ev", "_ZN6swoole6BufferD1Ev" =>
    /// `~Buffer()`: pops every entry, then releases the queue.
    /// # Safety
    /// `this` is not used again.
    pub unsafe extern "C" fn swr_cpp_buffer_drop(this: &mut CppBuffer) -> *mut CppBuffer {
        while this.pop() {}
        // SAFETY: the drained handle, released once.
        check(unsafe { swr_v1_buffer_release(this.queue) });
        this
    }
    "_ZNK6swoole6Buffer11typed_frontEv", "_ZNK6swoole6Buffer11typed_frontEv" =>
    /// `typed_front()`: the first entry; the queue must not be empty.
    #[must_use]
    pub extern "C" fn swr_cpp_buffer_typed_front(this: &CppBuffer) -> Front {
        let mut view = Front::default();
        check(this.with(|queue| swr_v1_buffer_typed_front(Some(queue), Some(&mut view))));
        view
    }
    "_ZN6swoole6Buffer13typed_consumeERK32swr_v1_buffer_typed_front_resultj", "_ZN6swoole6Buffer13typed_consumeERK32swr_v1_buffer_typed_front_resultj" =>
    /// `typed_consume(front, count)`: commits `count` sent bytes of `front`'s entry.
    pub extern "C" fn swr_cpp_buffer_typed_consume(this: &CppBuffer, front: &Front, count: u32) {
        check(this.with(|queue| swr_v1_buffer_typed_consume(Some(queue), front.generation, front.offset, count)));
    }
    "_ZN6swoole6Buffer11typed_closeEv", "_ZN6swoole6Buffer11typed_closeEv" =>
    /// `typed_close()`: queues a payload-free CLOSE marker.
    pub extern "C" fn swr_cpp_buffer_typed_close(this: &CppBuffer) {
        check(this.with(crate::Buffer::push_close));
    }
    "_ZN6swoole6Buffer3popEv", "_ZN6swoole6Buffer3popEv" =>
    /// `pop()`: drops the first entry, if any.
    pub extern "C" fn swr_cpp_buffer_pop(this: &CppBuffer) {
        this.pop();
    }
    "_ZN6swoole6Buffer6appendEPKcj", "_ZN6swoole6Buffer6appendEPKcj" =>
    /// `append(data, size)`.
    /// # Safety
    /// `data` holds `size` bytes.
    pub unsafe extern "C" fn swr_cpp_buffer_append(this: &CppBuffer, data: *const u8, size: u32) {
        // SAFETY: as the caller promises.
        check(this.with(|queue| unsafe { swr_v1_buffer_append(Some(queue), data, size as usize) }));
    }
    "_ZN6swoole6Buffer6appendEPK5iovecml", "_ZN6swoole6Buffer6appendEPK5iovecmx" =>
    /// `append(iov, iovcnt, offset)`: the bytes past `offset`.
    /// # Safety
    /// `iov` holds `count` readable descriptors.
    pub unsafe extern "C" fn swr_cpp_buffer_append_iovec(
        this: &CppBuffer,
        iov: *const libc::iovec,
        count: usize,
        offset: libc::off_t,
    ) {
        // SAFETY: as the caller promises.
        check(this.with(|queue| unsafe { crate::swr_v1_buffer_append_iovec(Some(queue), iov, count, offset) }));
    }
    "_ZNK6swoole6Buffer6lengthEv", "_ZNK6swoole6Buffer6lengthEv" =>
    /// `length()`: queued bytes.
    #[must_use]
    pub extern "C" fn swr_cpp_buffer_length(this: &CppBuffer) -> u32 {
        this.stats().0
    }
    "_ZNK6swoole6Buffer5countEv", "_ZNK6swoole6Buffer5countEv" =>
    /// `count()`: queued entries.
    #[must_use]
    pub extern "C" fn swr_cpp_buffer_count(this: &CppBuffer) -> usize {
        this.stats().1
    }
    "_ZNK6swoole6Buffer5emptyEv", "_ZNK6swoole6Buffer5emptyEv" =>
    /// `empty()`.
    #[must_use]
    pub extern "C" fn swr_cpp_buffer_empty(this: &CppBuffer) -> bool {
        this.stats().1 == 0
    }
}

/// `new Buffer(chunk_size)`, boxed: [`delete_buffer`] frees it.
#[must_use]
pub fn new_buffer(chunk_size: u32) -> *mut CppBuffer {
    swr_cpp_buffer_new(Box::leak(Box::new_uninit()), chunk_size)
}

/// `delete buffer`.
///
/// # Safety
/// `buffer` came from [`new_buffer`] and is not used again.
pub unsafe fn delete_buffer(buffer: *mut CppBuffer) {
    // SAFETY: as the caller promises.
    let mut this = unsafe { Box::from_raw(buffer) };
    // SAFETY: as above; the box frees it next.
    unsafe { swr_cpp_buffer_drop(&mut this) };
}

#[cfg(test)]
mod tests {
    use super::*;

    const TYPE_CLOSE: u32 = 2;

    /// Runs `body` over a constructed Buffer, then destroys it.
    fn with_buffer(chunk_size: u32, body: impl FnOnce(&CppBuffer)) {
        let mut storage = MaybeUninit::<CppBuffer>::uninit();
        swr_cpp_buffer_new(&mut storage, chunk_size);
        let this = unsafe { storage.assume_init_mut() };
        body(this);
        unsafe { swr_cpp_buffer_drop(this) };
    }

    /// Drains the queue as upstream's tests read `front()`: each entry's bytes past its
    /// cursor, then `pop()`.
    fn drain(this: &CppBuffer) -> Vec<u8> {
        let mut out = Vec::new();
        while !swr_cpp_buffer_empty(this) {
            let front = swr_cpp_buffer_typed_front(this);
            out.extend_from_slice(unsafe {
                std::slice::from_raw_parts(
                    front.data.add(front.offset as usize),
                    (front.length - front.offset) as usize,
                )
            });
            swr_cpp_buffer_pop(this);
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
                assert_eq!(swr_cpp_buffer_length(buf) as usize, total_len);
            });
            let at = i64::try_from(offset).unwrap();
            with_buffer(1024, |buf_for_offset| {
                unsafe {
                    swr_cpp_buffer_append_iovec(buf_for_offset, v.as_ptr(), 4, at);
                }
                assert_eq!(
                    swr_cpp_buffer_length(buf_for_offset) as usize,
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
        with_buffer(2, |this| {
            assert!(swr_cpp_buffer_empty(this));
            unsafe { swr_cpp_buffer_append(this, b"abc".as_ptr(), 3) };
            swr_cpp_buffer_typed_close(this);
            assert_eq!(
                (swr_cpp_buffer_length(this), swr_cpp_buffer_count(this)),
                (3, 3)
            );
            let view = swr_cpp_buffer_typed_front(this);
            assert_eq!((view.kind, view.length, view.offset), (0, 2, 0));
            swr_cpp_buffer_typed_consume(this, &view, 1);
            assert_eq!(swr_cpp_buffer_typed_front(this).offset, 1);
            swr_cpp_buffer_pop(this);
            swr_cpp_buffer_pop(this);
            assert_eq!(swr_cpp_buffer_typed_front(this).kind, TYPE_CLOSE);
            // Teardown pops the queued CLOSE marker.
        });
    }

    #[test]
    fn new_and_delete_buffer() {
        let buffer = new_buffer(4);
        unsafe { swr_cpp_buffer_append(&*buffer, b"abcdef".as_ptr(), 6) };
        assert_eq!(swr_cpp_buffer_count(unsafe { &*buffer }), 2);
        // Deleting a non-empty Buffer pops its entries first.
        unsafe { delete_buffer(buffer) };
    }
}
