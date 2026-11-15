// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/buffer.cc and include/swoole_buffer.h
// (Swoole contributors).
//! `swoole::Buffer`: the socket record's output queue (one Rust queue handle) and the
//! member functions Rust calls on it. No C++ calls them, so none is exported.
//!
//! A failed queue status aborts. `buffer.cc` threw `bad_alloc`, `length_error` or
//! `invalid_argument`, which no first-party code catches, so the process ended either way.
//! Nothing here calls C++: entry disposers are the Rust sendfile request's, so no C++
//! exception can unwind into these frames.
use crate::typed::Front;

/// `swoole::Buffer`: its only member is the queue handle, a boxed [`crate::Buffer`]
/// held as C++ sees it (an opaque pointer, so a record that embeds a `Buffer *` stays
/// a C type).
#[repr(C)]
pub struct CppBuffer {
    queue: *mut std::ffi::c_void,
}

fn check(status: Result<(), crate::Error>) {
    if status.is_err() {
        swoole_rs_corebase::fatal("a buffer queue operation failed");
    }
}

impl CppBuffer {
    /// `Buffer(chunk_size)`.
    fn new(chunk_size: u32) -> Self {
        let Ok(queue) = crate::Buffer::create(chunk_size) else {
            swoole_rs_corebase::fatal("a buffer queue can't be created");
        };
        Self {
            queue: Box::into_raw(queue).cast(),
        }
    }

    /// The queue's address, for a caller that reaches it between the borrows of the
    /// record that owns this `Buffer` (crates/tls's send path).
    #[must_use]
    pub const fn queue(&self) -> *mut crate::Buffer {
        self.queue.cast()
    }

    /// Runs `body` over the queue. A `CppBuffer` reachable by reference was built by
    /// [`Self::new`] (its field is private) and not yet dropped, so its queue is live.
    fn with<R>(&self, body: impl FnOnce(&mut crate::Buffer) -> R) -> R {
        // SAFETY: as above; nothing in `body` reaches this queue another way, and the
        // borrow ends with it.
        body(unsafe { &mut *self.queue() })
    }

    /// Pops the first entry and runs its disposer; false when the queue is empty.
    fn pop(&self) -> bool {
        self.with(crate::Buffer::pop)
            .map(crate::TypedDetached::dispose)
            .is_ok()
    }

    fn stats(&self) -> (u32, usize) {
        self.with(|queue| queue.stats())
    }

    /// `append(data, size)` of a slice.
    #[inline]
    pub fn append(&self, data: &[u8]) {
        check(self.with(|queue| queue.append(data)));
    }

    /// `append(iov, iovcnt, offset)` of slices: the bytes of `parts` past `offset`.
    pub fn append_from(&self, parts: &[impl std::ops::Deref<Target = [u8]>], offset: u64) {
        check(self.with(|queue| queue.appendv(parts.iter().map(|part| &**part), offset)));
    }
}

/// `~Buffer()`: pops every entry, then releases the queue.
impl Drop for CppBuffer {
    fn drop(&mut self) {
        while self.pop() {}
        // SAFETY: the handle `new` boxed, released once: a dropped value is not used again.
        if unsafe { Box::from_raw(self.queue()) }.release().is_err() {
            swoole_rs_corebase::fatal("a buffer queue still in use when dropped");
        }
    }
}

/// `typed_front()`: the first entry; the queue must not be empty.
#[must_use]
pub fn swr_cpp_buffer_typed_front(this: &CppBuffer) -> Front {
    this.with(|queue| queue.front())
        .unwrap_or_else(|_| swoole_rs_corebase::fatal("typed_front of an empty buffer"))
}

/// `typed_consume(front, count)`: commits `count` sent bytes of `front`'s entry.
pub fn swr_cpp_buffer_typed_consume(this: &CppBuffer, front: &Front, count: u32) {
    check(this.with(|queue| queue.consume(front.generation, front.offset, count)));
}

/// `typed_close()`: queues a payload-free CLOSE marker.
pub fn swr_cpp_buffer_typed_close(this: &CppBuffer) {
    check(this.with(crate::Buffer::push_close));
}

/// `pop()`: drops the first entry, if any.
pub fn swr_cpp_buffer_pop(this: &CppBuffer) {
    this.pop();
}

/// `length()`: queued bytes.
#[must_use]
pub fn swr_cpp_buffer_length(this: &CppBuffer) -> u32 {
    this.stats().0
}

/// `count()`: queued entries.
#[must_use]
pub fn swr_cpp_buffer_count(this: &CppBuffer) -> usize {
    this.stats().1
}

/// `empty()`.
#[must_use]
pub fn swr_cpp_buffer_empty(this: &CppBuffer) -> bool {
    this.stats().1 == 0
}

/// `new Buffer(chunk_size)`; dropping the box is `delete buffer`.
#[must_use]
pub fn new_buffer(chunk_size: u32) -> Box<CppBuffer> {
    Box::new(CppBuffer::new(chunk_size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EntryKind;

    /// Runs `body` over a constructed Buffer, then destroys it.
    fn with_buffer(chunk_size: u32, body: impl FnOnce(&CppBuffer)) {
        body(&new_buffer(chunk_size));
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
            let mut offset = rand(&mut state, lengths[0] + 1, total_len - 1);
            with_buffer(1024, |buf| {
                buf.append_from(&parts, 0);
                assert_eq!(swr_cpp_buffer_length(buf) as usize, total_len);
            });
            with_buffer(1024, |buf_for_offset| {
                buf_for_offset.append_from(&parts, offset as u64);
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

    /// Ported from the `tests/modules/buffer/iovec.cc` fixture: twelve positive offset-0
    /// vectors of three-byte binary parts, appended whole and drained entry by entry.
    #[test]
    fn positive_iovec_vectors() {
        // POSIX `struct iovec`: a pointer, then a `size_t`.
        let word = size_of::<usize>();
        assert_eq!(
            (
                size_of::<libc::iovec>(),
                align_of::<libc::iovec>(),
                std::mem::offset_of!(libc::iovec, iov_base),
                std::mem::offset_of!(libc::iovec, iov_len),
            ),
            (2 * word, word, 0, word)
        );
        let mut cases = 0;
        for count in [1u8, 8, 9, 32] {
            let bytes: Vec<[u8; 3]> = (0..count).map(|i| [0, i, 255]).collect();
            let parts: Vec<&[u8]> = bytes.iter().map(|part| &part[..]).collect();
            let expected = bytes.concat();
            for chunk in [0u32, 1, 4] {
                with_buffer(chunk, |buffer| {
                    buffer.append_from(&parts, 0);
                    assert_eq!(swr_cpp_buffer_length(buffer) as usize, expected.len());
                    let effective = if chunk == 0 {
                        i32::MAX as usize
                    } else {
                        chunk as usize
                    };
                    assert_eq!(
                        swr_cpp_buffer_count(buffer),
                        expected.len().div_ceil(effective)
                    );
                    let mut copied = Vec::new();
                    while !swr_cpp_buffer_empty(buffer) {
                        let front = swr_cpp_buffer_typed_front(buffer);
                        assert_eq!(front.offset, 0);
                        copied.extend_from_slice(unsafe {
                            std::slice::from_raw_parts(front.data, front.length as usize)
                        });
                        swr_cpp_buffer_pop(buffer);
                        assert_eq!(
                            swr_cpp_buffer_length(buffer) as usize,
                            expected.len() - copied.len()
                        );
                    }
                    assert_eq!(copied, expected);
                    assert_eq!(swr_cpp_buffer_count(buffer), 0);
                });
                cases += 1;
            }
        }
        assert_eq!(cases, 12);
    }

    #[test]
    fn typed_cursor_close_and_teardown() {
        with_buffer(2, |this| {
            assert!(swr_cpp_buffer_empty(this));
            this.append(b"abc");
            swr_cpp_buffer_typed_close(this);
            assert_eq!(
                (swr_cpp_buffer_length(this), swr_cpp_buffer_count(this)),
                (3, 3)
            );
            let view = swr_cpp_buffer_typed_front(this);
            assert_eq!(
                (view.kind, view.length, view.offset),
                (EntryKind::Data, 2, 0)
            );
            swr_cpp_buffer_typed_consume(this, &view, 1);
            assert_eq!(swr_cpp_buffer_typed_front(this).offset, 1);
            swr_cpp_buffer_pop(this);
            swr_cpp_buffer_pop(this);
            assert_eq!(swr_cpp_buffer_typed_front(this).kind, EntryKind::Close);
            // Teardown pops the queued CLOSE marker.
        });
    }

    #[test]
    fn new_and_delete_buffer() {
        let buffer = new_buffer(4);
        buffer.append(b"abcdef");
        assert_eq!(swr_cpp_buffer_count(&buffer), 2);
        // Deleting a non-empty Buffer pops its entries first.
        drop(buffer);
    }

    /// Ported from the `tests/modules/buffer/typed.cc` fixture: the DATA cursor and an
    /// inert CLOSE marker at chunk sizes 1, 3, 16 and 0, and the front's layout.
    #[test]
    fn typed_queue_facade() {
        assert_eq!(
            (
                size_of::<Front>(),
                align_of::<Front>(),
                std::mem::offset_of!(Front, data),
                std::mem::offset_of!(Front, token),
                std::mem::offset_of!(Front, generation),
            ),
            (40, 8, 16, 24, 32)
        );
        let bytes = [b'a', 0, 255, b'b', b'c', b'd', b'e'];
        for chunk in [1u32, 3, 16, 0] {
            let queue = new_buffer(chunk);
            queue.append(&bytes);
            swr_cpp_buffer_typed_close(&queue);
            let mut copied = 0;
            while swr_cpp_buffer_typed_front(&queue).kind == EntryKind::Data {
                let front = swr_cpp_buffer_typed_front(&queue);
                assert_eq!(front.offset, 0);
                assert_eq!(
                    unsafe { std::slice::from_raw_parts(front.data, front.length as usize) },
                    &bytes[copied..copied + front.length as usize]
                );
                swr_cpp_buffer_typed_consume(&queue, &front, 1);
                let next = swr_cpp_buffer_typed_front(&queue);
                assert!(next.offset == 1 && next.generation == front.generation);
                assert_eq!(swr_cpp_buffer_length(&queue) as usize, bytes.len() - copied);
                swr_cpp_buffer_typed_consume(&queue, &next, next.length - next.offset);
                copied += next.length as usize;
                swr_cpp_buffer_pop(&queue);
                assert_eq!(swr_cpp_buffer_length(&queue) as usize, bytes.len() - copied);
            }
            assert_eq!(copied, bytes.len());
            assert_eq!(swr_cpp_buffer_typed_front(&queue).kind, EntryKind::Close);
            swr_cpp_buffer_pop(&queue);
            assert!(swr_cpp_buffer_empty(&queue) && swr_cpp_buffer_length(&queue) == 0);
        }
    }
}
