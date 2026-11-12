//! The queue's front cursor and its native tokens (sendfile requests, CLOSE markers).
use super::{Buffer, EMPTY, Entry, INVALID, NOMEM, OK, c_void};

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

/// The first entry, as C++'s `Buffer::typed_front()` returns it.
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

/// A popped entry's native token and its disposer, which [`Detached::dispose`] runs once.
#[derive(Default)]
pub struct Detached {
    token: *mut c_void,
    dispose: Dispose,
}

impl Detached {
    /// The popped token, null for a data entry or a CLOSE marker.
    #[must_use]
    pub const fn token(&self) -> *mut c_void {
        self.token
    }

    /// Hands the token to its disposer, if it has one.
    pub fn dispose(self) {
        if let Some(dispose) = self.dispose {
            // SAFETY: `push`'s caller handed the queue this token for `dispose` to free;
            // it left the queue in this `Detached`, which isn't `Copy` and is consumed here.
            unsafe { dispose(self.token) };
        }
    }
}

impl Buffer {
    /// Transfer a native sendfile token (kind 1), or enqueue a payload-free CLOSE marker
    /// (kind 2, with or without a token). Custody transfers on OK only.
    /// # Safety
    /// `value` is unique to this queue, and `dispose(value)` is sound once and doesn't
    /// unwind; Rust stores the function address and only [`Detached::dispose`] calls it.
    pub unsafe fn push(&mut self, kind: u32, value: *mut c_void, dispose: Dispose) -> i32 {
        if !match kind {
            1 => !value.is_null() && dispose.is_some(),
            2 => value.is_null() == dispose.is_none(),
            _ => false,
        } {
            return INVALID;
        }
        self.push_token(Token {
            kind,
            value,
            dispose,
        })
    }

    /// Enqueue a payload-free CLOSE marker.
    pub fn push_close(&mut self) -> i32 {
        self.push_token(Token {
            kind: 2,
            value: std::ptr::null_mut(),
            dispose: None,
        })
    }

    fn push_token(&mut self, token: Token) -> i32 {
        if self.entries.try_reserve(1).is_err() {
            return NOMEM;
        }
        self.entries.push_back(Entry::TypedToken(token));
        OK
    }

    /// The first entry's facts. A data entry's bytes stay the queue's until it is popped.
    /// # Errors
    /// `EMPTY` for an empty queue.
    pub fn front(&self) -> Result<Front, i32> {
        let mut result = Front {
            offset: self.cursor.offset,
            generation: self.cursor.generation,
            ..Front::default()
        };
        match self.entries.front() {
            None => return Err(EMPTY),
            Some(Entry::Data(bytes)) => {
                result.length = super::chunk_length(bytes);
                result.data = bytes.as_ptr().cast_mut();
            }
            Some(Entry::TypedToken(token)) => {
                result.kind = token.kind;
                result.token = token.value;
            }
        }
        Ok(result)
    }

    /// The first entry's bytes, sent ones included: empty for a token or an empty queue.
    #[must_use]
    pub fn front_data(&self) -> &[u8] {
        match self.entries.front() {
            Some(Entry::Data(bytes)) => bytes,
            _ => &[],
        }
    }

    /// Commit a positive ordinary DATA send against the observed front identity.
    /// This updates only the cursor, retaining full queued-byte accounting until pop.
    pub fn consume(&mut self, generation: u64, offset: u32, count: u32) -> i32 {
        let cursor = &mut self.cursor;
        let Some(Entry::Data(bytes)) = self.entries.front() else {
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

    /// Detach exactly one entry. The caller takes the token's custody and disposes it
    /// outside the queue.
    /// # Errors
    /// `EMPTY` for an empty queue.
    pub fn pop(&mut self) -> Result<Detached, i32> {
        let Some(entry) = self.entries.pop_front() else {
            return Err(EMPTY);
        };
        let effect = match entry {
            Entry::Data(bytes) => {
                self.length -= super::chunk_length(&bytes);
                self.recycle(bytes);
                Detached::default()
            }
            Entry::TypedToken(token) => Detached {
                token: token.value,
                dispose: token.dispose,
            },
        };
        self.cursor.offset = 0;
        self.cursor.generation = self.cursor.generation.wrapping_add(1);
        Ok(effect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{mem, ptr, slice};

    unsafe extern "C" fn never_called(_: *mut c_void) {
        panic!("Rust must never invoke Native disposal");
    }

    #[test]
    fn typed_queue_owns_cursor_tags_and_full_byte_accounting() {
        assert_eq!(mem::size_of::<Front>(), 40);
        assert_eq!(mem::align_of::<Front>(), 8);
        assert_eq!(mem::offset_of!(Front, data), 16);
        assert_eq!(mem::offset_of!(Front, token), 24);
        assert_eq!(mem::offset_of!(Front, generation), 32);
        let mut queue = Buffer::create(3).unwrap();
        assert_eq!(queue.append(b"abcdefg"), OK);
        assert_eq!(unsafe { queue.push(2, ptr::null_mut(), None) }, OK);
        let mut total = 7;
        for bytes in [b"abc".as_slice(), b"def".as_slice(), b"g".as_slice()] {
            let view = queue.front().unwrap();
            assert_eq!(view.kind, 0);
            assert_eq!(view.offset, 0);
            assert_eq!(
                unsafe { slice::from_raw_parts(view.data, view.length as usize) },
                bytes
            );
            assert_eq!(queue.front_data(), bytes);
            let generation = view.generation;
            assert_eq!(queue.consume(generation, 0, 1), OK);
            let view = queue.front().unwrap();
            assert_eq!(view.offset, 1);
            assert_eq!(queue.length, total);
            assert_eq!(queue.consume(generation, 1, view.length - 1), OK);
            let detached = queue.pop().unwrap();
            assert!(detached.token.is_null() && detached.dispose.is_none());
            total -= u32::try_from(bytes.len()).unwrap();
            assert_eq!(queue.length, total);
        }
        assert_eq!(queue.front().unwrap().kind, 2);
        assert_eq!(queue.front_data(), b"");
        queue.pop().unwrap();
        assert_eq!(queue.front().err(), Some(EMPTY));
        let mut native_payload = 37u32;
        let token = ptr::from_mut(&mut native_payload).cast();
        // One reified pointer: each cast of `never_called` may get its own address.
        let dispose: unsafe extern "C" fn(*mut c_void) = never_called;
        assert_eq!(unsafe { queue.push(1, token, Some(dispose)) }, OK);
        let view = queue.front().unwrap();
        assert_eq!((view.kind, view.token), (1, token));
        let detached = queue.pop().unwrap();
        assert_eq!(detached.token, token);
        assert!(std::ptr::fn_addr_eq(detached.dispose.unwrap(), dispose));
        assert_eq!(native_payload, 37);
        assert!(queue.release().is_ok());
    }
}
