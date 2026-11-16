//! An `IOVector`: a copy of the caller's descriptors, typed by the direction their
//! buffers are lent in, and its cursor. Payload bytes are neither read nor copied.
use crate::iov_progress::progress;
use crate::socket_rw::IovState;
use crate::{Drain, Fill, IovStep};
use std::io::IoSlice;
use swoole_rs_corebase::sys;

pub use swoole_rs_corebase::lent::ReadVec;

/// The descriptors of buffers lent for `'a`: writable and reached through nothing else
/// for a read, readable for a write.
enum Descriptors<'a> {
    Read(Vec<ReadVec<'a>>),
    Write(Vec<IoSlice<'a>>),
}

/// An `IOVector` over buffers lent for `'a` in one direction ([`Self::reading`] or
/// [`Self::writing`]). The cursor moves the last `remain_count` descriptors, which are
/// the ones left.
pub struct ReactorIov<'a> {
    descriptors: Descriptors<'a>,
    state: IovState,
}

/// What is left of one of a [`ReactorIov`]'s buffers, lent in its direction: slots
/// to read into, or bytes to write.
pub enum IovBuffer<'a> {
    Read(Fill<'a>),
    Write(Drain<'a>),
}

impl IovBuffer<'_> {
    /// Whether nothing is left of it.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Read(fill) => fill.is_empty(),
            Self::Write(data) => data.is_empty(),
        }
    }

    /// The same buffer, lent again for a call that may be repeated.
    pub fn reborrow(&mut self) -> IovBuffer<'_> {
        match self {
            Self::Read(fill) => IovBuffer::Read(fill.reborrow()),
            Self::Write(data) => IovBuffer::Write(*data),
        }
    }

    /// Steps past the `moved` bytes a call transferred: whether any are left.
    pub fn advance(&mut self, moved: usize) -> bool {
        match self {
            Self::Read(fill) => fill.advance(moved),
            Self::Write(data) => data.advance(moved),
        }
    }
}

/// A copy of `source` and a cursor at its start; none where `IOVector`'s constructor
/// throws: it can't allocate, or the pin's `count + count` would overflow its `int`.
fn copy<T>(source: impl ExactSizeIterator<Item = T>) -> Option<(Vec<T>, IovState)> {
    let count = source.len();
    let remain_count = i32::try_from(count)
        .ok()
        .filter(|&count| count <= i32::MAX / 2)?;
    let mut copy = Vec::new();
    copy.try_reserve_exact(count).ok()?;
    copy.extend(source);
    let state = IovState {
        remain_count,
        index: 0,
        offset_bytes: 0,
    };
    Some((copy, state))
}

impl<'a> ReactorIov<'a> {
    /// `IOVector(iov, count)` to read into `vectors`' buffers: a cursor over a copy of
    /// their descriptors; none when it can't allocate.
    #[must_use]
    pub fn reading(vectors: &'a mut [ReadVec<'_>]) -> Option<Self> {
        let (copy, state) = copy(vectors.iter_mut().map(ReadVec::reborrow))?;
        Some(Self {
            descriptors: Descriptors::Read(copy),
            state,
        })
    }

    /// `IOVector(iov, count)` to write `vectors`' bytes: a cursor over a copy of their
    /// descriptors; none when it can't allocate.
    #[must_use]
    pub fn writing(vectors: &'a [IoSlice<'_>]) -> Option<Self> {
        let (copy, state) = copy(vectors.iter().copied())?;
        Some(Self {
            descriptors: Descriptors::Write(copy),
            state,
        })
    }

    /// Whether it was made to write from.
    #[must_use]
    pub const fn write(&self) -> bool {
        matches!(self.descriptors, Descriptors::Write(_))
    }

    /// Where the descriptors left start: `remain_count` from the end.
    fn left(&self) -> usize {
        let count = match &self.descriptors {
            Descriptors::Read(vectors) => vectors.len(),
            Descriptors::Write(vectors) => vectors.len(),
        };
        let left = usize::try_from(self.state.remain_count).unwrap_or(0);
        count.saturating_sub(left)
    }

    /// The descriptors left, the current one trimmed to where the transfers stopped,
    /// as the `iovec` array a ring request names: where it starts and how many. Both
    /// descriptor types are ABI-compatible with an `iovec`.
    #[must_use]
    pub fn remaining_iovecs(&self) -> (*const libc::iovec, usize) {
        let left = self.left();
        match &self.descriptors {
            Descriptors::Read(vectors) => {
                let rest = vectors.get(left..).unwrap_or_default();
                (rest.as_ptr().cast(), rest.len())
            }
            Descriptors::Write(vectors) => {
                let rest = vectors.get(left..).unwrap_or_default();
                (rest.as_ptr().cast(), rest.len())
            }
        }
    }

    /// The buffers left, in order, each lent in the vector's direction, the current
    /// one trimmed to where the transfers stopped.
    pub fn buffers(&mut self) -> impl Iterator<Item = IovBuffer<'_>> {
        let left = self.left();
        let (read, write): (&mut [ReadVec<'a>], &[IoSlice<'a>]) = match &mut self.descriptors {
            Descriptors::Read(vectors) => (
                vectors.get_mut(left..).unwrap_or_default(),
                Default::default(),
            ),
            Descriptors::Write(vectors) => {
                (Default::default(), vectors.get(left..).unwrap_or_default())
            }
        };
        let read = read.iter_mut().map(|vector| IovBuffer::Read(vector.fill()));
        read.chain(
            write
                .iter()
                .map(|bytes| IovBuffer::Write(Drain::new(bytes))),
        )
    }

    /// `IOVector::update_iterator(n)`: advance the descriptors and counters together.
    /// # Panics
    /// When `n` is more than the remaining descriptors hold.
    pub fn advance(&mut self, n: isize) {
        let (left, state) = (self.left(), self.state);
        let (remain, index, offset) = (state.remain_count, state.index, state.offset_bytes);
        let reply = match &self.descriptors {
            Descriptors::Read(vectors) => {
                let lengths = vectors.get(left..).unwrap_or_default().iter();
                progress(lengths.map(ReadVec::len), n, remain, index, offset)
            }
            Descriptors::Write(vectors) => {
                let lengths = vectors.get(left..).unwrap_or_default().iter();
                progress(lengths.map(|bytes| bytes.len()), n, remain, index, offset)
            }
        };
        if reply.action == IovStep::Unchanged {
            return;
        }
        assert!(
            reply.action != IovStep::Overrun,
            "iovec progress outside the original domain"
        );
        self.state = IovState {
            remain_count: reply.remain_count,
            index: reply.index,
            offset_bytes: reply.offset_bytes,
        };
        // The final completion never touches a descriptor.
        if reply.action == IovStep::Advanced {
            let current = left + reply.advance;
            match &mut self.descriptors {
                Descriptors::Read(vectors) => {
                    if let Some(vector) = vectors.get_mut(current) {
                        vector.advance(reply.consumed);
                    }
                }
                Descriptors::Write(vectors) => {
                    if let Some(bytes) = vectors.get_mut(current) {
                        bytes.advance(reply.consumed);
                    }
                }
            }
        }
    }
}

/// Observe the cursor. No reference to the owner escapes the call.
#[must_use]
pub fn swr_v1_reactor_iov_view(owner: &ReactorIov<'_>) -> IovState {
    owner.state
}

/// `Socket::readv`/`writev` through an owned `IOVector`, in the direction it was made
/// for: readv(2)/writev(2) over the remaining iovecs, or `tls` (its read and its
/// write) one iovec at a time until one comes up short, advancing the cursor; retried
/// on EINTR.
pub fn swr_v1_socket_iov_owned(
    fd: i32,
    owner: &mut ReactorIov<'_>,
    tls: Option<&(
        impl Fn(Fill<'_>) -> Option<usize>,
        impl Fn(Drain<'_>) -> Option<usize>,
    )>,
) -> Option<usize> {
    loop {
        let result = if let Some((read, send)) = tls {
            let mut total = 0;
            loop {
                let done = match owner.buffers().next() {
                    None => break Some(total),
                    Some(IovBuffer::Read(fill)) => read(fill),
                    Some(IovBuffer::Write(data)) => send(data),
                };
                total += done.unwrap_or(0);
                owner.advance(done.map_or(-1, usize::cast_signed));
                if !(done > Some(0) && owner.state.remain_count > 0) {
                    break if total > 0 { Some(total) } else { done };
                }
            }
        } else {
            let left = owner.left();
            let done = match &mut owner.descriptors {
                Descriptors::Read(vectors) => {
                    sys::readv(fd, vectors.get_mut(left..).unwrap_or_default())
                }
                Descriptors::Write(vectors) => {
                    sys::writev(fd, vectors.get(left..).unwrap_or_default())
                }
            };
            owner.advance(done.map_or(-1, usize::cast_signed));
            done
        };
        if !(result.is_none() && crate::socket_rw::interrupted()) {
            return result;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swoole_rs_corebase::CCast;

    /// Sum the descriptors' lengths with C `size_t` wrapping semantics.
    fn owned_length(descriptors: &[libc::iovec]) -> usize {
        descriptors
            .iter()
            .fold(0usize, |total, iov| total.wrapping_add(iov.iov_len))
    }

    /// The descriptors `owner` holds now, the ones done included.
    fn descriptors(owner: &ReactorIov<'_>) -> Vec<libc::iovec> {
        match &owner.descriptors {
            Descriptors::Read(vectors) => vectors.iter().map(ReadVec::descriptor).collect(),
            Descriptors::Write(vectors) => vectors
                .iter()
                .map(|bytes| libc::iovec {
                    iov_base: bytes.as_ptr().cast_mut().cast(),
                    iov_len: bytes.len(),
                })
                .collect(),
        }
    }

    /// Vectors to read into the buffers `input` names, which outlive them.
    fn reading<'a>(input: &[libc::iovec]) -> Vec<ReadVec<'a>> {
        let slots = |iov: &libc::iovec| unsafe {
            std::slice::from_raw_parts_mut(iov.iov_base.cast::<u8>(), iov.iov_len)
        };
        input.iter().map(|iov| ReadVec::new(slots(iov))).collect()
    }

    /// Vectors to write the buffers `input` names, which outlive them.
    fn writing<'a>(input: &[libc::iovec]) -> Vec<IoSlice<'a>> {
        let bytes = |iov: &libc::iovec| unsafe {
            std::slice::from_raw_parts(iov.iov_base.cast::<u8>(), iov.iov_len)
        };
        input.iter().map(|iov| IoSlice::new(bytes(iov))).collect()
    }

    fn same(a: libc::iovec, b: libc::iovec) -> bool {
        a.iov_base == b.iov_base && a.iov_len == b.iov_len
    }

    fn all_same(a: &[libc::iovec], b: &[libc::iovec]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(*a, *b))
    }

    #[test]
    fn owned_cursor_keeps_original_length_and_updates_live_state() {
        for count in [0, 1, 3, 17, 65] {
            let mut bytes = [0u8; 128];
            let source: Vec<_> = (0..count)
                .map(|i| libc::iovec {
                    iov_base: (&raw mut bytes).cast::<u8>().wrapping_add(i).cast(),
                    iov_len: 1,
                })
                .collect();
            let mut vectors = reading(&source);
            let mut owner = ReactorIov::reading(&mut vectors).unwrap();
            let initial = swr_v1_reactor_iov_view(&owner);
            assert_eq!(initial.remain_count, count.c_cast::<i32>());
            assert_eq!(initial.index, 0);
            assert_eq!(initial.offset_bytes, 0);
            assert_eq!(owned_length(&descriptors(&owner)), count);
            if count > 0 {
                owner.advance(1);
                let next = swr_v1_reactor_iov_view(&owner);
                assert_eq!(next.remain_count, count.c_cast::<i32>() - 1);
                assert_eq!(next.index, 1);
                assert_eq!(next.offset_bytes, 0);
                assert_eq!(owned_length(&descriptors(&owner)), count);
            }
            drop(owner);
            assert!(all_same(
                &vectors.iter().map(ReadVec::descriptor).collect::<Vec<_>>(),
                &source
            ));
            if count > 0 {
                assert_eq!(source[0].iov_len, 1);
                assert_eq!(source[0].iov_base, (&raw mut bytes).cast::<u8>().cast());
            }
        }
    }

    #[test]
    fn iov_owned_positive_construct_copy_length_cursor_and_release() {
        for count in [1, 3, 17, 65] {
            let mut bytes = [0u8; 128];
            let source: Vec<_> = (0..count)
                .map(|i| libc::iovec {
                    iov_base: (&raw mut bytes).cast::<u8>().wrapping_add(i).cast(),
                    iov_len: 1,
                })
                .collect();
            let mut vectors = reading(&source);
            let mut owner = ReactorIov::reading(&mut vectors).unwrap();
            assert_eq!(owned_length(&descriptors(&owner)), count);
            assert!(all_same(&descriptors(&owner), &source));
            owner.advance(1);
            let state = swr_v1_reactor_iov_view(&owner);
            assert_eq!(state.remain_count, count.c_cast::<i32>() - 1);
            assert_eq!(state.index, 1);
            assert_eq!(owned_length(&descriptors(&owner)), count);
            drop(owner);
            assert!(all_same(
                &vectors.iter().map(ReadVec::descriptor).collect::<Vec<_>>(),
                &source
            ));
        }
        // Normal positive partial consumption moves only the vector's copy; final
        // completion must not touch that descriptor again.
        let mut bytes = [0u8; 7];
        let source = libc::iovec {
            iov_base: (&raw mut bytes).cast::<u8>().cast(),
            iov_len: 7,
        };
        let mut vectors = reading(&[source]);
        let mut owner = ReactorIov::reading(&mut vectors).unwrap();
        owner.advance(2);
        let partial = swr_v1_reactor_iov_view(&owner);
        assert_eq!((partial.offset_bytes, partial.remain_count), (2, 1));
        let current = descriptors(&owner)[0];
        assert_eq!(
            current.iov_base,
            (&raw mut bytes).cast::<u8>().wrapping_add(2).cast()
        );
        assert_eq!(current.iov_len, 5);
        owner.advance(5);
        let last = swr_v1_reactor_iov_view(&owner);
        assert_eq!((last.index, last.remain_count), (1, 0));
        assert!(same(descriptors(&owner)[0], current));
        drop(owner);
        assert!(same(vectors[0].descriptor(), source));
    }

    /// Upstream's `IOVector` as the native facade drove it: the constructor over a
    /// copy, `update_iterator` and the counters.
    struct IoVector<'a> {
        owner: ReactorIov<'a>,
        count: i32,
    }
    impl IoVector<'_> {
        fn state(&self) -> IovState {
            swr_v1_reactor_iov_view(&self.owner)
        }
        fn update_iterator(&mut self, n: isize) {
            self.owner.advance(n);
        }
        /// The iterator's distance from the copy's start.
        fn iterator(&self) -> usize {
            self.owner.left()
        }
        fn remainder(&self) -> Vec<u8> {
            descriptors(&self.owner)[self.iterator()..]
                .iter()
                .flat_map(|iov| unsafe {
                    std::slice::from_raw_parts(iov.iov_base.cast::<u8>(), iov.iov_len).to_vec()
                })
                .collect()
        }
    }

    /// `run` over an `IoVector` of `input` made in each direction.
    fn both(input: &[libc::iovec], mut run: impl FnMut(IoVector<'_>)) {
        let count = input.len().c_cast::<i32>();
        let mut read = reading(input);
        run(IoVector {
            owner: ReactorIov::reading(&mut read).unwrap(),
            count,
        });
        assert!(all_same(
            &read.iter().map(ReadVec::descriptor).collect::<Vec<_>>(),
            input
        ));
        let write = writing(input);
        run(IoVector {
            owner: ReactorIov::writing(&write).unwrap(),
            count,
        });
        assert!(write.iter().zip(input).all(
            |(bytes, iov)| bytes.as_ptr() == iov.iov_base.cast() && bytes.len() == iov.iov_len
        ));
    }

    // The cases of the retired `iov_owned` gate, which ran them on the pinned `IOVector`
    // and on this one: 47 cases, 42 of them summing the cursor to the pin's 483. Each
    // direction runs them all.
    #[test]
    fn iov_vector_cursor_matches_the_pin() {
        for direction in 0..2 {
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
                let run = |run: &mut dyn FnMut(IoVector<'_>)| {
                    let mut runs = 0;
                    both(&input, |v| {
                        if runs == direction {
                            run(v);
                        }
                        runs += 1;
                    });
                };
                for sent in 1..=total {
                    run(&mut |mut v| {
                        let before = descriptors(&v.owner);
                        v.update_iterator(sent.c_cast::<isize>());
                        let state = v.state();
                        let expected = if sent == total {
                            0
                        } else {
                            input.len().c_cast::<i32>() - state.index
                        };
                        assert_eq!(state.remain_count, expected);
                        if sent == total {
                            assert!(all_same(&descriptors(&v.owner), &before));
                        } else {
                            assert_eq!(v.remainder(), bytes[sent..]);
                            assert_eq!(v.iterator(), state.index.c_cast::<usize>());
                        }
                        assert_eq!(v.count, input.len().c_cast::<i32>());
                        metadata += state.offset_bytes
                            + state.index.c_cast::<usize>()
                            + state.remain_count.c_cast::<usize>();
                        cases += 1;
                    });
                }
                run(&mut |mut v| {
                    for sent in 1..=total {
                        let before = descriptors(&v.owner);
                        v.update_iterator(1);
                        let state = v.state();
                        if sent == total {
                            assert_eq!(state.remain_count, 0);
                            assert!(all_same(&descriptors(&v.owner), &before));
                        } else {
                            assert_eq!(v.remainder(), bytes[sent..]);
                        }
                        metadata += state.offset_bytes
                            + state.index.c_cast::<usize>()
                            + state.remain_count.c_cast::<usize>();
                    }
                    cases += 1;
                });
                assert!(all_same(&input, &snapshot));
            }
            assert_eq!((cases, metadata), (42, 483));
        }
    }

    /// Construction copies the descriptors; updating moves only the copy.
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
            both(&input, |mut v| {
                let state = v.state();
                assert_eq!(v.iterator(), 0);
                assert_eq!(
                    (v.count, state.remain_count, state.index, state.offset_bytes),
                    (count.c_cast::<i32>(), count.c_cast::<i32>(), 0, 0)
                );
                assert_eq!(owned_length(&descriptors(&v.owner)), count * 3);
                assert!(all_same(&descriptors(&v.owner), &before));
                v.update_iterator(1);
                let state = v.state();
                assert_eq!(
                    (state.index, state.remain_count, state.offset_bytes),
                    (0, count.c_cast::<i32>(), 1)
                );
                let current = descriptors(&v.owner)[0];
                assert_eq!(current.iov_base, before[0].iov_base.wrapping_byte_add(1));
                assert_eq!(current.iov_len, 2);
            });
            assert!(all_same(&input, &before));
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
        both(&input, |mut v| {
            v.update_iterator(1);
            assert_eq!(v.state().offset_bytes, 1);
            v.update_iterator(1);
            let state = v.state();
            assert_eq!(
                (state.index, state.remain_count, state.offset_bytes),
                (1, 1, 1)
            );
            let current = descriptors(&v.owner)[v.iterator()];
            assert!(same(current, input[1]));
        });
    }
}
