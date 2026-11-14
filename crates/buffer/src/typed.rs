//! The queue's front cursor and its native tokens (sendfile requests, CLOSE markers).
use super::{Buffer, Entry, Error, c_void};

type Dispose = Option<unsafe extern "C" fn(*mut c_void)>;

/// `BufferChunk::Type`: what a queued entry holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum Kind {
    #[default]
    Data = 0,
    /// A native sendfile request.
    Sendfile = 1,
    /// A payload-free CLOSE marker.
    Close = 2,
}

pub(super) struct Token {
    kind: Kind,
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
    pub kind: Kind,
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
    /// Transfer a native sendfile token, or enqueue a payload-free CLOSE marker (with or
    /// without a token); a data entry has no token, so is `Invalid`. Custody transfers on
    /// `Ok` only.
    /// # Errors
    /// `Invalid` for a data kind or a token that does not match it, `NoMem` when the queue
    /// cannot grow.
    /// # Safety
    /// `value` is unique to this queue, and `dispose(value)` is sound once and doesn't
    /// unwind; Rust stores the function address and only [`Detached::dispose`] calls it.
    pub unsafe fn push(
        &mut self,
        kind: Kind,
        value: *mut c_void,
        dispose: Dispose,
    ) -> Result<(), Error> {
        if !match kind {
            Kind::Sendfile => !value.is_null() && dispose.is_some(),
            Kind::Close => value.is_null() == dispose.is_none(),
            Kind::Data => false,
        } {
            return Err(Error::Invalid);
        }
        self.push_token(Token {
            kind,
            value,
            dispose,
        })
    }

    /// Enqueue a payload-free CLOSE marker.
    /// # Errors
    /// `NoMem` when the queue cannot grow.
    pub fn push_close(&mut self) -> Result<(), Error> {
        self.push_token(Token {
            kind: Kind::Close,
            value: std::ptr::null_mut(),
            dispose: None,
        })
    }

    fn push_token(&mut self, token: Token) -> Result<(), Error> {
        if self.entries.try_reserve(1).is_err() {
            return Err(Error::NoMem);
        }
        self.entries.push_back(Entry::TypedToken(token));
        Ok(())
    }

    /// The first entry's facts. A data entry's bytes stay the queue's until it is popped.
    /// # Errors
    /// `Empty` for an empty queue.
    pub fn front(&self) -> Result<Front, Error> {
        let mut result = Front {
            offset: self.cursor.offset,
            generation: self.cursor.generation,
            ..Front::default()
        };
        match self.entries.front() {
            None => return Err(Error::Empty),
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
    /// # Errors
    /// `Invalid` when the front is not that data entry or `count` passes its end.
    pub fn consume(&mut self, generation: u64, offset: u32, count: u32) -> Result<(), Error> {
        let cursor = &mut self.cursor;
        let Some(Entry::Data(bytes)) = self.entries.front() else {
            return Err(Error::Invalid);
        };
        if generation != cursor.generation
            || offset != cursor.offset
            || count > super::chunk_length(bytes) - cursor.offset
        {
            return Err(Error::Invalid);
        }
        cursor.offset += count;
        Ok(())
    }

    /// Detach exactly one entry. The caller takes the token's custody and disposes it
    /// outside the queue.
    /// # Errors
    /// `Empty` for an empty queue.
    pub fn pop(&mut self) -> Result<Detached, Error> {
        let Some(entry) = self.entries.pop_front() else {
            return Err(Error::Empty);
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
        assert_eq!(queue.append(b"abcdefg"), Ok(()));
        assert_eq!(
            unsafe { queue.push(Kind::Close, ptr::null_mut(), None) },
            Ok(())
        );
        let mut total = 7;
        for bytes in [b"abc".as_slice(), b"def".as_slice(), b"g".as_slice()] {
            let view = queue.front().unwrap();
            assert_eq!(view.kind, Kind::Data);
            assert_eq!(view.offset, 0);
            assert_eq!(
                unsafe { slice::from_raw_parts(view.data, view.length as usize) },
                bytes
            );
            assert_eq!(queue.front_data(), bytes);
            let generation = view.generation;
            assert_eq!(queue.consume(generation, 0, 1), Ok(()));
            let view = queue.front().unwrap();
            assert_eq!(view.offset, 1);
            assert_eq!(queue.length, total);
            assert_eq!(queue.consume(generation, 1, view.length - 1), Ok(()));
            let detached = queue.pop().unwrap();
            assert!(detached.token.is_null() && detached.dispose.is_none());
            total -= u32::try_from(bytes.len()).unwrap();
            assert_eq!(queue.length, total);
        }
        assert_eq!(queue.front().unwrap().kind, Kind::Close);
        assert_eq!(queue.front_data(), b"");
        queue.pop().unwrap();
        assert_eq!(queue.front().err(), Some(Error::Empty));
        let mut native_payload = 37u32;
        let token = ptr::from_mut(&mut native_payload).cast();
        // One reified pointer: each cast of `never_called` may get its own address.
        let dispose: unsafe extern "C" fn(*mut c_void) = never_called;
        assert_eq!(
            unsafe { queue.push(Kind::Sendfile, token, Some(dispose)) },
            Ok(())
        );
        let view = queue.front().unwrap();
        assert_eq!((view.kind, view.token), (Kind::Sendfile, token));
        let detached = queue.pop().unwrap();
        assert_eq!(detached.token, token);
        assert!(std::ptr::fn_addr_eq(detached.dispose.unwrap(), dispose));
        assert_eq!(native_payload, 37);
        assert!(queue.release().is_ok());
    }
}
