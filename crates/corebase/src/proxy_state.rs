//! Shared HTTP/SOCKS proxy values and packet storage. Native objects only lend
//! references to these stable slots.
use crate::{CoreStringView, StringSlot, swr_v1_core_string_set, swr_v1_core_string_view};
use std::{cell::UnsafeCell, ptr};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ProxyFields {
    pub state: u8,
    pub dont_handshake: u8,
    pub version: u8,
    pub method: u8,
    pub dns_tunnel: u8,
    pub port: i32,
    pub target_port: i32,
    pub socket_type: i32,
}

const _: () = {
    assert!(
        size_of::<ProxyFields>() == 20,
        "native proxy scalar size changed"
    );
    assert!(
        std::mem::offset_of!(ProxyFields, port) == 8,
        "native proxy port offset changed"
    );
};
pub struct ProxyState {
    pub fields: UnsafeCell<ProxyFields>,
    strings: [StringSlot; 4],
    buffer: UnsafeCell<Option<Box<[u8; 512]>>>,
}
#[repr(C)]
pub struct PreparedProxy {
    pub owner: *mut ProxyState,
    pub fields: *mut ProxyFields,
    pub strings: [*const StringSlot; 4],
    pub buffer: *mut u8,
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_proxy_new(socks: bool) -> PreparedProxy {
    let state = Box::leak(Box::new(ProxyState {
        fields: UnsafeCell::new(ProxyFields::default()),
        strings: std::array::from_fn(|_| StringSlot::default()),
        buffer: UnsafeCell::new(socks.then(|| Box::new([0; 512]))),
    }));
    let buffer = state.buffer.get_mut().as_mut();
    PreparedProxy {
        buffer: buffer.map_or(ptr::null_mut(), |value| (&raw mut **value).cast()),
        fields: state.fields.get(),
        strings: state.strings.each_ref().map(ptr::from_ref),
        // Last, and with no call: a `&mut` reborrow, or moving a box, would end the views.
        owner: &raw mut *state,
    }
}
/// # Safety
/// owner is released once after all native field/string/packet views end.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_proxy_free(owner: *mut ProxyState) {
    if !owner.is_null() {
        // SAFETY: the caller returns its unique allocation after its views end.
        unsafe { drop(Box::from_raw(owner)) };
    }
}
impl ProxyState {
    /// The string at `index`, or an empty view past the four slots.
    /// # Safety
    /// self is live, externally synchronized and cannot change during the view.
    pub unsafe fn string(&self, index: usize) -> CoreStringView {
        let Some(slot) = self.strings.get(index) else {
            return CoreStringView {
                data: c"".as_ptr(),
                length: 0,
            };
        };
        // SAFETY: the caller lends the stable, synchronized inline string slot.
        unsafe { swr_v1_core_string_view(slot) }
    }
    /// Sets the string at `index` (0 host, 1 username, 2 password, 3 target host):
    /// `swr_v1_core_string_set`'s status, or -2 past the four slots.
    /// # Safety
    /// self is live and exclusively lent; no view of that string is in use.
    pub unsafe fn set_string(&self, index: usize, value: &[u8]) -> i32 {
        self.strings.get(index).map_or(-2, |slot| {
            // SAFETY: the caller lends the slot exclusively; `value` is readable.
            unsafe { swr_v1_core_string_set(slot, value.as_ptr().cast(), value.len()) }
        })
    }
    /// The packet, through `&raw mut **box` (what `Box::as_mut_ptr` expands to; that method is unstable on the fuzz toolchain), which leaves the pointers handed out
    /// before valid.
    /// # Safety
    /// self is a SOCKS owner, exclusively lent for packet writes.
    pub unsafe fn buffer(&self) -> *mut u8 {
        // SAFETY: the SOCKS owner contains the exclusive packet allocation.
        unsafe { &mut *self.buffer.get() }
            .as_mut()
            .map_or(ptr::null_mut(), |value| (&raw mut **value).cast())
    }
}
#[repr(C)]
pub struct ProxyCopyResult {
    pub status: i32,
    pub index: u32,
}
/// Native copy-assignment order, including its guarded allocation boundaries.
/// # Safety
/// destination is exclusive and source stable; both owners have the same kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_proxy_assign(
    destination: &ProxyState,
    source: &ProxyState,
) -> ProxyCopyResult {
    if ptr::eq(destination, source) {
        return ProxyCopyResult {
            status: 0,
            index: 0,
        };
    }
    // SAFETY: a leaf scalar snapshot before the string assignments.
    let fields = unsafe { *source.fields.get() };
    // SAFETY: private packet presence identifies the two proxy value layouts.
    let socks = unsafe { &*source.buffer.get() }.is_some();
    let slots = source.strings.iter().zip(&destination.strings);
    for (index, (from, to)) in slots.enumerate() {
        if (socks && index == 1) || (!socks && index == 0) {
            // SAFETY: snapshot the trailing fields that have not yet been assigned.
            let previous = unsafe { *destination.fields.get() };
            // SAFETY: scalar writes follow native member assignment order.
            unsafe {
                *destination.fields.get() = ProxyFields {
                    target_port: previous.target_port,
                    socket_type: previous.socket_type,
                    ..fields
                }
            };
        }
        // SAFETY: source string bytes remain unchanged through the assignment.
        let value = unsafe { swr_v1_core_string_view(from) };
        // SAFETY: destination is exclusive and the source view readable.
        let status = unsafe { swr_v1_core_string_set(to, value.data, value.length) };
        if status != 0 {
            return ProxyCopyResult {
                status,
                index: u32::try_from(index).unwrap_or(0),
            };
        }
    }
    // SAFETY: all string assignments finished; the trailing scalar members follow.
    unsafe { (*destination.fields.get()).target_port = fields.target_port };
    if socks {
        // SAFETY: trailing SOCKS socket type is assigned before its packet bytes.
        unsafe { (*destination.fields.get()).socket_type = fields.socket_type };
        // SAFETY: both same-kind owners contain stable disjoint packet arrays.
        let source_buffer = unsafe { source.buffer() };
        // SAFETY: destination packet storage is exclusively lent.
        let destination_buffer = unsafe { destination.buffer() };
        // SAFETY: both arrays contain512 bytes and distinct owners cannot overlap.
        unsafe { ptr::copy_nonoverlapping(source_buffer, destination_buffer, 512) };
    }
    ProxyCopyResult {
        status: 0,
        index: 0,
    }
}

/// Configure a fresh proxy after its protocol-specific credential validation.
/// # Safety
/// owner is exclusive; texts lends host/user/password terminated byte spans.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_proxy_configure(
    owner: *mut ProxyState,
    texts: *const CoreStringView,
    port: i32,
    authenticated: bool,
    socket_type: i32,
) -> ProxyCopyResult {
    // SAFETY: the fresh owner stays live through configuration.
    let owner = unsafe { &*owner };
    // SAFETY: three initialized input descriptors remain stable through assignment.
    let texts = unsafe { std::slice::from_raw_parts(texts, 3) };
    // SAFETY: packet ownership is immutable after construction and identifies SOCKS.
    let socks = unsafe { &*owner.buffer.get() }.is_some();
    for (index, (value, slot)) in texts.iter().zip(&owner.strings).enumerate() {
        if index != 0 && !authenticated {
            break;
        }
        // SAFETY: live exclusive slot and readable input span.
        let status = unsafe { swr_v1_core_string_set(slot, value.data, value.length) };
        if status != 0 {
            return ProxyCopyResult {
                status,
                index: u32::try_from(index).unwrap_or(0),
            };
        }
        if index == 0 {
            // SAFETY: scalar writes occur after the host and before credential assignment.
            let fields = unsafe { &mut *owner.fields.get() };
            fields.port = port;
            if socks {
                fields.dns_tunnel = 1;
                fields.socket_type = socket_type;
            }
        }
    }
    ProxyCopyResult {
        status: 0,
        index: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swr_v1_core_string_set;
    use std::ffi::CStr;
    #[test]
    fn proxy_copies_keep_scalar_packet_and_string_values_independent() {
        unsafe {
            for socks in [false, true] {
                let source = swr_v1_core_proxy_new(socks);
                let destination = swr_v1_core_proxy_new(socks);
                let inputs = [
                    CoreStringView {
                        data: c"proxy".as_ptr(),
                        length: 5,
                    },
                    CoreStringView {
                        data: c"user".as_ptr(),
                        length: 4,
                    },
                    CoreStringView {
                        data: c"pass".as_ptr(),
                        length: 4,
                    },
                ];
                assert_eq!(
                    swr_v1_core_proxy_configure(
                        source.owner,
                        inputs.as_ptr(),
                        1234,
                        true,
                        if socks { 1 } else { -1 }
                    )
                    .status,
                    0
                );
                (*source.fields).target_port = 4321;
                assert_eq!(
                    swr_v1_core_string_set(source.strings[3], c"target".as_ptr(), 6),
                    0
                );
                if socks {
                    std::ptr::write_bytes(source.buffer, 0x5a, 512);
                }
                assert_eq!(
                    swr_v1_core_proxy_assign(&*destination.owner, &*source.owner).status,
                    0
                );
                assert_eq!((*destination.fields).port, 1234);
                assert_eq!((*destination.fields).target_port, 4321);
                for index in 0..4 {
                    assert_eq!(
                        CStr::from_ptr(swr_v1_core_string_view(source.strings[index]).data),
                        CStr::from_ptr(swr_v1_core_string_view(destination.strings[index]).data)
                    );
                }
                assert_eq!(
                    swr_v1_core_string_set(source.strings[0], c"changed".as_ptr(), 7),
                    0
                );
                assert_eq!(
                    CStr::from_ptr(swr_v1_core_string_view(destination.strings[0]).data).to_bytes(),
                    b"proxy"
                );
                if socks {
                    assert_eq!(
                        std::slice::from_raw_parts(destination.buffer, 512),
                        &[0x5a; 512]
                    );
                    *source.buffer = 0;
                    assert_eq!(*destination.buffer, 0x5a);
                }
                swr_v1_core_proxy_free(source.owner);
                swr_v1_core_proxy_free(destination.owner);
            }
        }
    }
}
