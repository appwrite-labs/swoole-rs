//! The buffer of one I/O call as a typed handle: [`Fill`] is bytes a read may write and
//! [`Drain`] bytes a write may read. A slice makes one safely; an entry point C calls
//! makes one from its pointer and length, once, and everything below it is safe. Each is
//! an address and a length, as the raw pair was, so a null address stays what the kernel
//! (`EFAULT`) or OpenSSL (nothing to move) made of it.
//!
//! The libc calls that move bytes take a buffer as two arguments, an address and a
//! count. Here they take an [`Out`] or [`In`] and a [`Count`] branded with the same
//! lifetime, which only one lending of a live buffer mints, so each call gets an address
//! and the count that buffer holds, and is declared `safe`.
use crate::net_types::{AddressStorage, Room, SocketAddress, StoredLength};
use crate::sys::ReadSlot;
use std::ffi::{c_char, c_int, c_void};
use std::io::IoSlice;
use std::marker::PhantomData;
use std::mem::MaybeUninit;

/// A syscall's byte count, or None for its -1.
fn count(n: isize) -> Option<usize> {
    usize::try_from(n).ok()
}

/// A lifetime `'id` that names one lending: invariant, so no other lending's tokens
/// unify with it.
type Brand<'id> = PhantomData<fn(&'id ()) -> &'id ()>;

/// Where a lent buffer starts, for a call that writes up to its [`Count`]'s `T`s there:
/// null, or writable for that many and reached through nothing else for `'id`. Only a
/// lending mints one, with its count.
#[repr(transparent)]
pub struct Out<'id, T = c_void>(*mut T, Brand<'id>);

/// Where a lent buffer starts, for a call that reads up to its [`Count`]'s `T`s there:
/// null, or readable for that many and not written for `'id`. Only a lending mints one,
/// with its count.
#[repr(transparent)]
pub struct In<'id, T = c_void>(*const T, Brand<'id>);

/// How many `T`s the [`Out`] or [`In`] of the same lending holds, as the call's count
/// type `N`: at most the buffer's length.
#[repr(transparent)]
pub struct Count<'id, N = usize>(N, Brand<'id>);

impl<'id> In<'id, u8> {
    /// The same bytes, as the untyped address a call takes.
    #[inline]
    #[must_use]
    pub fn bytes(self) -> In<'id> {
        In(self.0.cast(), PhantomData)
    }

    /// The bytes as the [`Drain`] they are, `count` of them (a C lender's negative
    /// count is none): what a callback C lends bytes to reads them through.
    #[inline]
    #[must_use]
    pub fn drain<N: TryInto<usize>>(self, count: Count<'id, N>) -> Drain<'id> {
        Drain {
            at: self.0,
            len: count.0.try_into().unwrap_or(0),
            lent: PhantomData,
        }
    }
}

impl<'id, S: ReadSlot> Out<'id, S> {
    /// The slots as the [`Fill`] they are, `count` of them (a C lender's negative
    /// count is none): what a callback C lends a buffer to writes it through.
    #[inline]
    #[must_use]
    pub fn fill<N: TryInto<usize>>(self, count: Count<'id, N>) -> Fill<'id, S> {
        Fill {
            at: self.0,
            len: count.0.try_into().unwrap_or(0),
            lent: PhantomData,
        }
    }

    /// The same one-byte slots, as the untyped address a call takes.
    #[inline]
    #[must_use]
    pub fn bytes(self) -> Out<'id> {
        Out(self.0.cast(), PhantomData)
    }

    /// The same one-byte slots, as the `char`s a call takes.
    #[inline]
    #[must_use]
    pub fn chars(self) -> Out<'id, c_char> {
        Out(self.0.cast(), PhantomData)
    }
}

/// A count type a call takes, which a length saturates to: never more than the length.
/// Sealed: only the integer types here.
pub trait Narrow: sealed::Sealed + Copy {
    fn narrow(length: usize) -> Self;
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for usize {}
    impl Sealed for i32 {}
    impl Sealed for u32 {}
    impl Sealed for u64 {}
}

impl Narrow for usize {
    #[inline]
    fn narrow(length: usize) -> Self {
        length
    }
}

macro_rules! narrow {
    ($($type:ty),*) => {$(
        impl Narrow for $type {
            #[inline]
            fn narrow(length: usize) -> Self {
                Self::try_from(length).unwrap_or(Self::MAX)
            }
        }
    )*};
}
narrow!(i32, u32, u64);

/// Lends `slots` to `call` as an [`Out`] and its [`Count`].
#[inline]
pub fn lend_out<T, N: Narrow, R>(
    slots: &mut [T],
    call: impl for<'id> FnOnce(Out<'id, T>, Count<'id, N>) -> R,
) -> R {
    call(
        Out(slots.as_mut_ptr(), PhantomData),
        Count(N::narrow(slots.len()), PhantomData),
    )
}

/// Lends `items` to `call` as an [`In`] and its [`Count`].
#[inline]
pub fn lend_in<T, N: Narrow, R>(
    items: &[T],
    call: impl for<'id> FnOnce(In<'id, T>, Count<'id, N>) -> R,
) -> R {
    call(
        In(items.as_ptr(), PhantomData),
        Count(N::narrow(items.len()), PhantomData),
    )
}

/// Lends `value`'s bytes to `call` as an [`In`] and its [`Count`].
#[inline]
pub(crate) fn lend_value<T, N: Narrow, R>(
    value: &T,
    call: impl for<'id> FnOnce(In<'id>, Count<'id, N>) -> R,
) -> R {
    call(
        In(std::ptr::from_ref(value).cast(), PhantomData),
        Count(N::narrow(size_of::<T>()), PhantomData),
    )
}

/// A lent buffer that starts with a header `H` and then holds its [`Count`]'s bytes,
/// for a call that writes the header and up to that many bytes after it: writable for
/// them and reached through nothing else for `'id`. Only a lending mints one, with its
/// count.
#[repr(transparent)]
pub struct HeadedOut<'id, H>(*mut H, Brand<'id>);

/// As [`HeadedOut`], for a call that reads the header and up to the count's bytes
/// after it: readable for them and not written for `'id`.
#[repr(transparent)]
pub struct HeadedIn<'id, H>(*const H, Brand<'id>);

impl Count<'_> {
    /// At most `limit` of the same lending's items.
    #[inline]
    #[must_use]
    pub fn at_most(self, limit: usize) -> Self {
        Self(self.0.min(limit), PhantomData)
    }
}

/// Lends `node`, a header `H` and then plain bytes, to `call` as a [`HeadedOut`] and the
/// count of bytes after the header; `None`, without calling it, when `node` is shorter
/// than the header. Any bytes are a value of `node`'s plain elements.
#[inline]
pub fn lend_headed_out<H, T: crate::sys::Pod, R>(
    node: &mut [T],
    call: impl for<'id> FnOnce(HeadedOut<'id, H>, Count<'id>) -> R,
) -> Option<R> {
    let after = size_of_val(node).checked_sub(size_of::<H>())?;
    Some(call(
        HeadedOut(node.as_mut_ptr().cast(), PhantomData),
        Count(after, PhantomData),
    ))
}

/// As [`lend_headed_out`], for a call that only reads `node`.
#[inline]
pub fn lend_headed_in<H, T: crate::sys::Pod, R>(
    node: &[T],
    call: impl for<'id> FnOnce(HeadedIn<'id, H>, Count<'id>) -> R,
) -> Option<R> {
    let after = size_of_val(node).checked_sub(size_of::<H>())?;
    Some(call(
        HeadedIn(node.as_ptr().cast(), PhantomData),
        Count(after, PhantomData),
    ))
}

/// Lends `vectors` to `call` as the iovec array they are ABI-compatible with (std's
/// guarantee for `IoSlice` on Unix), each naming readable bytes.
#[inline]
pub(crate) fn lend_vectors<R>(
    vectors: &[IoSlice<'_>],
    call: impl for<'id> FnOnce(In<'id, libc::iovec>, Count<'id, c_int>) -> R,
) -> R {
    call(
        In(vectors.as_ptr().cast(), PhantomData),
        Count(c_int::narrow(vectors.len()), PhantomData),
    )
}

// libc's. Each moves at most the count of `T`s its lending gives between the buffer
// and the descriptor, reads its integers, and keeps nothing; `recvfrom` writes at most
// the room's incoming size of `address` and the address's length into the room, and
// `sendto` reads the stored length of `address`. So they are declared `safe`.
#[expect(
    suspicious_runtime_symbol_definitions,
    reason = "`read` and `write` keep their C ABI: each token is a transparent address or \
              count, and the lending that mints them is what makes the call safe"
)]
unsafe extern "C" {
    safe fn read<'id>(fd: c_int, buf: Out<'id>, count: Count<'id>) -> isize;
    safe fn recv<'id>(fd: c_int, buf: Out<'id>, count: Count<'id>, flags: c_int) -> isize;
    safe fn pread<'id>(fd: c_int, buf: Out<'id>, count: Count<'id>, offset: libc::off_t) -> isize;
    safe fn recvfrom<'id>(
        fd: c_int,
        buf: Out<'id>,
        count: Count<'id>,
        flags: c_int,
        address: &mut AddressStorage,
        room: &mut Room,
    ) -> isize;
    safe fn write<'id>(fd: c_int, buf: In<'id>, count: Count<'id>) -> isize;
    safe fn send<'id>(fd: c_int, buf: In<'id>, count: Count<'id>, flags: c_int) -> isize;
    safe fn pwrite<'id>(fd: c_int, buf: In<'id>, count: Count<'id>, offset: libc::off_t) -> isize;
    safe fn sendto<'id>(
        fd: c_int,
        buf: In<'id>,
        count: Count<'id>,
        flags: c_int,
        address: &AddressStorage,
        length: StoredLength,
    ) -> isize;
    pub(crate) safe fn writev<'id>(
        fd: c_int,
        vectors: In<'id, libc::iovec>,
        count: Count<'id, c_int>,
    ) -> isize;
    pub(crate) safe fn poll<'id>(
        fds: Out<'id, libc::pollfd>,
        count: Count<'id, libc::nfds_t>,
        timeout: c_int,
    ) -> c_int;
    pub(crate) safe fn setsockopt<'id>(
        fd: c_int,
        level: c_int,
        name: c_int,
        value: In<'id>,
        length: Count<'id, libc::socklen_t>,
    ) -> c_int;
    /// Writes at most `max` bytes of `out`; reads `format` up to its NUL and `tm`.
    pub(crate) safe fn strftime<'id>(
        out: Out<'id, c_char>,
        max: Count<'id>,
        format: crate::sys::Text<'_>,
        tm: &libc::tm,
    ) -> usize;
    /// Fills `entry`, its strings in `strings`, and stores it (or null) in `found`.
    pub(crate) safe fn getgrnam_r<'id>(
        name: crate::sys::Text<'_>,
        entry: &mut libc::group,
        strings: Out<'id, c_char>,
        size: Count<'id>,
        found: &mut *mut libc::group,
    ) -> c_int;
    /// As `getgrnam_r`.
    #[cfg_attr(target_os = "netbsd", link_name = "__getpwnam_r50")]
    pub(crate) safe fn getpwnam_r<'id>(
        name: crate::sys::Text<'_>,
        entry: &mut libc::passwd,
        strings: Out<'id, c_char>,
        size: Count<'id>,
        found: &mut *mut libc::passwd,
    ) -> c_int;
    /// Writes `count` items of `size` bytes from `items` to an open stream.
    pub(crate) safe fn fwrite<'id>(
        items: In<'id>,
        size: OneByte,
        count: Count<'id>,
        stream: crate::Stream,
    ) -> usize;
    /// Flushes an open stream.
    pub(crate) safe fn fflush(stream: crate::Stream) -> c_int;
    /// Fills at most `count` bytes of `buf` from the kernel's CSPRNG.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub(crate) safe fn getrandom<'id>(
        buf: Out<'id>,
        count: Count<'id>,
        flags: std::ffi::c_uint,
    ) -> isize;
    /// Fills `count` bytes of `buf` from the CSPRNG.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    pub(crate) safe fn arc4random_buf<'id>(buf: Out<'id>, count: Count<'id>);
}

/// An item size of one byte, so a [`Count`] of bytes counts items: the only value.
#[repr(transparent)]
pub(crate) struct OneByte(usize);

impl OneByte {
    pub(crate) const SIZE: Self = Self(1);
}

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

    /// The address and count, for one call while this is borrowed: the type's
    /// invariant is the tokens'.
    #[inline]
    fn lend<R>(&mut self, call: impl for<'id> FnOnce(Out<'id>, Count<'id>) -> R) -> R {
        call(
            Out(self.at.cast(), PhantomData),
            Count(self.len, PhantomData),
        )
    }

    /// recv(2) into the bytes: how many it wrote, or None with errno set.
    #[inline]
    pub fn recv(&mut self, fd: c_int, flags: c_int) -> Option<usize> {
        count(self.lend(|buf, n| recv(fd, buf, n, flags)))
    }

    /// read(2) into the bytes, as [`Fill::recv`].
    #[inline]
    pub fn read(&mut self, fd: c_int) -> Option<usize> {
        count(self.lend(|buf, n| read(fd, buf, n)))
    }

    /// pread(2) into the bytes at `offset`, as [`Fill::recv`].
    #[inline]
    pub fn pread(&mut self, fd: c_int, offset: libc::off_t) -> Option<usize> {
        count(self.lend(|buf, n| pread(fd, buf, n, offset)))
    }

    /// recvfrom(2) into the bytes, the sender into `address`'s storage, and its length,
    /// given the room `address` has, into `address.len`.
    /// How many bytes it wrote, or None with errno set.
    #[inline]
    pub fn recvfrom_one(
        &mut self,
        fd: c_int,
        flags: c_int,
        address: &mut SocketAddress,
    ) -> Option<usize> {
        let mut room = address.room();
        let storage = &mut address.addr;
        let received = self.lend(|buf, n| recvfrom(fd, buf, n, flags, storage, &mut room));
        address.len = room.length();
        count(received)
    }

    /// `Socket::recvfrom`: recvfrom(2) into the bytes, and the peer into `address` and
    /// its length, retried on `EINTR` up to `SW_SOCKET_RETRY_COUNT` times.
    #[inline]
    pub fn recvfrom(
        &mut self,
        fd: c_int,
        flags: c_int,
        address: &mut SocketAddress,
    ) -> Option<usize> {
        let mut received = Some(0);
        for _ in 0..crate::constants::SW_SOCKET_RETRY_COUNT {
            received = self.recvfrom_one(fd, flags, address);
            if received.is_some() || crate::sys::errno() != libc::EINTR {
                break;
            }
        }
        received
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

    /// The address and count, for one call: the type's invariant is the tokens'.
    #[inline]
    fn lend<R>(self, call: impl for<'id> FnOnce(In<'id>, Count<'id>) -> R) -> R {
        call(
            In(self.at.cast(), PhantomData),
            Count(self.len, PhantomData),
        )
    }

    /// send(2) of the bytes: how many it sent, or None with errno set.
    #[inline]
    #[must_use]
    pub fn send(self, fd: c_int, flags: c_int) -> Option<usize> {
        count(self.lend(|buf, n| send(fd, buf, n, flags)))
    }

    /// write(2) of the bytes, as [`Drain::send`].
    #[inline]
    #[must_use]
    pub fn write(self, fd: c_int) -> Option<usize> {
        count(self.lend(|buf, n| write(fd, buf, n)))
    }

    /// pwrite(2) of the bytes at `offset`, as [`Drain::send`].
    #[inline]
    #[must_use]
    pub fn pwrite(self, fd: c_int, offset: libc::off_t) -> Option<usize> {
        count(self.lend(|buf, n| pwrite(fd, buf, n, offset)))
    }

    /// sendto(2) of the bytes to `address`, as [`Drain::send`].
    #[inline]
    #[must_use]
    pub fn sendto(self, fd: c_int, flags: c_int, address: &SocketAddress) -> Option<usize> {
        let length = address.stored();
        count(self.lend(|buf, n| sendto(fd, buf, n, flags, &address.addr, length)))
    }
}
