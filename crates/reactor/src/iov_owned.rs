//! Owned POSIX descriptor spans and their cursor. The allocation header owns the
//! complete `IOVector`; native callers borrow views instead of mirroring counters.
use crate::ReactorIovProgress;
use crate::iov_progress::progress;
use crate::socket_rw::{IovState, TlsIo};
use std::alloc::{Layout, alloc, dealloc};
use std::ptr;

pub struct ReactorIov {
    layout: Layout,
    fields: ReactorIovFields,
}

/// Stable public slots borrowed by the native facade. Payload and descriptor
/// replacement obey the same live-span requirements as the original interface.
#[repr(C)]
pub struct ReactorIovFields {
    pub original: *mut libc::iovec,
    pub count: i32,
    pub state: IovState,
}
fn allocation_layout(count: usize) -> Option<(Layout, usize)> {
    let descriptors = Layout::array::<libc::iovec>(count.checked_mul(2)?).ok()?;
    let (layout, offset) = Layout::new::<ReactorIov>().extend(descriptors).ok()?;
    Some((layout.pad_to_align(), offset))
}
/// Construct both descriptor spans. Payload bytes are neither read nor owned.
/// Refused with -1 for a negative count, -2 when it can't allocate.
/// # Safety
/// For count>0, source names count initialized, readable iovecs, which stay the
/// caller's and are disjoint from the new owner.
#[expect(
    clippy::cast_ptr_alignment,
    reason = "the header sits at the start of allocation_layout's aligned block"
)]
unsafe fn owned_new(source: *const libc::iovec, count: i32) -> Result<*mut ReactorIov, i32> {
    if count < 0 {
        return Err(-1);
    }
    // The pinned Native expression count+count has no defined signed-overflow
    // behavior. This explicit refusal does not establish parity in that domain.
    if count > i32::MAX / 2 {
        return Err(-2);
    }
    let signed_count = count;
    let count = count.unsigned_abs() as usize;
    let (layout, offset) = allocation_layout(count).ok_or(-2)?;
    // SAFETY: `allocation_layout` returns a non-zero-sized layout; null is handled below.
    let allocation = unsafe { alloc(layout) };
    if allocation.is_null() {
        return Err(-2);
    }
    // `layout` holds the header, then two arrays of `count` iovecs starting at `offset`,
    // each suitably aligned.
    let original = allocation.wrapping_add(offset).cast::<libc::iovec>();
    let iterator = original.wrapping_add(count);
    // SAFETY: the header slot at the start of the fresh allocation is aligned and writable.
    unsafe {
        allocation.cast::<ReactorIov>().write(ReactorIov {
            layout,
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
        });
    };
    if count != 0 {
        // SAFETY: for `count > 0` the caller lends `count` readable, initialized iovecs at
        // `source`, disjoint from this fresh allocation; `original` has room for `count`.
        unsafe { ptr::copy_nonoverlapping(source, original, count) };
        // SAFETY: as above, for the second array.
        unsafe { ptr::copy_nonoverlapping(source, iterator, count) };
    }
    Ok(allocation.cast())
}

/// Release an opaque cursor and both spans; payload bytes stay borrowed.
/// # Safety
/// `owner` is the unique live cursor returned by prepare, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_iov_destroy(owner: *mut ReactorIov) {
    if !owner.is_null() {
        // SAFETY: the live owner's header, written at allocation.
        let layout = unsafe { (*owner).layout };
        // SAFETY: allocated with `layout`; exclusively owned and released once.
        unsafe { dealloc(owner.cast(), layout) };
    }
}

/// An opaque cursor in one allocation with its descriptors, and its field slots.
#[repr(C)]
pub struct ReactorIovPrepared {
    pub owner: *mut ReactorIov,
    pub fields: *mut ReactorIovFields,
    /// 0 ready, -1 a negative count, -2 an allocation refusal.
    pub status: i32,
}
/// # Safety
/// `source` lends `count` initialized iovecs. The slots remain valid until destroy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_iov_prepare(
    source: *const libc::iovec,
    count: i32,
) -> ReactorIovPrepared {
    // SAFETY: as the caller promises.
    match unsafe { owned_new(source, count) } {
        Ok(owner) => ReactorIovPrepared {
            owner,
            fields: owner
                .wrapping_byte_add(std::mem::offset_of!(ReactorIov, fields))
                .cast(),
            status: 0,
        },
        Err(status) => ReactorIovPrepared {
            owner: ptr::null_mut(),
            fields: ptr::null_mut(),
            status,
        },
    }
}

/// Observe the cursor. No reference to the owner escapes the call.
/// # Safety
/// `owner` is a live immutable cursor.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_iov_view(owner: *const ReactorIov) -> IovState {
    // SAFETY: live immutable owner per the contract.
    unsafe { (*owner).fields.state }
}

/// Sum the unchanged original descriptors.
/// # Safety
/// `owner` is a live immutable cursor with readable original descriptors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_iov_length(owner: *const ReactorIov) -> usize {
    // SAFETY: live immutable owner per the contract.
    let fields = unsafe { &(*owner).fields };
    // SAFETY: creation copied `count` (never negative) descriptors to `original`, live until destroy.
    owned_length(unsafe { span(fields.original, fields.count) })
}

/// Advance owned descriptors and counters together.
/// # Safety
/// `owner` is live/exclusive and `n` is in the original defined progress domain.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_iov_advance(owner: *mut ReactorIov, n: isize) {
    // SAFETY: live exclusive cursor per the contract.
    let owner = unsafe { &mut *owner };
    // SAFETY: its state owns the remaining initialized descriptor span.
    unsafe { owner.fields.state.update(n) };
}

/// Read/write through an owned `IOVector` without copying state through C++.
/// # Safety
/// `owner` is live/exclusive; each payload is valid for the direction and the
/// optional nonthrowing TLS effect is valid for `socket`.
pub unsafe fn swr_v1_socket_iov_owned(
    fd: i32,
    write: bool,
    owner: *mut ReactorIov,
    tls: Option<TlsIo>,
    socket: *mut std::ffi::c_void,
) -> isize {
    // SAFETY: live exclusive owner per the contract.
    let state = unsafe { &mut (*owner).fields.state };
    // SAFETY: the payload and TLS effect contracts are forwarded unchanged.
    unsafe { crate::socket_iov(fd, write, state, tls, socket) }
}
/// The `count` descriptors at `base`, none when `count` isn't positive.
///
/// # Safety
/// For positive `count`, `base` names `count` initialized iovecs, which nothing
/// else uses while the span lives.
pub(crate) unsafe fn span<'a>(base: *mut libc::iovec, count: i32) -> &'a mut [libc::iovec] {
    match usize::try_from(count) {
        // SAFETY: as the caller promises.
        Ok(count) if count > 0 => unsafe { std::slice::from_raw_parts_mut(base, count) },
        _ => &mut [],
    }
}

/// Sum the descriptors' lengths with C `size_t` wrapping semantics.
pub(crate) fn owned_length(descriptors: &[libc::iovec]) -> usize {
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
    if reply.action == 1
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
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "tests drive the native integer interfaces with literal and loop values"
)]
mod tests {
    use super::*;

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
                let owner = owned_new(source.as_ptr(), count as i32);
                assert!(owner.is_ok_and(|owner| !owner.is_null()));
                let owner = owner.unwrap();
                let initial = swr_v1_reactor_iov_view(owner);
                assert_eq!(initial.remain_count, count as i32);
                assert_eq!(initial.index, 0);
                assert_eq!(initial.offset_bytes, 0);
                assert_eq!(swr_v1_reactor_iov_length(owner), count);
                if count > 0 {
                    swr_v1_reactor_iov_advance(owner, 1);
                    let next = swr_v1_reactor_iov_view(owner);
                    assert_eq!(next.remain_count, count as i32 - 1);
                    assert_eq!(next.index, 1);
                    assert_eq!(next.offset_bytes, 0);
                    assert_eq!(swr_v1_reactor_iov_length(owner), count);
                    assert_eq!(source[0].iov_len, 1);
                    assert_eq!(source[0].iov_base, bytes.as_mut_ptr().cast());
                }
                swr_v1_reactor_iov_destroy(owner);
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
                let owner = owned_new(source.as_ptr(), count as i32).unwrap();
                let (original, iterator) =
                    ((*owner).fields.original, (*owner).fields.state.iterator);
                assert_eq!(original.add(count), iterator);
                assert_eq!(original as usize % std::mem::align_of::<libc::iovec>(), 0);
                assert_eq!(owned_length(span(original, count as i32)), count);
                for (i, expected) in source.iter().enumerate() {
                    for span in [original, iterator] {
                        assert_eq!((*span.add(i)).iov_base, expected.iov_base);
                        assert_eq!((*span.add(i)).iov_len, expected.iov_len);
                    }
                }
                let reply = owned_progress(span(iterator, count as i32), 1, count as i32, 0, 0);
                assert_eq!(reply.remain_count, count as i32 - 1);
                assert_eq!(reply.index, 1);
                assert_eq!(reply.action, if count == 1 { 2 } else { 1 });
                assert_eq!(owned_length(span(original, count as i32)), count);
                swr_v1_reactor_iov_destroy(owner);
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
            let owner = owned_new(&raw const source, 1).unwrap();
            let (original, iterator) = ((*owner).fields.original, (*owner).fields.state.iterator);
            let partial = owned_progress(span(iterator, 1), 2, 1, 0, 0);
            assert_eq!(
                (partial.action, partial.offset_bytes, partial.remain_count),
                (1, 2, 1)
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
                (2, 1, 0)
            );
            assert_eq!((*iterator).iov_len, 5);
            assert_eq!((*iterator).iov_base, bytes.as_mut_ptr().add(2).cast());
            swr_v1_reactor_iov_destroy(owner);
        }
    }

    /// Upstream's `IOVector` as the hybrid's C++ facade drives these exports
    /// (socket.cc: the constructor, destructor and `update_iterator`).
    struct IoVector {
        owner: *mut ReactorIov,
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
            let owner = unsafe { owned_new(input.as_ptr(), count) }.unwrap();
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
            unsafe { owned_length(span(self.iov, self.count)) }
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
            unsafe { swr_v1_reactor_iov_destroy(self.owner) };
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
