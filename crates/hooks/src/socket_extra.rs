//! Rust-owned variable-length coroutine socket metadata. Native string members
//! borrow these stable slots; snapshot copies own independent Rust bytes.
use std::ffi::c_void;
use std::ptr;
use swoole_rs_corebase::StringSlot;

pub struct SocketExtra {
    pub(crate) strings: [StringSlot; 3],
    /// The socket's `SSLContext` owner (null without one); native code shares the
    /// context and keeps it alive while this is set.
    pub(crate) tls: *const c_void,
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
