//! The queue's front cursor and its tokens (sendfile requests, CLOSE markers).
use super::{Buffer, Entry, Error, c_void};
use std::any::Any;

/// `BufferChunk::Type`: what a queued entry holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum Kind {
    #[default]
    Data = 0,
    /// A sendfile request.
    Sendfile = 1,
    /// A payload-free CLOSE marker.
    Close = 2,
}

pub(super) struct Token {
    kind: Kind,
    /// The queued request, which the queue owns; none for a CLOSE marker.
    value: Option<Box<dyn Any>>,
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

/// The address of a queued request, null without one: what [`Front::token`] and
/// [`Detached::token`] show.
fn address(value: Option<&dyn Any>) -> *mut c_void {
    value.map_or(std::ptr::null_mut(), |value| {
        std::ptr::from_ref(value).cast::<c_void>().cast_mut()
    })
}

/// A popped entry's request, which [`Detached::dispose`] drops.
#[derive(Default)]
pub struct Detached {
    token: Option<Box<dyn Any>>,
}

impl Detached {
    /// The popped request's address, null for a data entry or a CLOSE marker.
    #[must_use]
    pub fn token(&self) -> *mut c_void {
        address(self.token.as_deref())
    }

    /// Drops the request, if there is one.
    pub fn dispose(self) {
        drop(self.token);
    }
}

impl Buffer {
    /// Queue a sendfile request, which the queue owns until its entry is popped. A
    /// refused one is dropped.
    /// # Errors
    /// `NoMem` when the queue cannot grow.
    pub fn push_file(&mut self, request: Box<dyn Any>) -> Result<(), Error> {
        self.push_token(Token {
            kind: Kind::Sendfile,
            value: Some(request),
        })
    }

    /// Enqueue a payload-free CLOSE marker.
    /// # Errors
    /// `NoMem` when the queue cannot grow.
    pub fn push_close(&mut self) -> Result<(), Error> {
        self.push_token(Token {
            kind: Kind::Close,
            value: None,
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
                result.token = address(token.value.as_deref());
            }
        }
        Ok(result)
    }

    /// The first entry's sendfile request, when it is a `T`.
    pub fn front_file<T: Any>(&mut self) -> Option<&mut T> {
        match self.entries.front_mut() {
            Some(Entry::TypedToken(Token {
                value: Some(value), ..
            })) => value.downcast_mut(),
            _ => None,
        }
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
            Entry::TypedToken(token) => Detached { token: token.value },
        };
        self.cursor.offset = 0;
        self.cursor.generation = self.cursor.generation.wrapping_add(1);
        Ok(effect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, mem, ptr, rc::Rc, slice};

    /// A queued request that counts its drops.
    struct Payload(u32, Rc<Cell<u32>>);
    impl Drop for Payload {
        fn drop(&mut self) {
            self.1.set(self.1.get() + 1);
        }
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
        assert_eq!(queue.push_close(), Ok(()));
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
            assert!(detached.token.is_none());
            total -= u32::try_from(bytes.len()).unwrap();
            assert_eq!(queue.length, total);
        }
        assert_eq!(queue.front().unwrap().kind, Kind::Close);
        assert_eq!(queue.front_data(), b"");
        queue.pop().unwrap();
        assert_eq!(queue.front().err(), Some(Error::Empty));
        let drops = Rc::new(Cell::new(0));
        let payload = Box::new(Payload(37, Rc::clone(&drops)));
        let token = ptr::from_ref(&*payload).cast_mut().cast();
        assert_eq!(queue.push_file(payload), Ok(()));
        let view = queue.front().unwrap();
        assert_eq!((view.kind, view.token), (Kind::Sendfile, token));
        assert_eq!(
            queue.front_file::<Payload>().map(|payload| payload.0),
            Some(37)
        );
        let detached = queue.pop().unwrap();
        assert_eq!(detached.token(), token);
        assert_eq!(drops.get(), 0, "popping leaves the request to its caller");
        detached.dispose();
        assert_eq!(drops.get(), 1);
        assert!(queue.release().is_ok());
    }
}
