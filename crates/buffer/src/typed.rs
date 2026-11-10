//! The queue's front cursor and its native tokens (sendfile requests, CLOSE markers).
use super::{Buffer, EMPTY, Entry, INVALID, NOMEM, OK, c_void, create_buffer};

type Dispose = Option<unsafe extern "C" fn(*mut c_void)>;

pub(super) struct Token {
    kind: u32,
    value: *mut c_void,
    dispose: Dispose,
}

#[derive(Default)]
pub(super) struct Cursor {
    offset: u32,
    generation: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Front {
    pub kind: u32,
    pub length: u32,
    pub offset: u32,
    pub data: *mut u8,
    pub token: *mut c_void,
    pub generation: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Detached {
    pub token: *mut c_void,
    pub dispose: Dispose,
}

/// Create an independent typed queue; never change the mode of an existing one.
/// # Safety
/// The output is aligned, writable and disjoint, as for the legacy create API.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_buffer_typed_create(size: u32, out: *mut *mut Buffer) -> i32 {
    // SAFETY: forwards the caller's aligned, writable, disjoint output.
    unsafe { create_buffer(size, out) }
}

/// Transfer a native sendfile token, or enqueue a payload-free CLOSE marker.
/// # Safety
/// Exclusive typed handle, unique native token and nonthrowing disposer; Rust
/// stores the function address without invoking it. Custody transfers on OK only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_buffer_typed_push(
    buffer: *mut Buffer,
    kind: u32,
    value: *mut c_void,
    dispose: Dispose,
) -> i32 {
    if buffer.is_null() {
        return INVALID;
    }
    // SAFETY: `buffer` is non-null (checked above), live and exclusive.
    let queue = unsafe { &mut *buffer };
    if !match kind {
        1 => !value.is_null() && dispose.is_some(),
        2 => value.is_null() == dispose.is_none(),
        _ => false,
    } {
        return INVALID;
    }
    if queue.entries.try_reserve(1).is_err() {
        return NOMEM;
    }
    queue.entries.push_back(Entry::TypedToken(Token {
        kind,
        value,
        dispose,
    }));
    OK
}

/// Copy initialized facts, with no borrowed Rust reference escaping the call.
/// # Safety
/// Live typed handle and writable aligned disjoint output. DATA remains owned
/// by the queue until detach; callers must not mutate/reenter it during effects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_buffer_typed_front(buffer: *const Buffer, out: *mut Front) -> i32 {
    if out.is_null() {
        return INVALID;
    }
    // SAFETY: `out` is non-null (checked above), aligned, writable and disjoint.
    unsafe { out.write(Front::default()) };
    if buffer.is_null() {
        return INVALID;
    }
    // SAFETY: `buffer` is non-null (checked above) and live.
    let queue = unsafe { &*buffer };
    let cursor = &queue.cursor;
    let mut result = Front {
        offset: cursor.offset,
        generation: cursor.generation,
        ..Front::default()
    };
    match queue.entries.front() {
        None => return EMPTY,
        Some(Entry::Data(bytes)) => {
            result.length = super::chunk_length(bytes);
            result.data = bytes.as_ptr().cast_mut();
        }
        Some(Entry::TypedToken(token)) => {
            result.kind = token.kind;
            result.token = token.value;
        }
    }
    // SAFETY: `out` is non-null (checked above), aligned, writable and disjoint.
    unsafe { out.write(result) };
    OK
}

/// Commit a positive ordinary DATA send against the observed front identity.
/// # Safety
/// Live exclusive typed handle; identity/offset are copied observations. This
/// updates only the cursor, retaining full queued-byte accounting until detach.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_buffer_typed_consume(
    buffer: *mut Buffer,
    generation: u64,
    offset: u32,
    count: u32,
) -> i32 {
    if buffer.is_null() {
        return INVALID;
    }
    // SAFETY: `buffer` is non-null (checked above), live and exclusive.
    let queue = unsafe { &mut *buffer };
    let cursor = &mut queue.cursor;
    let Some(Entry::Data(bytes)) = queue.entries.front() else {
        return INVALID;
    };
    if generation != cursor.generation
        || offset != cursor.offset
        || count > super::chunk_length(bytes) - cursor.offset
    {
        return INVALID;
    }
    cursor.offset += count;
    OK
}

/// Detach exactly one entry; native token disposal happens after this returns.
/// # Safety
/// Exclusive typed handle and aligned writable disjoint output. Native caller
/// assumes token custody and invokes any disposer outside every Rust frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_buffer_typed_pop(buffer: *mut Buffer, out: *mut Detached) -> i32 {
    if out.is_null() {
        return INVALID;
    }
    // SAFETY: `out` is non-null (checked above), aligned, writable and disjoint.
    unsafe { out.write(Detached::default()) };
    if buffer.is_null() {
        return INVALID;
    }
    // SAFETY: `buffer` is non-null (checked above), live and exclusive.
    let queue = unsafe { &mut *buffer };
    let Some(entry) = queue.entries.pop_front() else {
        return EMPTY;
    };
    let effect = match entry {
        Entry::Data(bytes) => {
            queue.length -= super::chunk_length(&bytes);
            queue.recycle(bytes);
            Detached::default()
        }
        Entry::TypedToken(token) => Detached {
            token: token.value,
            dispose: token.dispose,
        },
    };
    queue.cursor.offset = 0;
    queue.cursor.generation = queue.cursor.generation.wrapping_add(1);
    // SAFETY: `out` is non-null (checked above), aligned, writable and disjoint.
    unsafe { out.write(effect) };
    OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{swr_v1_buffer_append, swr_v1_buffer_release};
    use std::{mem, ptr, slice};

    unsafe extern "C" fn never_called(_: *mut c_void) {
        panic!("Rust must never invoke Native disposal");
    }

    #[test]
    #[expect(clippy::cognitive_complexity, reason = "one end-to-end queue scenario")]
    fn typed_queue_owns_cursor_tags_and_full_byte_accounting() {
        assert_eq!(mem::size_of::<Front>(), 40);
        assert_eq!(mem::align_of::<Front>(), 8);
        assert_eq!(mem::offset_of!(Front, data), 16);
        assert_eq!(mem::offset_of!(Front, token), 24);
        assert_eq!(mem::offset_of!(Front, generation), 32);
        assert_eq!(mem::size_of::<Detached>(), 16);
        unsafe {
            let mut queue = ptr::null_mut();
            assert_eq!(swr_v1_buffer_typed_create(3, &raw mut queue), OK);
            assert_eq!(swr_v1_buffer_append(queue, b"abcdefg".as_ptr(), 7), OK);
            assert_eq!(
                swr_v1_buffer_typed_push(queue, 2, ptr::null_mut(), None),
                OK
            );
            let mut view = Front::default();
            let mut detached = Detached::default();
            let mut total = 7;
            for bytes in [b"abc".as_slice(), b"def".as_slice(), b"g".as_slice()] {
                assert_eq!(swr_v1_buffer_typed_front(queue, &raw mut view), OK);
                assert_eq!(view.kind, 0);
                assert_eq!(view.offset, 0);
                assert_eq!(
                    slice::from_raw_parts(view.data, view.length as usize),
                    bytes
                );
                let generation = view.generation;
                assert_eq!(swr_v1_buffer_typed_consume(queue, generation, 0, 1), OK);
                assert_eq!(swr_v1_buffer_typed_front(queue, &raw mut view), OK);
                assert_eq!(view.offset, 1);
                assert_eq!((*queue).length, total);
                assert_eq!(
                    swr_v1_buffer_typed_consume(queue, generation, 1, view.length - 1),
                    OK
                );
                assert_eq!(swr_v1_buffer_typed_pop(queue, &raw mut detached), OK);
                assert!(detached.token.is_null() && detached.dispose.is_none());
                total -= u32::try_from(bytes.len()).unwrap();
                assert_eq!((*queue).length, total);
            }
            assert_eq!(swr_v1_buffer_typed_front(queue, &raw mut view), OK);
            assert_eq!(view.kind, 2);
            assert_eq!(swr_v1_buffer_typed_pop(queue, &raw mut detached), OK);
            assert_eq!(swr_v1_buffer_typed_front(queue, &raw mut view), EMPTY);
            let mut native_payload = 37u32;
            let token = ptr::from_mut(&mut native_payload).cast();
            // One reified pointer: each cast of `never_called` may get its own address.
            let dispose: unsafe extern "C" fn(*mut c_void) = never_called;
            assert_eq!(swr_v1_buffer_typed_push(queue, 1, token, Some(dispose)), OK);
            assert_eq!(swr_v1_buffer_typed_front(queue, &raw mut view), OK);
            assert_eq!((view.kind, view.token), (1, token));
            assert_eq!(swr_v1_buffer_typed_pop(queue, &raw mut detached), OK);
            assert_eq!(detached.token, token);
            assert!(std::ptr::fn_addr_eq(detached.dispose.unwrap(), dispose));
            assert_eq!(native_payload, 37);
            assert_eq!(swr_v1_buffer_release(queue), OK);
        }
    }
}
