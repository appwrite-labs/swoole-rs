//! Rust-owned variable-length coroutine socket metadata. Native string members
//! borrow these stable slots; snapshot copies own independent Rust bytes.
use crate::co_socket::CoSocket;
use std::ffi::c_void;
use std::ptr;
use swoole_rs_corebase::StringSlot;

/// A destructor callback: `hook(arg, socket)`.
pub type CloseHook = unsafe extern "C" fn(*mut c_void, *mut c_void);

pub struct SocketExtra {
    pub(crate) strings: [StringSlot; 3],
    /// The socket's `SSLContext` owner (null without one); native code shares the
    /// context and keeps it alive while this is set.
    pub(crate) tls: *const c_void,
    /// `set_dtor`'s callback, its argument and the native `Socket` it is passed.
    close_hook: Option<(CloseHook, *mut c_void, *mut c_void)>,
    /// The ring's host when the socket is a `UringSocket`, else null.
    #[cfg(feature = "iouring")]
    pub(crate) uring: *const c_void,
    /// A `UringSocket`'s TLS memory BIOs (read, write), which its session owns.
    #[cfg(feature = "iouring")]
    pub(crate) bio: [*mut c_void; 2],
}
#[repr(C)]
pub struct PreparedSocketExtra {
    pub(crate) owner: *mut SocketExtra,
    error: *const StringSlot,
    connect_host: *const StringSlot,
    ssl_host_name: *const StringSlot,
    tls: *mut *const c_void,
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_socket_extra_new() -> PreparedSocketExtra {
    let owner = Box::into_raw(Box::new(SocketExtra {
        strings: std::array::from_fn(|_| StringSlot::default()),
        tls: ptr::null(),
        close_hook: None,
        #[cfg(feature = "iouring")]
        uring: ptr::null(),
        #[cfg(feature = "iouring")]
        bio: [ptr::null_mut(); 2],
    }));
    // SAFETY: the fresh Box remains allocated until its enclosing native owner drops.
    let strings = unsafe { ptr::addr_of!((*owner).strings).cast::<StringSlot>() };
    PreparedSocketExtra {
        owner,
        error: strings,
        // SAFETY: all three inline slots exist at their stable Box address.
        connect_host: unsafe { strings.add(1) },
        // SAFETY: the third slot is within the same live array.
        ssl_host_name: unsafe { strings.add(2) },
        // SAFETY: the fresh Box's own field.
        tls: unsafe { &raw mut (*owner).tls },
    }
}
/// `UringSocket`'s constructors (`swoole_socket_impl.h`): with `io_uring`, Rust runs
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

/// # Safety
/// owner came from `extra_new` and is released once after every borrowed view ends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_socket_extra_free(owner: *mut SocketExtra) {
    if !owner.is_null() {
        // SAFETY: the caller returns its unique Box after all views are destroyed.
        unsafe { drop(Box::from_raw(owner)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_slot_is_the_extra_records_owner() {
        let prepared = swr_v1_hook_socket_extra_new();
        let owner = 0x40 as *const c_void;
        unsafe { *prepared.tls = owner };
        assert_eq!(unsafe { (*prepared.owner).tls }, owner);
        unsafe { swr_v1_hook_socket_extra_free(prepared.owner) };
    }
}
