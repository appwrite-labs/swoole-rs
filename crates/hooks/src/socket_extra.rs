//! The coroutine socket's variable-length metadata, which its record owns: the error,
//! connect and TLS host strings, the proxies, the TLS context and the destructor
//! callback.
use crate::co_socket::CoSocket;
use std::ffi::c_void;
use std::ptr;
use swoole_rs_corebase::{ProxyState, StringSlot, swr_v1_core_proxy_free};
use swoole_rs_tls::{OwnedTlsContext, swr_v1_tls_owner_free};

/// A destructor callback: `hook(arg, socket)`.
pub type CloseHook = unsafe extern "C" fn(*mut c_void, *mut c_void);

/// `repr(C)` for `tls`, first: the hooks system tests set it as `enable_ssl_encrypt`
/// does.
#[repr(C)]
pub struct SocketExtra {
    /// The socket's `SSLContext` owner (null without one): one reference, which an
    /// accepted socket shares with its server and the record releases.
    pub(crate) tls: *const OwnedTlsContext,
    pub(crate) strings: [StringSlot; 3],
    /// `set_dtor`'s callback, its argument and the native `Socket` it is passed.
    close_hook: Option<(CloseHook, *mut c_void, *mut c_void)>,
    /// `socks5_proxy` and `http_proxy`: each an owner, or null.
    pub(crate) proxies: [*mut ProxyState; 2],
    /// The ring's host when the socket is a `UringSocket`, else null.
    #[cfg(feature = "iouring")]
    pub(crate) uring: *const c_void,
    /// A `UringSocket`'s TLS memory BIOs (read, write), which its session owns.
    #[cfg(feature = "iouring")]
    pub(crate) bio: [*mut c_void; 2],
}
const _: () = assert!(
    core::mem::offset_of!(SocketExtra, tls) == 0,
    "the hooks system tests set the TLS owner at the record's extra pointer"
);

impl Drop for SocketExtra {
    fn drop(&mut self) {
        // SAFETY: the record's own reference, released once; the network::Socket and
        // its session went first (`swr_v1_hook_co_socket_release`).
        unsafe { swr_v1_tls_owner_free(self.tls) };
        for proxy in self.proxies {
            // SAFETY: the socket's own proxy, released once.
            unsafe { swr_v1_core_proxy_free(proxy) };
        }
    }
}
impl SocketExtra {
    /// A record's fresh metadata, which [`SocketExtra::free`] releases.
    pub(crate) fn alloc() -> *mut Self {
        Box::into_raw(Box::new(Self {
            strings: std::array::from_fn(|_| StringSlot::default()),
            tls: ptr::null(),
            close_hook: None,
            proxies: [ptr::null_mut(); 2],
            #[cfg(feature = "iouring")]
            uring: ptr::null(),
            #[cfg(feature = "iouring")]
            bio: [ptr::null_mut(); 2],
        }))
    }

    /// Release what [`SocketExtra::alloc`] made, or nothing for null.
    ///
    /// # Safety
    /// `owner` is null or from `alloc`, released once.
    pub(crate) unsafe fn free(owner: *mut Self) {
        if !owner.is_null() {
            // SAFETY: as the caller promises.
            drop(unsafe { Box::from_raw(owner) });
        }
    }
}

/// `UringSocket`'s constructors (`swoole_rs_coroutine_socket.h`): with `io_uring`, Rust runs
/// the socket's operations through the ring (`crate::uring_socket`).
///
/// # Safety
/// `socket` is a live record whose extra record is set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_uring(socket: *mut CoSocket) {
    #[cfg(feature = "iouring")]
    if let Some(host) = crate::stdio::host() {
        // SAFETY: as the caller promises.
        let extra = unsafe { (*socket).extra };
        // SAFETY: the record's live extra record.
        unsafe { (*extra).uring = host.uring };
    }
    #[cfg(not(feature = "iouring"))]
    let _ = socket;
}
/// `set_dtor`: `hook(arg, owner)` runs once, when the socket closes or is destroyed.
///
/// # Safety
/// `socket` is a live record whose extra record is set; `hook` may be called with
/// `arg` and `owner` until the socket is destroyed.
pub unsafe fn set_close_hook(
    socket: *mut CoSocket,
    owner: *mut c_void,
    hook: CloseHook,
    arg: *mut c_void,
) {
    // SAFETY: as the caller promises.
    let extra = unsafe { (*socket).extra };
    // SAFETY: the record's live extra record.
    unsafe { (*extra).close_hook = Some((hook, arg, owner)) };
}

/// The destructor callback, once: `close()`'s release and `~Socket`. The socket may be
/// gone after it returns.
///
/// # Safety
/// `socket` is a live record whose extra record is set or null.
pub unsafe fn run_close_hook(socket: *mut CoSocket) {
    // SAFETY: as the caller promises.
    let extra = unsafe { (*socket).extra };
    if extra.is_null() {
        return;
    }
    // SAFETY: the record's live extra record; the callback is taken before it runs.
    let hook = unsafe { (*extra).close_hook.take() };
    if let Some((hook, arg, owner)) = hook {
        // SAFETY: the callback set_close_hook's caller lent until now.
        unsafe { hook(arg, owner) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_slot_is_the_extra_records_owner() {
        let extra = SocketExtra::alloc();
        let owner = swoole_rs_tls::swr_v1_tls_owner_new(swoole_rs_tls::swr_v1_tls_warning).owner;
        unsafe { (*extra).tls = owner };
        assert_eq!(unsafe { (*extra).tls }, owner);
        unsafe { SocketExtra::free(extra) };
    }
}
