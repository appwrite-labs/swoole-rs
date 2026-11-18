//! `swoole::Protocol`: how a stream socket's bytes split into packages, and the
//! callbacks that measure and hand on each one. Upstream's layout, which C++ shares.
use core::ffi::{c_int, c_void};
use swoole_rs_corebase::buffer::string::StringBuffer;
use swoole_rs_corebase::lent::Fill;
use swoole_rs_corebase::net_types::PacketLength;
pub use swoole_rs_corebase::reactor::Socket;

/// `Protocol::onPackage`: hands on one whole package read from the socket. A caller
/// passes the protocol the socket reads with, which the callback may ignore. The
/// protocol stays a pointer: the callback may run PHP code, whose `set()` can rewrite
/// it meanwhile. Only Rust calls it, so it takes the package as Rust lends it.
pub type OnPackage = fn(*const Protocol, Socket<'_>, Package<'_>) -> c_int;

/// `Protocol::get_package_length`: the length of the package in the packet's
/// `buf_size` readable bytes at `buf`, 0 while incomplete, -1 when bad. A caller
/// passes a copy of the protocol the socket reads with, taken before the call, so
/// PHP code the callback runs may `set()` the protocol meanwhile; the callback may
/// rely on the private data stored beside it there. The optional header size comes from
/// the current server connection. The owner is a reference the caller keeps
/// beside the protocol, which the callback drops, or `None` where it keeps none. Only
/// Rust calls it.
pub type GetPackageLength =
    fn(&Protocol, Option<u8>, Option<std::rc::Rc<dyn core::any::Any>>, &mut PacketLength) -> isize;

/// The dynamic header size for a session's WebSocket status and HTTP/2 flag.
pub type GetPackageLengthSize = fn(swoole_rs_corebase::WebSocketStatus, bool) -> u8;

/// A whole package in a receive buffer, lent to its handler: `len` bytes at `data`
/// (null for none), readable and writable for `'a`. It stays a pointer: the handler
/// may run PHP that frees the buffer, after which it reads the package no more.
pub struct Package<'a>(Fill<'a, u8>);

impl<'a> Package<'a> {
    /// `len` bytes of `buffer` from `start`, cut to what it holds.
    pub fn of(buffer: &'a mut StringBuffer, start: usize, len: usize) -> Self {
        Self(Fill::string_range(buffer, start, len))
    }

    /// Where the package starts, or null for none.
    #[must_use]
    pub const fn as_ptr(&self) -> *const u8 {
        self.0.as_ptr()
    }

    /// The package's length.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the package is none (null) or holds no bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The package's bytes, borrowed from it.
    pub fn bytes(&mut self) -> &mut [u8] {
        self.0.slots()
    }
}

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
