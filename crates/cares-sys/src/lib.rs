//! Raw system c-ares calls. The SDK shim adapts its versioned options and server
//! node layouts; it contains no resolver state or scheduling policy.
//! Empty without `libcares`, so ordinary builds need no c-ares installation.
#![cfg(feature = "libcares")]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
use std::ffi::{c_char, c_int, c_void};

#[repr(C)]
pub struct Channel {
    _opaque: [u8; 0],
}
/// Constants read from the actual SDK, including its server API generation.
#[repr(C)]
pub struct Metadata {
    pub library_all: c_int,
    pub success: c_int,
    pub cancelled: c_int,
    pub timeout: c_int,
    pub timeout_ms: c_int,
    pub tries: c_int,
    pub socket_state: c_int,
    pub lookups: c_int,
    pub server_api: c_int,
}
pub type SocketState = unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int);
pub type HostCallback = unsafe extern "C" fn(*mut c_void, c_int, c_int, *mut libc::hostent);
unsafe extern "C" {
    pub fn ares_library_init(flags: c_int) -> c_int;
    pub fn ares_library_cleanup();
    pub fn ares_gethostbyname(
        channel: *mut Channel,
        name: *const c_char,
        family: c_int,
        callback: Option<HostCallback>,
        data: *mut c_void,
    );
    pub fn ares_process_fd(channel: *mut Channel, read: c_int, write: c_int);
    pub fn ares_cancel(channel: *mut Channel);
    pub fn ares_destroy(channel: *mut Channel);
    pub fn ares_strerror(code: c_int) -> *const c_char;
    pub fn swrs_cares_metadata() -> *const Metadata;
    pub fn swrs_cares_init(
        channel: *mut *mut Channel,
        lookups: *const c_char,
        timeout: c_int,
        tries: c_int,
        callback: Option<SocketState>,
        data: *mut c_void,
        mask: c_int,
    ) -> c_int;
    pub fn swrs_cares_server(channel: *mut Channel, host: *const c_char, port: u16);
}
