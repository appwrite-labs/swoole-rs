//! `swoole::Protocol`: how a stream socket's bytes split into packages, and the
//! callbacks that measure and hand on each one. Upstream's layout, which C++ shares.
use core::ffi::{c_int, c_void};
use swoole_rs_corebase::net_types::{PacketLength, RecvData};
pub use swoole_rs_reactor::Socket;

/// `Protocol::onPackage`: hands on one whole package read from the socket. A caller
/// passes the live protocol the socket reads with and a package of it; the callback
/// may rely on the private data stored beside it in the protocol and on the socket
/// being of the kind that protocol was set up for (a port's connection, a client's).
/// The protocol stays a pointer: the callback may run PHP code, whose `set()` can
/// rewrite it meanwhile.
pub type OnPackage = unsafe extern "C-unwind" fn(*const Protocol, Socket<'_>, &RecvData) -> c_int;

/// `Protocol::get_package_length`: the length of the package in the packet's
/// `buf_size` readable bytes at `buf`, 0 while incomplete, -1 when bad. The protocol,
/// socket and private data are as for [`OnPackage`]; the socket is `None` (null) where
/// a measure has none.
pub type GetPackageLength =
    unsafe extern "C" fn(*const Protocol, Option<Socket<'_>>, &mut PacketLength) -> isize;

/// `Protocol::get_package_length_size`: a socket's length-header size, which may rely
/// on the socket being of the kind the protocol was set up for.
pub type GetPackageLengthSize = unsafe extern "C" fn(Socket<'_>) -> u8;

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

const _: () = assert!(
    size_of::<Protocol>() == 64 && core::mem::offset_of!(Protocol, on_package) == 40,
    "swoole::Protocol's layout moved"
);

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
