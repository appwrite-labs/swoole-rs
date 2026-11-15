//! Shared wire records for network addresses used by sockets and server connections.
use core::ffi::{c_int, c_void};

/// `network::Address`'s storage: the largest of the address families it holds.
#[repr(C)]
#[derive(Clone, Copy)]
pub union AddressStorage {
    pub ss: libc::sockaddr,
    pub inet_v4: libc::sockaddr_in,
    pub inet_v6: libc::sockaddr_in6,
    pub un: libc::sockaddr_un,
}

/// `network::Address`: the storage, its length and its `swSocketType`.
#[repr(C)]
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

    /// The storage as its member `T`, to fill.
    const fn member_mut<T: Member>(&mut self) -> &mut T {
        // SAFETY: as for `member`, and any bytes written are a value of every member.
        unsafe { &mut *std::ptr::from_mut(self).cast::<T>() }
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

    /// The storage as a Unix address, to fill.
    pub const fn unix_mut(&mut self) -> &mut libc::sockaddr_un {
        self.member_mut()
    }
}

/// A member of [`AddressStorage`]: private, so only the union's own member types.
trait Member {}
impl Member for libc::sockaddr {}
impl Member for libc::sockaddr_in {}
impl Member for libc::sockaddr_in6 {}
impl Member for libc::sockaddr_un {}

impl SocketAddress {
    /// The storage's size, the most a call reads or writes of it.
    pub const CAPACITY: libc::socklen_t = crate::sys::socklen::<AddressStorage>();

    fn storage(&self) -> (*const libc::sockaddr, libc::socklen_t) {
        ((&raw const self.addr).cast(), self.stored().0)
    }

    /// The length a call reads of the storage: `len`, at most the storage's size.
    fn stored(&self) -> StoredLength {
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

    /// `sendto(2)` of `data` to this address.
    #[must_use]
    pub fn sendto(&self, fd: c_int, data: &[u8], flags: c_int) -> isize {
        let (storage, length) = self.storage();
        // SAFETY: `data` is readable for its length, the storage for `length` bytes.
        unsafe { libc::sendto(fd, data.as_ptr().cast(), data.len(), flags, storage, length) }
    }

    /// `recvfrom(2)` into `buffer`, the sender's address and its length into this one.
    pub fn recvfrom(
        &mut self,
        fd: c_int,
        buffer: &mut [impl crate::sys::ReadSlot],
        flags: c_int,
    ) -> isize {
        self.len = Self::CAPACITY;
        let (data, size) = (buffer.as_mut_ptr().cast(), buffer.len());
        let storage = (&raw mut self.addr).cast();
        // SAFETY: `buffer` is writable for its length, the storage for the `len` given.
        unsafe { libc::recvfrom(fd, data, size, flags, storage, &raw mut self.len) }
    }

    /// `getsockname(2)` of `fd` into this address and its length.
    pub fn local_name(&mut self, fd: c_int) -> c_int {
        self.named(fd, getsockname)
    }

    /// `getpeername(2)` of `fd` into this address and its length.
    pub fn peer_name(&mut self, fd: c_int) -> c_int {
        self.named(fd, getpeername)
    }

    /// `accept(2)` on `fd`, the peer's address and its length into this one: the new
    /// fd, or -1 with errno set.
    pub fn accept(&mut self, fd: c_int) -> c_int {
        self.named(fd, accept)
    }

    /// `name(fd, ...)` into the whole storage, its length going to `len`.
    fn named(&mut self, fd: c_int, name: NameCall) -> c_int {
        let mut room = Room::FULL;
        let result = name(fd, &mut self.addr, &mut room);
        self.len = room.0;
        result
    }
}

/// A length of an [`AddressStorage`]'s bytes, at most its size: only
/// [`SocketAddress::stored`] makes one.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct StoredLength(libc::socklen_t);

// libc's. Each reads the first `length` bytes of `address`, which `StoredLength`
// keeps within it, and its integers, and keeps nothing, so they are declared `safe`.
unsafe extern "C" {
    safe fn connect(fd: c_int, address: &AddressStorage, length: StoredLength) -> c_int;
    safe fn bind(fd: c_int, address: &AddressStorage, length: StoredLength) -> c_int;
}

/// The room `getsockname`/`getpeername` are given, in and out: the storage's whole size
/// going in (only [`Room::FULL`] makes one), the address's length coming out. Each value
/// goes to one call and is then read, never passed again.
#[repr(transparent)]
struct Room(libc::socklen_t);

impl Room {
    const FULL: Self = Self(SocketAddress::CAPACITY);
}

/// `getsockname`, `getpeername` or `accept`.
type NameCall = extern "C" fn(c_int, &mut AddressStorage, &mut Room) -> c_int;

// libc's. Each writes at most the room's incoming size (the whole storage) of
// `address`, and the address's length into the room, and keeps nothing.
unsafe extern "C" {
    safe fn accept(fd: c_int, address: &mut AddressStorage, room: &mut Room) -> c_int;
    safe fn getsockname(fd: c_int, address: &mut AddressStorage, room: &mut Room) -> c_int;
    safe fn getpeername(fd: c_int, address: &mut AddressStorage, room: &mut Room) -> c_int;
}

/// `swoole::PacketLength`: the bytes a length function measures, and the header
/// length it reports. Rust makes one only over a borrowed slice, so `buf` holds
/// `buf_size` readable bytes; native code that passes one promises the same.
#[repr(C)]
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

/// `Protocol::onPackage`: hands on one whole package, a `const RecvData *`, read from
/// the `network::Socket *`. A caller passes the live protocol, socket and package; the
/// callback may also rely on the private data stored beside it in the protocol.
pub type OnPackage =
    unsafe extern "C-unwind" fn(*const Protocol, *mut c_void, *const c_void) -> c_int;

/// `Protocol::get_package_length`: the length of the package in the packet's
/// `buf_size` readable bytes at `buf`, 0 while incomplete, -1 when bad. The protocol,
/// socket and private data are as for [`OnPackage`].
pub type GetPackageLength =
    unsafe extern "C" fn(*const Protocol, *mut c_void, &mut PacketLength) -> isize;

/// `Protocol::get_package_length_size`: a live socket's length-header size.
pub type GetPackageLengthSize = unsafe extern "C" fn(*mut c_void) -> u8;

/// `swoole::Protocol`, whole (`split_by_eof` is a C++ `bool`). The callbacks are
/// nullable function pointers, upstream's layout.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Protocol {
    pub split_by_eof: bool,
    pub package_eof: [u8; 8],
    pub package_eof_len: u8,
    /// A C `char`.
    pub package_length_type: u8,
    pub package_length_size: u8,
    pub package_length_offset: u16,
    pub package_body_offset: u16,
    pub package_max_length: u32,
    pub private_data_1: *mut c_void,
    pub private_data_2: *mut c_void,
    pub on_package: Option<OnPackage>,
    pub get_package_length: Option<GetPackageLength>,
    pub get_package_length_size: Option<GetPackageLengthSize>,
}

impl Protocol {
    /// `Protocol{}`: no framing, no callbacks.
    pub const EMPTY: Self = Self {
        split_by_eof: false,
        package_eof: [0; 8],
        package_eof_len: 0,
        package_length_type: 0,
        package_length_size: 0,
        package_length_offset: 0,
        package_body_offset: 0,
        package_max_length: 0,
        private_data_1: core::ptr::null_mut(),
        private_data_2: core::ptr::null_mut(),
        on_package: None,
        get_package_length: None,
        get_package_length_size: None,
    };
}
