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

impl SocketAddress {
    /// The storage's size, the most a call reads or writes of it.
    pub const CAPACITY: libc::socklen_t = crate::sys::socklen::<AddressStorage>();

    fn storage(&self) -> (*const libc::sockaddr, libc::socklen_t) {
        ((&raw const self.addr).cast(), self.len.min(Self::CAPACITY))
    }

    /// `connect(2)` of `fd` to this address.
    #[must_use]
    pub fn connect(&self, fd: c_int) -> c_int {
        let (storage, length) = self.storage();
        // SAFETY: the storage holds the `length` readable bytes given.
        unsafe { libc::connect(fd, storage, length) }
    }

    /// `bind(2)` of `fd` to this address.
    #[must_use]
    pub fn bind(&self, fd: c_int) -> c_int {
        let (storage, length) = self.storage();
        // SAFETY: the storage holds the `length` readable bytes given.
        unsafe { libc::bind(fd, storage, length) }
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
        self.len = Self::CAPACITY;
        // SAFETY: the storage is writable for the `len` given.
        unsafe { libc::getsockname(fd, (&raw mut self.addr).cast(), &raw mut self.len) }
    }

    /// `getpeername(2)` of `fd` into this address and its length.
    pub fn peer_name(&mut self, fd: c_int) -> c_int {
        self.len = Self::CAPACITY;
        // SAFETY: the storage is writable for the `len` given.
        unsafe { libc::getpeername(fd, (&raw mut self.addr).cast(), &raw mut self.len) }
    }
}

/// `swoole::PacketLength`: the bytes a length function measures, and the header
/// length it reports.
#[repr(C)]
pub struct PacketLength {
    pub buf: *const u8,
    pub buf_size: u32,
    pub header_len: u32,
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
