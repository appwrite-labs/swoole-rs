//! Shared wire records for network addresses used by sockets and server connections.
use core::ffi::c_int;

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
