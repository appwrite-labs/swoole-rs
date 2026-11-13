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
use std::ffi::{c_char, c_int};
use std::marker::PhantomData;
use std::ptr::NonNull;

/// An `ares_channel`'s state. Only an [`OwnedChannel`] lends one, and only to its
/// own thread: c-ares does no locking here.
#[repr(C)]
pub struct Channel {
    _opaque: [u8; 0],
    _thread: PhantomData<*mut u8>,
}
/// An initialised channel, which `ares_destroy` frees when it is dropped. C writes it
/// as an `ares_channel`: whoever initialises one promises its callbacks' userdata
/// outlives it and that no callback drops it.
#[repr(transparent)]
pub struct OwnedChannel(NonNull<Channel>);
impl std::ops::Deref for OwnedChannel {
    type Target = Channel;
    fn deref(&self) -> &Channel {
        // SAFETY: the channel stays live until this handle drops it.
        unsafe { self.0.as_ref() }
    }
}
impl Drop for OwnedChannel {
    fn drop(&mut self) {
        // SAFETY: this handle's channel, destroyed once; the initialiser promised its
        // callbacks' userdata is still live as destruction calls them.
        unsafe { ares_destroy(self.0.as_ptr()) };
    }
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
unsafe extern "C" {
    pub fn ares_library_init(flags: c_int) -> c_int;
    pub fn ares_library_cleanup();
    /// Reads and writes whichever of the channel's own descriptors match, and runs its
    /// callbacks synchronously, which the initialiser vouched for.
    pub safe fn ares_process_fd(channel: &Channel, read: c_int, write: c_int);
    /// Ends the channel's queries, running their callbacks synchronously.
    pub safe fn ares_cancel(channel: &Channel);
    fn ares_destroy(channel: *mut Channel);
    /// The status's description: one of c-ares' static strings (`unknown` for a code it
    /// doesn't know), never null. It reads only the code.
    pub safe fn ares_strerror(code: c_int) -> *const c_char;
    /// The shim's constant table (`sdk.c`): a `static const` of this layout, which it
    /// only returns the address of.
    pub safe fn swrs_cares_metadata() -> &'static Metadata;
}
