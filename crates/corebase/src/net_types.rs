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

impl SocketAddress {
    /// An empty address: zeroed storage, length and type.
    #[must_use]
    pub const fn zeroed() -> Self {
        // SAFETY: every member of the storage is a C sockaddr of integers and byte
        // arrays, and the length and type are integers: all valid zeroed.
        unsafe { core::mem::zeroed() }
    }
}

/// `swoole::Protocol`, whole (`split_by_eof` is a C++ `bool`).
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
    pub on_package: *mut c_void,
    pub get_package_length: *mut c_void,
    pub get_package_length_size: *mut c_void,
}
