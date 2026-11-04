//! Opt-in owned POSIX descriptor storage. Public C++ fields retain their layout;
//! allocation-local release metadata is independent of those mutable fields.
use crate::ReactorIovProgress;
use crate::iov_progress::progress;
use std::alloc::{Layout, alloc, dealloc};
use std::ptr;

struct Header {
    layout: Layout,
}
#[repr(C)]
pub struct ReactorIovAllocation {
    pub original: *mut libc::iovec,
    pub iterator: *mut libc::iovec,
    ///0 ready, -1 invalid Native array bound, -2 allocation/capacity refusal.
    pub status: i32,
}
fn allocation_layout(count: usize) -> Option<(Layout, usize)> {
    let descriptors = Layout::array::<libc::iovec>(count.checked_mul(2)?).ok()?;
    let (layout, offset) = Layout::new::<Header>().extend(descriptors).ok()?;
    Some((layout.pad_to_align(), offset))
}
fn descriptor_offset() -> usize {
    std::mem::size_of::<Header>().next_multiple_of(std::mem::align_of::<libc::iovec>())
}
/// Construct both actual descriptor spans, not a Native mirror. Payload bytes
/// are neither read nor owned. Native throws only after this function returns.
/// # Safety
/// For count>0, source names count initialized, readable target C iovec objects.
/// Source objects/payloads remain Native-owned and disjoint from the new owner.
#[unsafe(no_mangle)]
#[expect(
    clippy::cast_ptr_alignment,
    reason = "the header sits at the start of allocation_layout's aligned block"
)]
pub unsafe extern "C" fn swr_v1_reactor_iov_owned_new(
    source: *const libc::iovec,
    count: i32,
) -> ReactorIovAllocation {
    let refused = |status| ReactorIovAllocation {
        original: ptr::null_mut(),
        iterator: ptr::null_mut(),
        status,
    };
    if count < 0 {
        return refused(-1);
    }
    // The pinned Native expression count+count has no defined signed-overflow
    // behavior. This explicit refusal does not establish parity in that domain.
    if count > i32::MAX / 2 {
        return refused(-2);
    }
    let count = count.unsigned_abs() as usize;
    let Some((layout, offset)) = allocation_layout(count) else {
        return refused(-2);
    };
    // SAFETY: `allocation_layout` returns a non-zero-sized layout; null is handled below.
    let allocation = unsafe { alloc(layout) };
    if allocation.is_null() {
        return refused(-2);
    }
    // `layout` holds the header, then two arrays of `count` iovecs starting at `offset`,
    // each suitably aligned.
    let original = allocation.wrapping_add(offset).cast::<libc::iovec>();
    let iterator = original.wrapping_add(count);
    // SAFETY: the header slot at the start of the fresh allocation is aligned and writable.
    unsafe { allocation.cast::<Header>().write(Header { layout }) };
    if count != 0 {
        // SAFETY: for `count > 0` the caller lends `count` readable, initialized iovecs at
        // `source`, disjoint from this fresh allocation; `original` has room for `count`.
        unsafe { ptr::copy_nonoverlapping(source, original, count) };
        // SAFETY: as above, for the second array.
        unsafe { ptr::copy_nonoverlapping(source, iterator, count) };
    }
    ReactorIovAllocation {
        original,
        iterator,
        status: 0,
    }
}
/// Release only descriptor storage and allocation-local metadata. No payload,
/// Native callback, destructor or public counter is consulted.
/// # Safety
/// original is null or the unmodified base returned by `owned_new`, exclusively owned and
/// released exactly once. Foreign/replaced/shallow-copied owners are unsupported.
#[unsafe(no_mangle)]
#[expect(
    clippy::cast_ptr_alignment,
    reason = "the header sits at the start of allocation_layout's aligned block"
)]
pub unsafe extern "C" fn swr_v1_reactor_iov_owned_free(original: *mut libc::iovec) {
    if original.is_null() {
        return;
    }
    // `original` is the unmodified base from `owned_new`, so the allocation (and its header)
    // starts `descriptor_offset()` bytes before it.
    let allocation = original.cast::<u8>().wrapping_sub(descriptor_offset());
    // SAFETY: as above; the header was written at allocation time.
    let header = unsafe { allocation.cast::<Header>().read() };
    // SAFETY: allocated with `header.layout`; exclusively owned and released once.
    unsafe { dealloc(allocation, header.layout) };
}
/// Sum the live original descriptor span with C `size_t` wrapping semantics.
/// # Safety
/// For positive count, original names count readable initialized C iovec objects.
/// No payload is dereferenced and no borrow survives this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_iov_owned_length(
    original: *const libc::iovec,
    count: i32,
) -> usize {
    (0..count).fold(0usize, |total, i| {
        // SAFETY: `i < count`, and `original` names `count` readable initialized iovecs.
        total.wrapping_add(unsafe { (*original.wrapping_add(i.unsigned_abs() as usize)).iov_len })
    })
}
/// Compute the existing progress policy and mutate the actual owned iterator
/// descriptor before returning new public cursor facts. The final completion
/// path never touches the iterator payload, matching the original behavior.
/// # Safety
/// For positive n/remaining, iterator names remaining writable initialized
/// descriptors with the same live C payload-pointer domain as the Native API.
/// Public counter/sum values are in the original defined integer domain.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_iov_owned_progress(
    iterator: *mut libc::iovec,
    n: isize,
    remaining: i32,
    index: i32,
    offset: usize,
) -> ReactorIovProgress {
    let reply = progress(
        (0..remaining).map(|i| {
            // SAFETY: `i < remaining`, and `iterator` names `remaining` initialized iovecs.
            unsafe { (*iterator.wrapping_add(i.unsigned_abs() as usize)).iov_len }
        }),
        n,
        remaining,
        index,
        offset,
    );
    if reply.action == 1 {
        // SAFETY: `progress` returns `advance < remaining`; the iterator entries are
        // writable and nothing else borrows them during this call.
        let current = unsafe { &mut *iterator.wrapping_add(reply.advance) };
        current.iov_base = current
            .iov_base
            .cast::<u8>()
            .wrapping_add(reply.consumed)
            .cast();
        current.iov_len = current.iov_len.wrapping_sub(reply.consumed);
    }
    reply
}
#[cfg(test)]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "tests drive the native integer interfaces with literal and loop values"
)]
mod tests {
    use super::*;
    #[test]
    fn iov_owned_positive_construct_copy_length_cursor_and_release() {
        for count in [1, 3, 17, 65] {
            let mut bytes = [0u8; 128];
            let source: Vec<_> = (0..count)
                .map(|i| libc::iovec {
                    iov_base: bytes.as_mut_ptr().wrapping_add(i).cast(),
                    iov_len: 1,
                })
                .collect();
            unsafe {
                let owner = swr_v1_reactor_iov_owned_new(source.as_ptr(), count as i32);
                assert_eq!(owner.status, 0);
                assert_eq!(owner.original.add(count), owner.iterator);
                assert_eq!(
                    owner.original as usize % std::mem::align_of::<libc::iovec>(),
                    0
                );
                assert_eq!(
                    swr_v1_reactor_iov_owned_length(owner.original, count as i32),
                    count
                );
                for (i, expected) in source.iter().enumerate() {
                    for span in [owner.original, owner.iterator] {
                        assert_eq!((*span.add(i)).iov_base, expected.iov_base);
                        assert_eq!((*span.add(i)).iov_len, expected.iov_len);
                    }
                }
                let reply =
                    swr_v1_reactor_iov_owned_progress(owner.iterator, 1, count as i32, 0, 0);
                assert_eq!(reply.remain_count, count as i32 - 1);
                assert_eq!(reply.index, 1);
                assert_eq!(reply.action, if count == 1 { 2 } else { 1 });
                assert_eq!(
                    swr_v1_reactor_iov_owned_length(owner.original, count as i32),
                    count
                );
                swr_v1_reactor_iov_owned_free(owner.original);
            }
        }
        // Normal positive partial consumption mutates only the iterator span;
        // final completion must not touch that descriptor again.
        let mut bytes = [0u8; 7];
        let source = libc::iovec {
            iov_base: bytes.as_mut_ptr().cast(),
            iov_len: 7,
        };
        unsafe {
            let owner = swr_v1_reactor_iov_owned_new(&raw const source, 1);
            assert_eq!(owner.status, 0);
            let partial = swr_v1_reactor_iov_owned_progress(owner.iterator, 2, 1, 0, 0);
            assert_eq!(
                (partial.action, partial.offset_bytes, partial.remain_count),
                (1, 2, 1)
            );
            assert_eq!((*owner.iterator).iov_base, bytes.as_mut_ptr().add(2).cast());
            assert_eq!((*owner.iterator).iov_len, 5);
            assert_eq!(swr_v1_reactor_iov_owned_length(owner.original, 1), 7);
            let final_piece = swr_v1_reactor_iov_owned_progress(owner.iterator, 5, 1, 0, 2);
            assert_eq!(
                (
                    final_piece.action,
                    final_piece.index,
                    final_piece.remain_count
                ),
                (2, 1, 0)
            );
            assert_eq!((*owner.iterator).iov_len, 5);
            assert_eq!((*owner.iterator).iov_base, bytes.as_mut_ptr().add(2).cast());
            swr_v1_reactor_iov_owned_free(owner.original);
        }
    }

    /// Upstream's `IOVector` as the hybrid's C++ facade drives these exports
    /// (socket.cc: the constructor, destructor and `update_iterator`).
    struct IoVector {
        iov: *mut libc::iovec,
        iterator: *mut libc::iovec,
        count: i32,
        remain_count: i32,
        index: i32,
        offset_bytes: usize,
    }
    impl IoVector {
        fn new(input: &[libc::iovec]) -> Self {
            let count = input.len() as i32;
            let allocation = unsafe { swr_v1_reactor_iov_owned_new(input.as_ptr(), count) };
            assert_eq!(allocation.status, 0);
            Self {
                iov: allocation.original,
                iterator: allocation.iterator,
                count,
                remain_count: count,
                index: 0,
                offset_bytes: 0,
            }
        }
        fn update_iterator(&mut self, n: isize) {
            let reply = unsafe {
                swr_v1_reactor_iov_owned_progress(
                    self.iterator,
                    n,
                    self.remain_count,
                    self.index,
                    self.offset_bytes,
                )
            };
            assert!(reply.action >= 0);
            if reply.action == 0 {
                return;
            }
            self.remain_count = reply.remain_count;
            self.index = reply.index;
            self.offset_bytes = reply.offset_bytes;
            if reply.action == 1 {
                self.iterator = self.iterator.wrapping_add(reply.advance);
            }
        }
        fn length(&self) -> usize {
            unsafe { swr_v1_reactor_iov_owned_length(self.iov, self.count) }
        }
        fn original(&self, i: usize) -> libc::iovec {
            unsafe { *self.iov.add(i) }
        }
        fn remainder(&self) -> Vec<u8> {
            (0..self.remain_count as usize)
                .flat_map(|i| unsafe {
                    let iov = *self.iterator.add(i);
                    std::slice::from_raw_parts(iov.iov_base.cast::<u8>(), iov.iov_len).to_vec()
                })
                .collect()
        }
    }
    impl Drop for IoVector {
        fn drop(&mut self) {
            unsafe { swr_v1_reactor_iov_owned_free(self.iov) };
        }
    }
    fn same(a: libc::iovec, b: libc::iovec) -> bool {
        a.iov_base == b.iov_base && a.iov_len == b.iov_len
    }

    // The cases of the retired `iov_owned` gate, which ran them on the pinned `IOVector`
    // and on this one: 47 cases, 42 of them summing the cursor to the pin's 483.
    #[test]
    fn iov_vector_cursor_matches_the_pin() {
        let (mut cases, mut metadata) = (0, 0usize);
        for lengths in [&[1][..], &[2, 3], &[1, 3, 7], &[4, 1, 2, 9], &[0, 1, 0, 3]] {
            let total: usize = lengths.iter().sum();
            let mut bytes: Vec<u8> = (0..total).map(|i| i as u8).collect();
            let mut start = 0;
            let input: Vec<_> = lengths
                .iter()
                .map(|&length| {
                    let iov = libc::iovec {
                        iov_base: bytes.as_mut_ptr().wrapping_add(start).cast(),
                        iov_len: length,
                    };
                    start += length;
                    iov
                })
                .collect();
            let snapshot = input.clone();
            let unchanged = |v: &IoVector| {
                assert_eq!(v.length(), total);
                assert_eq!(v.count, input.len() as i32);
                for (i, expected) in snapshot.iter().enumerate() {
                    assert!(same(v.original(i), *expected) && same(input[i], *expected));
                }
            };
            for sent in 1..=total {
                let mut v = IoVector::new(&input);
                let initial = v.iterator;
                v.update_iterator(sent as isize);
                let expected = if sent == total {
                    0
                } else {
                    input.len() as i32 - v.index
                };
                assert_eq!(v.remain_count, expected);
                if sent == total {
                    assert_eq!(v.iterator, initial);
                } else {
                    assert_eq!(v.remainder(), bytes[sent..]);
                    assert_eq!(v.iterator, initial.wrapping_add(v.index as usize));
                }
                unchanged(&v);
                metadata += v.offset_bytes + v.index as usize + v.remain_count as usize;
                cases += 1;
            }
            let mut v = IoVector::new(&input);
            for sent in 1..=total {
                let previous = v.iterator;
                v.update_iterator(1);
                if sent == total {
                    assert_eq!(v.remain_count, 0);
                    assert_eq!(v.iterator, previous);
                } else {
                    assert_eq!(v.remainder(), bytes[sent..]);
                }
                unchanged(&v);
                metadata += v.offset_bytes + v.index as usize + v.remain_count as usize;
            }
            cases += 1;
        }
        assert_eq!((cases, metadata), (42, 483));
    }

    /// Construction copies the descriptors; updating moves only the iterator copy.
    #[test]
    fn iov_vector_copies_and_moves_only_the_iterator() {
        for count in [1, 3, 17, 65] {
            let mut bytes = vec![b'x'; count * 3];
            let input: Vec<_> = (0..count)
                .map(|i| libc::iovec {
                    iov_base: bytes.as_mut_ptr().wrapping_add(i * 3).cast(),
                    iov_len: 3,
                })
                .collect();
            let before = input.clone();
            let mut v = IoVector::new(&input);
            assert_ne!(v.iov.cast_const(), input.as_ptr());
            assert_eq!(v.iterator, v.iov.wrapping_add(count));
            assert_eq!(
                (v.count, v.remain_count, v.index, v.offset_bytes),
                (count as i32, count as i32, 0, 0)
            );
            assert_eq!(v.iov as usize % std::mem::align_of::<libc::iovec>(), 0);
            assert_eq!(v.length(), count * 3);
            for (i, expected) in before.iter().enumerate() {
                assert!(same(v.original(i), *expected));
                assert!(same(unsafe { *v.iterator.add(i) }, *expected));
            }
            v.update_iterator(1);
            assert_eq!(
                (v.index, v.remain_count, v.offset_bytes),
                (0, count as i32, 1)
            );
            let current = unsafe { *v.iterator };
            assert_eq!(current.iov_base, before[0].iov_base.wrapping_byte_add(1));
            assert_eq!(current.iov_len, 2);
            assert_eq!(v.length(), count * 3);
            for (i, expected) in before.iter().enumerate() {
                assert!(same(v.original(i), *expected) && same(input[i], *expected));
            }
        }
    }

    /// `offset_bytes` is not reset when an update crosses into the next descriptor.
    #[test]
    fn iov_vector_keeps_offset_across_a_boundary() {
        let mut data = *b"abcde";
        let input = [
            libc::iovec {
                iov_base: data.as_mut_ptr().cast(),
                iov_len: 2,
            },
            libc::iovec {
                iov_base: data.as_mut_ptr().wrapping_add(2).cast(),
                iov_len: 3,
            },
        ];
        let mut v = IoVector::new(&input);
        v.update_iterator(1);
        assert_eq!(v.offset_bytes, 1);
        v.update_iterator(1);
        assert_eq!((v.index, v.remain_count, v.offset_bytes), (1, 1, 1));
        let current = unsafe { *v.iterator };
        assert!(current.iov_base == input[1].iov_base && current.iov_len == 3);
    }
}
