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
use crate::reactor::{
    IovState, Plain, ReactorIov, swr_v1_reactor_iov_view, swr_v1_socket_iov_owned,
};
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
    impl Sealed for i16 {}
    impl Sealed for u16 {}
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
narrow!(i16, u16, i32, u32, u64);

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

/// Lends `value`'s bytes to `call` as an [`Out`] and its [`Count`]. Any bytes are a `T`.
#[inline]
pub(crate) fn lend_value_out<T: crate::sys::Pod, N: Narrow, R>(
    value: &mut T,
    call: impl for<'id> FnOnce(Out<'id>, &mut Count<'id, N>) -> R,
) -> (R, N) {
    let mut count = Count(N::narrow(size_of::<T>()), PhantomData);
    let out = call(
        Out(std::ptr::from_mut(value).cast(), PhantomData),
        &mut count,
    );
    (out, count.0)
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

impl<N: Copy> Count<'_, N> {
    /// The count, as the call that took it by `&mut` left it: what it wrote, or what
    /// is left of the lending past its moved address.
    #[inline]
    #[must_use]
    pub fn get(&self) -> N {
        self.0
    }
}

impl<N: Narrow + Ord> Count<'_, N> {
    /// At most `limit` of the same lending's items.
    #[inline]
    #[must_use]
    pub fn at_most(self, limit: usize) -> Self {
        Self(self.0.min(N::narrow(limit)), PhantomData)
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
    /// Fills the buffers that the count's vectors name, in order.
    pub(crate) safe fn readv<'id>(
        fd: c_int,
        vectors: Out<'id, ReadVec<'_>>,
        count: Count<'id, c_int>,
    ) -> isize;
    pub(crate) safe fn poll<'id>(
        fds: Out<'id, libc::pollfd>,
        count: Count<'id, libc::nfds_t>,
        timeout: c_int,
    ) -> c_int;
    /// Writes at most `count` ready events into `events`.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub(crate) safe fn epoll_wait<'id>(
        epfd: c_int,
        events: Out<'id, libc::epoll_event>,
        count: Count<'id, c_int>,
        timeout: c_int,
    ) -> c_int;
    /// Writes at most `length`'s bytes of `value`, and how many it stored into
    /// `length`, which is never more than the count it read there.
    pub(crate) safe fn getsockopt<'id>(
        fd: c_int,
        level: c_int,
        name: c_int,
        value: Out<'id>,
        length: &mut Count<'id, libc::socklen_t>,
    ) -> c_int;
    pub(crate) safe fn setsockopt<'id>(
        fd: c_int,
        level: c_int,
        name: c_int,
        value: In<'id>,
        length: Count<'id, libc::socklen_t>,
    ) -> c_int;
    /// Writes at most `max` bytes of `out`; reads `format` up to its NUL and `tm`.
    #[cfg(not(miri))]
    pub(crate) safe fn strftime<'id>(
        out: Out<'id, c_char>,
        max: Count<'id>,
        format: crate::sys::Text<'_>,
        tm: &libc::tm,
    ) -> usize;
    /// Fills `entry`, its lists in `buffer`, stores it (or null) in `found` and the
    /// resolver's error in `error`.
    #[cfg(target_os = "linux")]
    pub(crate) safe fn gethostbyname2_r<'id>(
        name: crate::sys::Text<'_>,
        family: c_int,
        entry: &mut libc::hostent,
        buffer: Out<'id, c_char>,
        length: Count<'id>,
        found: &mut *mut libc::hostent,
        error: &mut c_int,
    ) -> c_int;
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
pub struct OneByte(usize);

impl OneByte {
    pub const SIZE: Self = Self(1);
}

/// The size of each item of a buffer C lends as `size` × its [`Count`] bytes (`fread`,
/// `fwrite`). Only C mints one, as an export's parameter, with the buffer's [`Out`] or
/// [`In`] and its count of items.
#[repr(transparent)]
pub struct ItemSize<'id>(usize, Brand<'id>);

impl ItemSize<'_> {
    /// The size in bytes.
    #[inline]
    #[must_use]
    pub const fn get(&self) -> usize {
        self.0
    }
}

/// The bytes `size` × `count` items take, none when that overflows: no buffer is
/// that large.
fn item_bytes(size: &ItemSize<'_>, Count(count, _): Count<'_>) -> usize {
    size.0.checked_mul(count).unwrap_or(0)
}

impl<'id, S: ReadSlot> Out<'id, S> {
    /// The slots of `count` items of `size` bytes each, as the [`Fill`] they are.
    #[inline]
    #[must_use]
    pub fn fill_items(self, size: &ItemSize<'id>, count: Count<'id>) -> Fill<'id, S> {
        Fill {
            at: self.0,
            len: item_bytes(size, count),
            lent: PhantomData,
        }
    }
}

impl<'id> In<'id, u8> {
    /// The bytes of `count` items of `size` bytes each, as the [`Drain`] they are.
    #[inline]
    #[must_use]
    pub fn drain_items(self, size: &ItemSize<'id>, count: Count<'id>) -> Drain<'id> {
        Drain {
            at: self.0,
            len: item_bytes(size, count),
            lent: PhantomData,
        }
    }
}

/// Items C lends for a call that reads them and writes them back (`poll`'s
/// `pollfd`s): null, or as many initialized `T`s as its [`Count`] says, reached through
/// nothing else for `'id`. Only C mints one, as an export's parameter, with its count.
#[repr(transparent)]
pub struct InOut<'id, T>(*mut T, Brand<'id>);

impl<'id, T> InOut<'id, T> {
    /// The `count` items (a negative count is none), or `None` for a null address
    /// with a nonzero count.
    #[inline]
    #[must_use]
    pub fn items<N: TryInto<usize>>(self, count: Count<'id, N>) -> Option<&'id mut [T]> {
        let len = count.0.try_into().unwrap_or(0);
        if len == 0 {
            return Some(&mut []);
        }
        if self.0.is_null() {
            return None;
        }
        // SAFETY: the type's invariant: `len` initialized items, lent to this handle
        // alone for `'id`.
        Some(unsafe { std::slice::from_raw_parts_mut(self.0, len) })
    }
}

/// `len` bytes at `at` that a read may overwrite, lent for `'a`: null, or writable
/// and reached through nothing else meanwhile.
pub struct Fill<'a, S: ReadSlot = MaybeUninit<u8>> {
    at: *mut S,
    len: usize,
    lent: PhantomData<&'a mut [S]>,
}

impl<'a> Fill<'a, u8> {
    /// The initialized part of a string range. An empty range retains its source
    /// address, including a null address for an unallocated string at offset zero.
    #[inline]
    pub fn string_range(
        buffer: &'a mut crate::buffer::string::StringBuffer,
        start: usize,
        len: usize,
    ) -> Self {
        Self {
            at: buffer.as_mut_ptr().wrapping_add(start),
            len: len.min(buffer.len().saturating_sub(start)),
            lent: PhantomData,
        }
    }

    /// The address handed to a callback, which may distinguish null from empty.
    #[must_use]
    pub const fn as_ptr(&self) -> *const u8 {
        self.at
    }
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
    pub const fn len(&self) -> usize {
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

    /// recvfrom(2) into the bytes, the sender into `address`'s whole storage, and its
    /// length into `address.len`. The room is the whole storage whatever `len` held: a
    /// zero `len` would otherwise leave the sender unwritten.
    /// How many bytes it wrote, or None with errno set.
    #[inline]
    pub fn recvfrom_one(
        &mut self,
        fd: c_int,
        flags: c_int,
        address: &mut SocketAddress,
    ) -> Option<usize> {
        let mut room = Room::FULL;
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

/// What a call declared `safe` returns when it writes into an [`Out`]: how many of
/// that lending's first `N`s it wrote, or a negative failure. Only an extern block's
/// declaration gives a call this result, so the declaration is the promise.
#[repr(transparent)]
pub struct Wrote<'id, N = isize>(N, Brand<'id>);

/// The room past a buffer's content, lent to whatever fills it from the front: `len`
/// writable bytes at `at`, reached through nothing else for the lending, the first
/// `filled` of them written. Only [`lend_spare`] makes one, and only a write that put
/// bytes there (a read below, [`Spare::put`], or a call whose declared result is a
/// [`Wrote`]) moves `filled`, so the count a lending ends with is of written bytes. The
/// brand keeps two lendings' rooms from trading places.
pub struct Spare<'s> {
    at: *mut u8,
    filled: usize,
    len: usize,
    lent: Brand<'s>,
}

/// Lends `room` to `fill`: what it returned, and how many of `room`'s first bytes it
/// wrote.
#[inline]
pub fn lend_spare<R>(
    room: &mut [impl ReadSlot],
    fill: impl for<'s> FnOnce(&mut Spare<'s>) -> R,
) -> (R, usize) {
    let mut spare = Spare {
        at: room.as_mut_ptr().cast(),
        filled: 0,
        len: room.len(),
        lent: PhantomData,
    };
    let result = fill(&mut spare);
    (result, spare.filled)
}

impl<S: ReadSlot> Fill<'_, S> {
    /// The bytes lent to `fill` as a [`Spare`]: a C lender's room, which counts nothing.
    #[inline]
    pub fn spare<R>(&mut self, fill: impl for<'s> FnOnce(&mut Spare<'s>) -> R) -> R {
        fill(&mut Spare {
            at: self.at.cast(),
            filled: 0,
            len: self.len,
            lent: PhantomData,
        })
    }
}

impl Spare<'_> {
    /// How many bytes are left to fill.
    #[inline]
    #[must_use]
    pub fn room(&self) -> usize {
        self.len - self.filled
    }

    /// Counts `wrote` bytes past the written ones: what a read returned, which never
    /// counts past the room (`MSG_TRUNC` reports a datagram's whole length).
    #[inline]
    fn count(&mut self, wrote: Option<usize>) -> Option<usize> {
        if let Some(n) = wrote {
            self.filled += n.min(self.room());
        }
        wrote
    }

    /// The room left as a [`Fill`], for a read its own result counts.
    #[inline]
    fn rest(&mut self) -> Fill<'_> {
        Fill {
            at: self.at.wrapping_add(self.filled).cast(),
            len: self.room(),
            lent: PhantomData,
        }
    }

    /// The written bytes.
    #[inline]
    #[must_use]
    pub fn written(&mut self) -> &mut [u8] {
        if self.at.is_null() {
            return &mut [];
        }
        // SAFETY: the type's invariant: the first `filled` of the lent bytes are written,
        // and this borrows the only handle to them.
        unsafe { std::slice::from_raw_parts_mut(self.at, self.filled) }
    }

    /// Writes all of `data` after the written bytes, and counts it: the copy, or `None`,
    /// writing nothing, when it doesn't fit.
    #[inline]
    pub fn put(&mut self, data: &[u8]) -> Option<&mut [u8]> {
        let start = self.filled;
        let mut rest = self.rest();
        let to = rest.slots().get_mut(..data.len())?;
        to.write_copy_of_slice(data);
        self.filled += data.len();
        self.written().get_mut(start..)
    }

    /// The room to `call`, as an address and its count, and counts what its declared
    /// result says it wrote: that result.
    #[inline]
    pub fn lend<N: Narrow, R: Copy + TryInto<usize>>(
        &mut self,
        call: impl for<'id> FnOnce(Out<'id>, Count<'id, N>) -> Wrote<'id, R>,
    ) -> R {
        let at = self.at.wrapping_add(self.filled).cast();
        let Wrote(n, _) = call(
            Out(at, PhantomData),
            Count(N::narrow(self.room()), PhantomData),
        );
        self.count(n.try_into().ok());
        n
    }

    /// A ring's receive into the room, counted as [`Ring::recv`] vouches.
    #[inline]
    pub fn receive(&mut self, ring: &mut (impl Ring + ?Sized)) -> Option<usize> {
        let received = ring.recv(self.rest().slots());
        self.count(received)
    }

    /// The room to `write`, which nothing counts: a peek the receive after it
    /// overwrites.
    #[inline]
    pub fn uncounted<R>(&mut self, write: impl FnOnce(&mut [MaybeUninit<u8>]) -> R) -> R {
        write(self.rest().slots())
    }

    /// read(2) into the room, as [`Fill::read`].
    #[inline]
    pub fn read_fd(&mut self, fd: c_int) -> Option<usize> {
        let received = self.rest().read(fd);
        self.count(received)
    }

    /// Reads into `head` and then the remaining room. Returns the total byte count.
    #[inline]
    pub fn readv_after(&mut self, fd: c_int, head: &mut [u8]) -> Option<usize> {
        let header = head.len();
        let received = crate::sys::readv(
            fd,
            &mut [ReadVec::new(head), ReadVec::new(self.rest().slots())],
        );
        self.count(received.map(|total| total.saturating_sub(header)));
        received
    }

    /// recv(2) into the room, as [`Fill::recv`].
    #[inline]
    pub fn recv(&mut self, fd: c_int, flags: c_int) -> Option<usize> {
        let received = self.rest().recv(fd, flags);
        self.count(received)
    }

    /// One recvfrom(2) into the room, as [`Fill::recvfrom_one`].
    #[inline]
    pub fn recvfrom_one(
        &mut self,
        fd: c_int,
        flags: c_int,
        address: &mut SocketAddress,
    ) -> Option<usize> {
        let received = self.rest().recvfrom_one(fd, flags, address);
        self.count(received)
    }

    /// recvfrom(2) into the room, as [`Fill::recvfrom`].
    #[inline]
    pub fn recvfrom(
        &mut self,
        fd: c_int,
        flags: c_int,
        address: &mut SocketAddress,
    ) -> Option<usize> {
        let received = self.rest().recvfrom(fd, flags, address);
        self.count(received)
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

#[cfg(test)]
#[path = "../../../tests/modules/corebase/lent.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/modules/corebase/spare.rs"]
mod spare_tests;

/// One buffer a vectored read fills, as an `iovec`: slots that may not be written
/// yet, lent for `'b`.
#[repr(transparent)]
pub struct ReadVec<'b> {
    iov: libc::iovec,
    lent: PhantomData<&'b mut [MaybeUninit<u8>]>,
}

impl<'b> ReadVec<'b> {
    /// The descriptor while this vector's borrowed storage remains live.
    #[must_use]
    pub fn descriptor(&self) -> libc::iovec {
        self.iov
    }

    /// How many slots are left to fill.
    #[must_use]
    pub fn len(&self) -> usize {
        self.iov.iov_len
    }

    /// Whether no slots are left to fill.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.iov.iov_len == 0
    }

    /// The same slots, lent again while this is borrowed.
    pub fn reborrow(&mut self) -> ReadVec<'_> {
        ReadVec {
            iov: self.iov,
            lent: PhantomData,
        }
    }

    /// The slots as a [`Fill`], for a read while this is borrowed.
    pub fn fill(&mut self) -> Fill<'_> {
        Fill {
            at: self.iov.iov_base.cast(),
            len: self.iov.iov_len,
            lent: PhantomData,
        }
    }

    /// Steps past the first `n` slots, at most all of them.
    pub fn advance(&mut self, n: usize) {
        let n = n.min(self.iov.iov_len);
        self.iov.iov_base = self.iov.iov_base.wrapping_byte_add(n);
        self.iov.iov_len -= n;
    }

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

/// A ring's receives, whose results count the bytes they wrote.
///
/// # Safety
/// `Some(n)` from [`recv`](Self::recv) or [`readv`](Self::readv) means the call wrote
/// the first `n` bytes it was lent, in order across the vectors (all of them when they
/// hold fewer than `n`).
pub unsafe trait Ring {
    /// Receives into `room`.
    fn recv(&mut self, room: &mut [MaybeUninit<u8>]) -> Option<usize>;
    /// Receives into the vectors' slots, in order.
    fn readv(&mut self, vectors: &mut [ReadVec<'_>]) -> Option<usize>;
    /// Sends the vectors' bytes, in order.
    fn writev(&mut self, vectors: &[IoSlice<'_>]) -> Option<usize>;
}

/// Ordered rooms lent to vectored receives: a cursor over a copy of their descriptors,
/// and how many bytes from the first room's start the receives wrote. Only
/// [`transfer`](Self::transfer) moves either, by what a receive wrote.
pub struct SpareVectors<'s> {
    vectors: ReactorIov<'s>,
    filled: usize,
    lent: Brand<'s>,
}

/// Lends several rooms in order: the operation's result and how many bytes it wrote
/// across them.
pub fn lend_spare_vectors<'a, R>(
    rooms: impl IntoIterator<Item = &'a mut [MaybeUninit<u8>], IntoIter: ExactSizeIterator>,
    fill: impl for<'s> FnOnce(&mut SpareVectors<'s>) -> R,
) -> (R, usize) {
    let Some(vectors) = ReactorIov::rooms(rooms.into_iter().map(ReadVec::new)) else {
        // `IOVector`'s constructor throws here, which ended at this Rust frame.
        crate::fatal("the I/O vectors can't be built");
    };
    let mut out = SpareVectors {
        vectors,
        filled: 0,
        lent: PhantomData,
    };
    let result = fill(&mut out);
    (result, out.filled)
}

impl SpareVectors<'_> {
    /// One receive into the rooms past the written bytes, as
    /// [`swr_v1_socket_iov_owned`]: what it wrote, counted.
    pub fn transfer(
        &mut self,
        plain: Plain<'_>,
        tls: Option<&(
            impl Fn(&mut Spare<'_>) -> Option<usize>,
            impl Fn(Drain<'_>) -> Option<usize>,
        )>,
    ) -> Option<usize> {
        let received = swr_v1_socket_iov_owned(plain, &mut self.vectors, tls);
        self.filled += received.unwrap_or(0);
        received
    }

    /// The cursor: where the receives stopped and the descriptors left.
    #[must_use]
    pub fn state(&self) -> IovState {
        swr_v1_reactor_iov_view(&self.vectors)
    }
}
