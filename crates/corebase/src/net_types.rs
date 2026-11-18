//! Shared wire records for network addresses used by sockets and server connections.
use core::ffi::c_int;

/// `network::Address`'s storage: the largest of the address families it holds.
// `repr(C)`: libc's socket address storage, which `connect`, `bind` and `getsockname` read and fill.
#[repr(C)]
#[derive(Clone, Copy)]
pub union AddressStorage {
    pub ss: libc::sockaddr,
    pub inet_v4: libc::sockaddr_in,
    pub inet_v6: libc::sockaddr_in6,
    pub un: libc::sockaddr_un,
    /// The largest member's bytes, as C lends an address of any family.
    pub bytes: [u8; size_of::<libc::sockaddr_un>()],
}

/// `network::Address`: the storage, its length and its `swSocketType`.
#[derive(Clone, Copy)]
pub struct SocketAddress {
    pub addr: AddressStorage,
    pub len: libc::socklen_t,
    pub kind: c_int,
}

impl Default for SocketAddress {
    /// The empty address, `network::Address{}`.
    fn default() -> Self {
        crate::sys::zeroed()
    }
}

impl AddressStorage {
    /// Copy this address into the prefix of a native socket address storage record.
    #[cfg(not(target_vendor = "apple"))]
    pub fn write_to(&self, storage: &mut libc::sockaddr_storage) {
        const {
            assert!(
                size_of::<AddressStorage>() <= size_of::<libc::sockaddr_storage>(),
                "a native storage record must fit the address"
            );
        };
        // SAFETY: the destination has room for the whole address, at any alignment,
        // and each address member starts with the native socket address header.
        unsafe {
            std::ptr::from_mut(storage)
                .cast::<Self>()
                .write_unaligned(*self);
        }
    }

    /// The family, which every member starts with.
    #[must_use]
    pub const fn family(&self) -> libc::sa_family_t {
        // Every address struct starts as `sockaddr` does, so this is `ss.sa_family`.
        self.v4().sin_family
    }

    /// The storage as its member `T`.
    const fn member<T: Member>(&self) -> &T {
        // SAFETY: `T` is one of the union's members (`Member` is sealed to them), which
        // all start at its start and are plain bytes, so any of them reads whatever was
        // stored, for as long as the union is borrowed.
        unsafe { &*std::ptr::from_ref(self).cast::<T>() }
    }

    /// The storage as an IPv4 address.
    #[must_use]
    pub const fn v4(&self) -> &libc::sockaddr_in {
        self.member()
    }

    /// The storage as an IPv6 address.
    #[must_use]
    pub const fn v6(&self) -> &libc::sockaddr_in6 {
        self.member()
    }

    /// The storage as a Unix address.
    #[must_use]
    pub const fn unix(&self) -> &libc::sockaddr_un {
        self.member()
    }

    /// The storage as its largest member's bytes, whichever family it holds.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; size_of::<libc::sockaddr_un>()] {
        self.member()
    }

    /// Runs `edit` on the storage as a Unix address and stores the result whole, so
    /// bytes `edit` leaves alone keep their value.
    pub fn edit_unix<R>(&mut self, edit: impl FnOnce(&mut libc::sockaddr_un) -> R) -> R {
        let mut un = *self.unix();
        let result = edit(&mut un);
        self.un = un;
        result
    }
}

/// A member of [`AddressStorage`]: private, so only the union's own member types.
trait Member {}
impl Member for libc::sockaddr {}
impl Member for libc::sockaddr_in {}
impl Member for libc::sockaddr_in6 {}
impl Member for libc::sockaddr_un {}
impl Member for [u8; size_of::<libc::sockaddr_un>()] {}

impl SocketAddress {
    /// The storage's size, the most a call reads or writes of it.
    pub const CAPACITY: libc::socklen_t = crate::sys::socklen::<AddressStorage>();

    /// The address a C caller lends as a `sockaddr` and its length: as many of its
    /// bytes as the storage holds.
    #[must_use]
    pub fn from_bytes(address: &[u8]) -> Self {
        let mut bytes = [0; size_of::<libc::sockaddr_un>()];
        for (slot, byte) in bytes.iter_mut().zip(address) {
            *slot = *byte;
        }
        Self {
            addr: AddressStorage { bytes },
            len: crate::sys::socklen::<libc::sockaddr_un>()
                .min(libc::socklen_t::try_from(address.len()).unwrap_or(Self::CAPACITY)),
            kind: 0,
        }
    }

    /// The length a call reads of the storage: `len`, at most the storage's size.
    pub(crate) fn stored(&self) -> StoredLength {
        StoredLength(self.len.min(Self::CAPACITY))
    }

    /// `connect(2)` of `fd` to this address: whether it connected, else errno says why
    /// not.
    #[must_use]
    pub fn connect(&self, fd: c_int) -> bool {
        connect(fd, &self.addr, self.stored()) == 0
    }

    /// `bind(2)` of `fd` to this address: whether it bound, else errno says why not.
    #[must_use]
    pub fn bind(&self, fd: c_int) -> bool {
        bind(fd, &self.addr, self.stored()) == 0
    }

    /// `sendto(2)` of `data` to this address: how many bytes it sent, or `None` with
    /// errno set.
    #[must_use]
    pub fn sendto(&self, fd: c_int, data: &[u8], flags: c_int) -> Option<usize> {
        crate::lent::Drain::new(data).sendto(fd, flags, self)
    }

    /// `recvfrom(2)` into `buffer`, the sender's address and its length into this one:
    /// how many bytes it received, or `None` with errno set.
    pub fn recvfrom(
        &mut self,
        fd: c_int,
        buffer: &mut [impl crate::sys::ReadSlot],
        flags: c_int,
    ) -> Option<usize> {
        crate::lent::Fill::new(buffer).recvfrom_one(fd, flags, self)
    }

    /// `getsockname(2)` of `fd` into this address and its length.
    ///
    /// # Errors
    /// The syscall's errno.
    pub fn local_name(&mut self, fd: c_int) -> std::io::Result<()> {
        self.named(fd, getsockname)
    }

    /// `getpeername(2)` of `fd` into this address and its length.
    ///
    /// # Errors
    /// The syscall's errno.
    pub fn peer_name(&mut self, fd: c_int) -> std::io::Result<()> {
        self.named(fd, getpeername)
    }

    /// `accept(2)` on `fd`, the peer's address and its length into this one: the new
    /// descriptor, or `None` with errno set.
    pub fn accept(&mut self, fd: c_int) -> Option<std::os::fd::OwnedFd> {
        let mut room = Room::FULL;
        let result = accept(fd, &mut self.addr, &mut room);
        self.len = room.0;
        result
    }

    /// `accept4(2)` on `fd` with `flags`, as [`Self::accept`]: the accepted
    /// descriptor, or `None` with `errno` set.
    #[cfg(target_os = "linux")]
    pub fn accept4(&mut self, fd: c_int, flags: c_int) -> Option<std::os::fd::OwnedFd> {
        let mut room = Room::FULL;
        let result = accept4(fd, &mut self.addr, &mut room, flags);
        self.len = room.0;
        result
    }

    /// `name(fd, ...)` into the whole storage, its length going to `len`.
    fn named(&mut self, fd: c_int, name: NameCall) -> std::io::Result<()> {
        let mut room = Room::FULL;
        let result = name(fd, &mut self.addr, &mut room);
        let error = std::io::Error::last_os_error();
        self.len = room.0;
        if result == 0 { Ok(()) } else { Err(error) }
    }
}

/// A length of an [`AddressStorage`]'s bytes, at most its size: only
/// [`SocketAddress::stored`] makes one.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub(crate) struct StoredLength(libc::socklen_t);

// libc's. Each reads the first `length` bytes of `address`, which `StoredLength`
// keeps within it, and its integers, and keeps nothing, so they are declared `safe`.
unsafe extern "C" {
    safe fn connect(fd: c_int, address: &AddressStorage, length: StoredLength) -> c_int;
    safe fn bind(fd: c_int, address: &AddressStorage, length: StoredLength) -> c_int;
}

/// The room `getsockname`, `getpeername`, `accept` and `recvfrom` are given, in and
/// out: the whole storage going in (only [`Room::FULL`] makes one), the address's
/// length coming out. Each value goes to one call and is then read, never passed again.
#[repr(transparent)]
pub(crate) struct Room(libc::socklen_t);

impl Room {
    pub(crate) const FULL: Self = Self(SocketAddress::CAPACITY);

    /// The length the call stored.
    pub(crate) fn length(self) -> libc::socklen_t {
        self.0
    }
}

/// `getsockname` or `getpeername`.
type NameCall = extern "C" fn(c_int, &mut AddressStorage, &mut Room) -> c_int;

// libc's. Each writes at most the room's incoming size (the whole storage) of
// `address`, and the address's length into the room, and keeps nothing.
unsafe extern "C" {
    safe fn accept(
        fd: c_int,
        address: &mut AddressStorage,
        room: &mut Room,
    ) -> Option<std::os::fd::OwnedFd>;
    safe fn getsockname(fd: c_int, address: &mut AddressStorage, room: &mut Room) -> c_int;
    safe fn getpeername(fd: c_int, address: &mut AddressStorage, room: &mut Room) -> c_int;
    #[cfg(target_os = "linux")]
    safe fn accept4(
        fd: c_int,
        address: &mut AddressStorage,
        room: &mut Room,
        flags: c_int,
    ) -> Option<std::os::fd::OwnedFd>;
}

/// `swoole::PacketLength`: the bytes a length function measures, and the header
/// length it reports. Rust makes one only over a borrowed slice, so `buf` holds
/// `buf_size` readable bytes; native code that passes one promises the same.
pub struct PacketLength<'a> {
    buf: *const u8,
    buf_size: u32,
    pub header_len: u32,
    data: core::marker::PhantomData<&'a [u8]>,
}

impl<'a> PacketLength<'a> {
    /// A packet over `data` (its first `u32::MAX` bytes when longer), with no header
    /// length yet.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            buf: data.as_ptr(),
            buf_size: u32::try_from(data.len()).unwrap_or(u32::MAX),
            header_len: 0,
            data: core::marker::PhantomData,
        }
    }

    /// The bytes to measure.
    #[must_use]
    #[inline]
    pub fn bytes(&self) -> &'a [u8] {
        if self.buf.is_null() {
            return &[];
        }
        // SAFETY: `buf` holds `buf_size` readable bytes for `'a` (the type's invariant).
        unsafe { core::slice::from_raw_parts(self.buf, self.buf_size as usize) }
    }
}
