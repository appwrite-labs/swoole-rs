//! The buffer of one I/O call as a typed handle: [`Fill`] is bytes a read may write and
//! [`Drain`] bytes a write may read. A slice makes one safely; an entry point C calls
//! makes one from its pointer and length, once, and everything below it is safe. Each is
//! an address and a length, as the raw pair was, so a null address stays what the kernel
//! (`EFAULT`) or OpenSSL (nothing to move) made of it.
use crate::SocketAddress;
use std::ffi::{c_int, c_void};
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use swoole_rs_corebase::sys::ReadSlot;

/// `len` bytes at `at` that a read may overwrite, lent for `'a`: null, or writable
/// and reached through nothing else meanwhile.
pub struct Fill<'a, S: ReadSlot = MaybeUninit<u8>> {
    at: *mut S,
    len: usize,
    lent: PhantomData<&'a mut [S]>,
}

impl<'a, S: ReadSlot> Fill<'a, S> {
    /// `buffer`'s bytes.
    #[inline]
    pub fn new(buffer: &'a mut [S]) -> Self {
        Self {
            at: buffer.as_mut_ptr(),
            len: buffer.len(),
            lent: PhantomData,
        }
    }

    /// How many bytes are left to fill.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether nothing is left to fill.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The same bytes, lent again for a call that may be repeated.
    #[inline]
    pub fn reborrow(&mut self) -> Fill<'_, S> {
        Fill {
            at: self.at,
            len: self.len,
            lent: PhantomData,
        }
    }

    /// Steps past the `moved` bytes a call filled: whether any are left. A count that
    /// leaves none (or exceeds what was lent) changes nothing and answers false.
    #[inline]
    pub fn advance(&mut self, moved: usize) -> bool {
        if moved >= self.len {
            return false;
        }
        // Within the lent bytes, since `moved < len`.
        self.at = self.at.wrapping_add(moved);
        self.len -= moved;
        true
    }

    /// The bytes as a slice, borrowed from the handle; none for a null address.
    #[inline]
    #[must_use]
    pub fn slots(&mut self) -> &mut [S] {
        if self.at.is_null() {
            return &mut [];
        }
        // SAFETY: the type's invariant: `len` writable one-byte slots, of which every
        // bit pattern is a value, lent to this handle alone, which this borrows.
        unsafe { std::slice::from_raw_parts_mut(self.at, self.len) }
    }

    /// recv(2) into the bytes.
    #[inline]
    pub fn recv(&mut self, fd: c_int, flags: c_int) -> isize {
        // SAFETY: the type's invariant: null, which the kernel refuses, or `len`
        // writable bytes.
        unsafe { libc::recv(fd, self.at.cast(), self.len, flags) }
    }

    /// read(2) into the bytes.
    #[inline]
    pub fn read(&mut self, fd: c_int) -> isize {
        // SAFETY: as in `recv`.
        unsafe { libc::read(fd, self.at.cast(), self.len) }
    }

    /// [`crate::fd_recvfrom`] into the bytes, and the peer into `address` and its length.
    #[inline]
    pub fn recvfrom(&mut self, fd: c_int, flags: c_int, address: &mut SocketAddress) -> isize {
        let (storage, size) = (&raw mut address.addr, &raw mut address.len);
        // SAFETY: as in `recv`; the address's own storage and its length.
        unsafe { crate::fd_recvfrom(fd, self.at.cast(), self.len, flags, storage.cast(), size) }
    }
}

impl Fill<'_> {
    /// The `len` bytes at `at`.
    ///
    /// # Safety
    /// `at` is null, or denotes `len` writable bytes that nothing else reads or writes
    /// for `'a`.
    #[inline]
    #[must_use]
    pub unsafe fn from_raw(at: *mut c_void, len: usize) -> Self {
        Self {
            at: at.cast(),
            len,
            lent: PhantomData,
        }
    }
}

/// `len` bytes at `at` that a write reads, lent for `'a`: null, or readable and not
/// written meanwhile.
#[derive(Clone, Copy)]
pub struct Drain<'a> {
    at: *const u8,
    len: usize,
    lent: PhantomData<&'a [u8]>,
}

impl<'a> Drain<'a> {
    /// `data`'s bytes.
    #[inline]
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            at: data.as_ptr(),
            len: data.len(),
            lent: PhantomData,
        }
    }

    /// The `len` bytes at `at`.
    ///
    /// # Safety
    /// `at` is null, or denotes `len` readable bytes that nothing writes for `'a`.
    #[inline]
    #[must_use]
    pub unsafe fn from_raw(at: *const c_void, len: usize) -> Self {
        Self {
            at: at.cast(),
            len,
            lent: PhantomData,
        }
    }

    /// How many bytes are left to write.
    #[inline]
    #[must_use]
    pub fn len(self) -> usize {
        self.len
    }

    /// Where the bytes are, for a call that takes an address and [`Self::len`].
    #[inline]
    #[must_use]
    pub fn as_ptr(self) -> *const c_void {
        self.at.cast()
    }

    /// Whether nothing is left to write.
    #[inline]
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.len == 0
    }

    /// Steps past the `moved` bytes a call wrote: whether any are left, as
    /// [`Fill::advance`] does.
    #[inline]
    pub fn advance(&mut self, moved: usize) -> bool {
        if moved >= self.len {
            return false;
        }
        // Within the lent bytes, since `moved < len`.
        self.at = self.at.wrapping_add(moved);
        self.len -= moved;
        true
    }

    /// At most the first `limit` bytes.
    #[inline]
    #[must_use]
    pub fn first(self, limit: usize) -> Self {
        Self {
            len: self.len.min(limit),
            ..self
        }
    }

    /// The bytes as a slice, none for a null address.
    #[inline]
    #[must_use]
    pub fn bytes(self) -> &'a [u8] {
        if self.at.is_null() {
            return &[];
        }
        // SAFETY: the type's invariant: `len` readable bytes nothing writes for 'a.
        unsafe { std::slice::from_raw_parts(self.at, self.len) }
    }

    /// send(2) of the bytes.
    #[inline]
    #[must_use]
    pub fn send(self, fd: c_int, flags: c_int) -> isize {
        // SAFETY: the type's invariant: null, which the kernel refuses, or `len`
        // readable bytes.
        unsafe { libc::send(fd, self.at.cast(), self.len, flags) }
    }

    /// write(2) of the bytes.
    #[inline]
    #[must_use]
    pub fn write(self, fd: c_int) -> isize {
        // SAFETY: as in `send`.
        unsafe { libc::write(fd, self.at.cast(), self.len) }
    }

    /// sendto(2) of the bytes to `address`.
    #[inline]
    #[must_use]
    pub fn sendto(self, fd: c_int, flags: c_int, address: &SocketAddress) -> isize {
        let to = (&raw const address.addr).cast();
        // SAFETY: as in `send`; the address's storage, `len` bytes of which are its own.
        unsafe { libc::sendto(fd, self.at.cast(), self.len, flags, to, address.len) }
    }
}

/// The typed calls in the shape the tests' raw buffers and C-style TLS stubs have.
#[cfg(test)]
pub(crate) mod raw {
    use super::Fill;
    use crate::{Socket, SteadyMs, TlsIo, record_peek, record_recv};
    use std::ffi::{c_int, c_void};

    /// `tls` over a typed buffer.
    pub(crate) fn fills(tls: TlsIo) -> impl Fn(Socket<'_>, Fill<'_>) -> isize {
        move |socket, mut fill| {
            let slots = fill.slots();
            unsafe {
                tls(
                    socket.as_ptr().cast(),
                    slots.as_mut_ptr().cast(),
                    slots.len(),
                )
            }
        }
    }

    pub(crate) unsafe fn socket_record_recv(
        socket: Socket<'_>,
        buffer: *mut c_void,
        length: usize,
        flags: c_int,
        tls: TlsIo,
        now: SteadyMs,
    ) -> isize {
        let buffer = unsafe { Fill::from_raw(buffer, length) };
        record_recv(socket, buffer, flags, fills(tls), now)
    }

    pub(crate) unsafe fn socket_record_peek(
        socket: Socket<'_>,
        buffer: *mut c_void,
        length: usize,
        flags: c_int,
        tls: TlsIo,
    ) -> isize {
        let buffer = unsafe { Fill::from_raw(buffer, length) };
        record_peek(socket, buffer, flags, fills(tls))
    }
}
