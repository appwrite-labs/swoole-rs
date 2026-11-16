//! Owned POSIX descriptor spans and their cursor. The allocation header owns the
//! complete `IOVector`; native callers borrow views instead of mirroring counters.
use crate::iov_progress::progress;
use crate::socket_rw::IovState;
use crate::{IovStep, ReactorIovProgress};
use std::io::IoSlice;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use swoole_rs_corebase::sys::ReadSlot;

/// An `IOVector` over buffers lent for `'a` in one direction: writable and reached
/// through nothing else for a read ([`Self::reading`]), readable for a write
/// ([`Self::writing`]). Only those make one, so its descriptors always name them.
pub struct ReactorIov<'a> {
    fields: ReactorIovFields,
    /// The two spans `fields` points into: the original descriptors, then the
    /// iterator's copy, whose last `remain_count` are the ones left.
    descriptors: Vec<libc::iovec>,
    write: bool,
    lent: PhantomData<&'a mut [u8]>,
}

/// One buffer a vectored read fills, as an `iovec`: slots that may not be written
/// yet, lent for `'b`.
#[repr(transparent)]
pub struct ReadVec<'b> {
    iov: libc::iovec,
    lent: PhantomData<&'b mut [MaybeUninit<u8>]>,
}

impl<'b> ReadVec<'b> {
    /// The vector over `slots`.
    #[must_use]
    pub fn new(slots: &'b mut [impl ReadSlot]) -> Self {
        Self {
            iov: libc::iovec {
                iov_base: slots.as_mut_ptr().cast(),
                iov_len: slots.len(),
            },
            lent: PhantomData,
        }
    }
}

/// Stable public slots borrowed by the native facade. Payload and descriptor
/// replacement obey the same live-span requirements as the original interface.
pub struct ReactorIovFields {
    pub original: *mut libc::iovec,
    pub count: i32,
    pub state: IovState,
}
/// Construct both descriptor spans from `source`, in `write`'s direction. Payload
/// bytes are neither read nor owned. Refused with -2 when it can't allocate.
fn owned_new<'a>(
    source: impl IntoIterator<Item = libc::iovec, IntoIter: ExactSizeIterator + Clone>,
    write: bool,
) -> Result<Box<ReactorIov<'a>>, i32> {
    let source = source.into_iter();
    let count = source.len();
    // The pinned Native expression count+count has no defined signed-overflow
    // behavior. This explicit refusal does not establish parity in that domain.
    let signed_count = i32::try_from(count)
        .ok()
        .filter(|&count| count <= i32::MAX / 2)
        .ok_or(-2)?;
    let mut descriptors = Vec::new();
    descriptors.try_reserve_exact(count * 2).map_err(|_| -2)?;
    descriptors.extend(source.clone());
    descriptors.extend(source);
    // Moving the vector into the owner leaves its buffer, and these pointers, in place.
    let original = descriptors.as_mut_ptr();
    let iterator = original.wrapping_add(count);
    swoole_rs_buffer::try_box(|| ReactorIov {
        fields: ReactorIovFields {
            original,
            count: signed_count,
            state: IovState {
                iterator,
                remain_count: signed_count,
                index: 0,
                offset_bytes: 0,
            },
        },
        descriptors,
        write,
        lent: PhantomData,
    })
    .ok_or(-2)
}

impl<'a> ReactorIov<'a> {
    /// `IOVector(iov, count)` to read into `vectors`' buffers: a cursor over a copy of
    /// their descriptors; none when it can't allocate.
    #[must_use]
    pub fn reading(vectors: &'a mut [ReadVec<'_>]) -> Option<Box<Self>> {
        owned_new(vectors.iter().map(|vector| vector.iov), false).ok()
    }

    /// `IOVector(iov, count)` to write `vectors`' bytes: a cursor over a copy of their
    /// descriptors; none when it can't allocate.
    #[must_use]
    pub fn writing(vectors: &'a [IoSlice<'_>]) -> Option<Box<Self>> {
        let iovecs = vectors.iter().map(|bytes| libc::iovec {
            iov_base: bytes.as_ptr().cast_mut().cast(),
            iov_len: bytes.len(),
        });
        owned_new(iovecs, true).ok()
    }

    /// Whether it was made to write from.
    #[must_use]
    pub const fn write(&self) -> bool {
        self.write
    }

    /// Where the descriptors left start: the iterator, which only `advance` moves,
    /// together with `remain_count`, so it stays `remain_count` from the end.
    fn left(&self) -> usize {
        let left = usize::try_from(self.fields.state.remain_count).unwrap_or(0);
        self.descriptors.len().saturating_sub(left)
    }

    /// The descriptors the cursor has left.
    fn remaining(&mut self) -> &mut [libc::iovec] {
        let start = self.left();
        self.descriptors.get_mut(start..).unwrap_or_default()
    }

    /// The descriptors the cursor has left, the current one trimmed to where the
    /// transfers stopped.
    #[must_use]
    pub fn remaining_iovecs(&self) -> &[libc::iovec] {
        self.descriptors.get(self.left()..).unwrap_or_default()
    }

    /// `IOVector::update_iterator(n)`: advance the descriptors and counters together.
    /// # Panics
    /// When `n` is more than the remaining descriptors hold.
    pub fn advance(&mut self, n: isize) {
        let state = self.fields.state;
        let reply = owned_progress(
            self.remaining(),
            n,
            state.remain_count,
            state.index,
            state.offset_bytes,
        );
        if reply.action == IovStep::Unchanged {
            return;
        }
        assert!(
            reply.action != IovStep::Overrun,
            "iovec progress outside the original domain"
        );
        let state = &mut self.fields.state;
        state.remain_count = reply.remain_count;
        state.index = reply.index;
        state.offset_bytes = reply.offset_bytes;
        if reply.action == IovStep::Advanced {
            state.iterator = state.iterator.wrapping_add(reply.advance);
        }
    }
}

/// Observe the cursor. No reference to the owner escapes the call.
#[must_use]
pub fn swr_v1_reactor_iov_view(owner: &ReactorIov<'_>) -> IovState {
    owner.fields.state
}

/// `Socket::readv`/`writev` through an owned `IOVector`, in the direction it was made
/// for: readv(2)/writev(2) over the remaining iovecs, or `tls` (its read and its
/// write) one iovec at a time until one comes up short, advancing the cursor; retried
/// on EINTR.
pub fn swr_v1_socket_iov_owned(
    fd: i32,
    owner: &mut ReactorIov<'_>,
    tls: Option<&(
        impl Fn(crate::Fill<'_>) -> Option<usize>,
        impl Fn(crate::Drain<'_>) -> Option<usize>,
    )>,
) -> Option<usize> {
    let write = owner.write;
    loop {
        let result = if let Some((read, send)) = tls {
            let mut total = 0;
            loop {
                let Some(current) = owner.remaining().first().copied() else {
                    break Some(total);
                };
                let done = if write {
                    // SAFETY: the type's invariant: a remaining iovec of a writing
                    // vector names bytes readable while it lives.
                    send(unsafe { crate::Drain::from_raw(current.iov_base, current.iov_len) })
                } else {
                    // SAFETY: the type's invariant: a remaining iovec of a reading
                    // vector names writable bytes nothing else reaches while it lives.
                    read(unsafe { crate::Fill::from_raw(current.iov_base, current.iov_len) })
                };
                total += done.unwrap_or(0);
                owner.advance(done.map_or(-1, usize::cast_signed));
                if !(done > Some(0) && owner.fields.state.remain_count > 0) {
                    break if total > 0 { Some(total) } else { done };
                }
            }
        } else {
            let count = owner.fields.state.remain_count;
            let left = owner.remaining().as_ptr();
            let done = if write {
                // SAFETY: the `remain_count` iovecs left name buffers readable while
                // the vector lives (the type's invariant).
                unsafe { libc::writev(fd, left, count) }
            } else {
                // SAFETY: as above, writable and reached through nothing else.
                unsafe { libc::readv(fd, left, count) }
            };
            owner.advance(done);
            usize::try_from(done).ok()
        };
        if !(result.is_none() && crate::socket_rw::interrupted()) {
            return result;
        }
    }
}
/// Sum the descriptors' lengths with C `size_t` wrapping semantics.
#[cfg(test)]
fn owned_length(descriptors: &[libc::iovec]) -> usize {
    descriptors
        .iter()
        .fold(0usize, |total, iov| total.wrapping_add(iov.iov_len))
}

/// Compute the progress policy over the `remaining` descriptors at `iterator` and
/// move the one it stops inside. The final completion path never touches them.
pub(crate) fn owned_progress(
    iterator: &mut [libc::iovec],
    n: isize,
    remaining: i32,
    index: i32,
    offset: usize,
) -> ReactorIovProgress {
    let reply = progress(
        iterator.iter().map(|iov| iov.iov_len),
        n,
        remaining,
        index,
        offset,
    );
    if reply.action == IovStep::Advanced
        && let Some(current) = iterator.get_mut(reply.advance)
    {
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
mod tests {
    use super::*;
    use swoole_rs_corebase::CCast;

    /// The `count` descriptors at `base`, none when `count` isn't positive.
    unsafe fn span<'a>(base: *mut libc::iovec, count: i32) -> &'a mut [libc::iovec] {
        match usize::try_from(count) {
            Ok(count) if count > 0 => unsafe { std::slice::from_raw_parts_mut(base, count) },
            _ => &mut [],
        }
    }

    #[test]
    fn owned_cursor_keeps_original_length_and_updates_live_state() {
        for count in [0, 1, 3, 17, 65] {
            let mut bytes = [0u8; 128];
            let source: Vec<_> = (0..count)
                .map(|i| libc::iovec {
                    iov_base: bytes.as_mut_ptr().wrapping_add(i).cast(),
                    iov_len: 1,
                })
                .collect();
            unsafe {
                let length = |owner: &ReactorIov<'_>| {
                    owned_length(span(owner.fields.original, owner.fields.count))
                };
                let owner = owned_new(source.iter().copied(), false).map(Box::into_raw);
                assert!(owner.is_ok_and(|owner| !owner.is_null()));
                let owner = owner.unwrap();
                let initial = swr_v1_reactor_iov_view(&*owner);
                assert_eq!(initial.remain_count, count.c_cast::<i32>());
                assert_eq!(initial.index, 0);
                assert_eq!(initial.offset_bytes, 0);
                assert_eq!(length(&*owner), count);
                if count > 0 {
                    (*owner).advance(1);
                    let next = swr_v1_reactor_iov_view(&*owner);
                    assert_eq!(next.remain_count, count.c_cast::<i32>() - 1);
                    assert_eq!(next.index, 1);
                    assert_eq!(next.offset_bytes, 0);
                    assert_eq!(length(&*owner), count);
                    assert_eq!(source[0].iov_len, 1);
                    assert_eq!(source[0].iov_base, bytes.as_mut_ptr().cast());
                }
                drop(Box::from_raw(owner));
            }
        }
    }
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
                let owner = owned_new(source.iter().copied(), false)
                    .map(Box::into_raw)
                    .unwrap();
                let (original, iterator) =
                    ((*owner).fields.original, (*owner).fields.state.iterator);
                assert_eq!(original.add(count), iterator);
                assert_eq!(original as usize % std::mem::align_of::<libc::iovec>(), 0);
                assert_eq!(owned_length(span(original, count.c_cast::<i32>())), count);
                for (i, expected) in source.iter().enumerate() {
                    for span in [original, iterator] {
                        assert_eq!((*span.add(i)).iov_base, expected.iov_base);
                        assert_eq!((*span.add(i)).iov_len, expected.iov_len);
                    }
                }
                let reply = owned_progress(
                    span(iterator, count.c_cast::<i32>()),
                    1,
                    count.c_cast::<i32>(),
                    0,
                    0,
                );
                assert_eq!(reply.remain_count, count.c_cast::<i32>() - 1);
                assert_eq!(reply.index, 1);
                assert_eq!(
                    reply.action,
                    if count == 1 {
                        IovStep::Finished
                    } else {
                        IovStep::Advanced
                    }
                );
                assert_eq!(owned_length(span(original, count.c_cast::<i32>())), count);
                drop(Box::from_raw(owner));
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
            let owner = owned_new([source], false).map(Box::into_raw).unwrap();
            let (original, iterator) = ((*owner).fields.original, (*owner).fields.state.iterator);
            let partial = owned_progress(span(iterator, 1), 2, 1, 0, 0);
            assert_eq!(
                (partial.action, partial.offset_bytes, partial.remain_count),
                (IovStep::Advanced, 2, 1)
            );
            assert_eq!((*iterator).iov_base, bytes.as_mut_ptr().add(2).cast());
            assert_eq!((*iterator).iov_len, 5);
            assert_eq!(owned_length(span(original, 1)), 7);
            let final_piece = owned_progress(span(iterator, 1), 5, 1, 0, 2);
            assert_eq!(
                (
                    final_piece.action,
                    final_piece.index,
                    final_piece.remain_count
                ),
                (IovStep::Finished, 1, 0)
            );
            assert_eq!((*iterator).iov_len, 5);
            assert_eq!((*iterator).iov_base, bytes.as_mut_ptr().add(2).cast());
            drop(Box::from_raw(owner));
        }
    }

    /// Upstream's `IOVector` as the hybrid's C++ facade drives these exports
    /// (socket.cc: the constructor, destructor and `update_iterator`).
    struct IoVector {
        owner: *mut ReactorIov<'static>,
        iov: *mut libc::iovec,
        iterator: *mut libc::iovec,
        count: i32,
        remain_count: i32,
        index: i32,
        offset_bytes: usize,
    }
    impl IoVector {
        fn new(input: &[libc::iovec]) -> Self {
            let count = input.len().c_cast::<i32>();
            let owner = owned_new(input.iter().copied(), false)
                .map(Box::into_raw)
                .unwrap();
            let fields = unsafe { &(*owner).fields };
            Self {
                owner,
                iov: fields.original,
                iterator: fields.state.iterator,
                count,
                remain_count: count,
                index: 0,
                offset_bytes: 0,
            }
        }
        fn update_iterator(&mut self, n: isize) {
            let reply = unsafe {
                owned_progress(
                    span(self.iterator, self.remain_count),
                    n,
                    self.remain_count,
                    self.index,
                    self.offset_bytes,
                )
            };
            assert_ne!(reply.action, IovStep::Overrun);
            if reply.action == IovStep::Unchanged {
                return;
            }
            self.remain_count = reply.remain_count;
            self.index = reply.index;
            self.offset_bytes = reply.offset_bytes;
            if reply.action == IovStep::Advanced {
                self.iterator = self.iterator.wrapping_add(reply.advance);
            }
        }
        fn length(&self) -> usize {
            unsafe { owned_length(span(self.iov, self.count)) }
        }
        fn original(&self, i: usize) -> libc::iovec {
            unsafe { *self.iov.add(i) }
        }
        fn remainder(&self) -> Vec<u8> {
            (0..self.remain_count.c_cast::<usize>())
                .flat_map(|i| unsafe {
                    let iov = *self.iterator.add(i);
                    std::slice::from_raw_parts(iov.iov_base.cast::<u8>(), iov.iov_len).to_vec()
                })
                .collect()
        }
    }
    impl Drop for IoVector {
        fn drop(&mut self) {
            drop(unsafe { Box::from_raw(self.owner) });
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
            let mut bytes: Vec<u8> = (0..total).map(CCast::c_cast::<u8>).collect();
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
                assert_eq!(v.count, input.len().c_cast::<i32>());
                for (i, expected) in snapshot.iter().enumerate() {
                    assert!(same(v.original(i), *expected) && same(input[i], *expected));
                }
            };
            for sent in 1..=total {
                let mut v = IoVector::new(&input);
                let initial = v.iterator;
                v.update_iterator(sent.c_cast::<isize>());
                let expected = if sent == total {
                    0
                } else {
                    input.len().c_cast::<i32>() - v.index
                };
                assert_eq!(v.remain_count, expected);
                if sent == total {
                    assert_eq!(v.iterator, initial);
                } else {
                    assert_eq!(v.remainder(), bytes[sent..]);
                    assert_eq!(v.iterator, initial.wrapping_add(v.index.c_cast::<usize>()));
                }
                unchanged(&v);
                metadata +=
                    v.offset_bytes + v.index.c_cast::<usize>() + v.remain_count.c_cast::<usize>();
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
                metadata +=
                    v.offset_bytes + v.index.c_cast::<usize>() + v.remain_count.c_cast::<usize>();
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
                (count.c_cast::<i32>(), count.c_cast::<i32>(), 0, 0)
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
                (0, count.c_cast::<i32>(), 1)
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
