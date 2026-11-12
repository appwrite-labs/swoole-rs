//! The coroutine socket's variable-length metadata, which its record owns: the error,
//! connect and TLS host strings, the proxies, the TLS context and the destructor
//! callback.
use crate::co_socket::Co;
use std::ffi::c_void;
use swoole_rs_corebase::{ProxyState, StringSlot};
use swoole_rs_tls::TlsContext;

/// A destructor callback, which gets the native socket.
pub type CloseHook = Box<dyn FnOnce(*mut c_void)>;

/// `repr(C)` for `tls`, first: the hooks system tests set it as `enable_ssl_encrypt`
/// does.
#[repr(C)]
pub struct SocketExtra {
    /// The socket's `SSLContext` owner (null without one): one reference, which an
    /// accepted socket shares with its server. Dropped after the `network::Socket`
    /// and its session (`swr_v1_hook_co_socket_release`).
    pub(crate) tls: Option<TlsContext>,
    pub(crate) strings: [StringSlot; 3],
    /// `set_dtor`'s callback.
    close_hook: Option<CloseHook>,
    /// `socks5_proxy` and `http_proxy`: each the socket's own, or none.
    pub(crate) proxies: [Option<Box<ProxyState>>; 2],
    /// The ring's host when the socket is a `UringSocket`, else none.
    #[cfg(feature = "iouring")]
    pub(crate) uring: Option<&'static crate::uring::Host>,
    /// The addresses of the TLS memory BIOs (read, write) `tls_session` gave a
    /// `UringSocket`'s session, which owns them. Only compared with the session's
    /// own, never dereferenced: stale once it is freed.
    #[cfg(feature = "iouring")]
    pub(crate) bio: [*mut openssl_sys::BIO; 2],
}
const _: () = assert!(
    core::mem::offset_of!(SocketExtra, tls) == 0,
    "the hooks system tests set the TLS owner at the record's extra pointer"
);

impl SocketExtra {
    /// A record's fresh metadata.
    pub(crate) fn boxed() -> Box<Self> {
        Box::new(Self {
            strings: std::array::from_fn(|_| StringSlot::default()),
            tls: None,
            close_hook: None,
            proxies: [None, None],
            #[cfg(feature = "iouring")]
            uring: None,
            #[cfg(feature = "iouring")]
            bio: [std::ptr::null_mut(); 2],
        })
    }
}

/// `UringSocket`'s constructors (`swoole_rs_coroutine_socket.h`): with `io_uring`, Rust runs
/// the socket's operations through the ring (`crate::uring_socket`).
///
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_co_socket_uring(socket: Co) {
    #[cfg(feature = "iouring")]
    if let Some(host) = crate::stdio::host() {
        socket.with_extra(|extra| extra.uring = host.uring);
    }
    #[cfg(not(feature = "iouring"))]
    let _ = socket;
}
/// `set_dtor`: `hook(owner)`, with the native socket, runs once, when the socket
/// closes or is destroyed.
pub fn set_close_hook(socket: Co, hook: CloseHook) {
    socket.with_extra(|extra| extra.close_hook = Some(hook));
}

/// The destructor callback, once: `close()`'s release and `~Socket`. The socket may be
/// gone after it returns.
///
pub(crate) fn run_close_hook(socket: Co) {
    let hook = socket.with_extra(|extra| extra.close_hook.take()).flatten();
    if let Some(hook) = hook {
        hook(socket.this());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_slot_is_the_extra_records_owner() {
        let mut extra = SocketExtra::boxed();
        let owner = TlsContext::new();
        let pointer = owner.as_ptr();
        extra.tls = Some(owner);
        let slot = unsafe { *(&raw const *extra).cast::<*const swoole_rs_tls::OwnedTlsContext>() };
        assert_eq!(
            slot, pointer,
            "the slot is the owner pointer native code reads"
        );
    }
}
