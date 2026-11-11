//! Shared HTTP/SOCKS proxy values and packet storage. Native objects only lend
//! references to these stable slots.
use crate::{CoreStringView, StringSlot, swr_v1_core_string_set, swr_v1_core_string_view};
use std::{
    cell::{Cell, RefCell, RefMut},
    ptr,
};

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
/// A proxy's values, which one thread (it is `!Sync`) reads and sets through
/// `&self`. The native head's views write the same cells between Rust calls.
pub struct ProxyState {
    pub fields: Cell<ProxyFields>,
    strings: [StringSlot; 4],
    /// A SOCKS5 proxy's packet; none for an HTTP proxy.
    packet: Option<Box<RefCell<[u8; 512]>>>,
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
        fields: Cell::default(),
        strings: std::array::from_fn(|_| StringSlot::default()),
        packet: socks.then(|| Box::new(RefCell::new([0; 512]))),
    }));
    PreparedProxy {
        buffer: state.buffer(),
        fields: state.fields.as_ptr(),
        strings: state.strings.each_ref().map(ptr::from_ref),
        // Last: a `&mut` reborrow, or moving a box, would end the views.
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
    /// A copy of the string at `index` (0 host, 1 username, 2 password, 3 target
    /// host), or empty past the four slots.
    #[must_use]
    pub fn string(&self, index: usize) -> Vec<u8> {
        self.strings.get(index).map_or_else(Vec::new, |slot| {
            // SAFETY: no reference into a slot outlives a call on this `!Sync` owner.
            let view = unsafe { swr_v1_core_string_view(slot) };
            // SAFETY: the view names `length` initialized bytes, copied at once.
            unsafe { std::slice::from_raw_parts(view.data.cast::<u8>(), view.length) }.to_vec()
        })
    }
    /// Sets the string at `index`: `swr_v1_core_string_set`'s status, or -2 past
    /// the four slots.
    pub fn set_string(&self, index: usize, value: &[u8]) -> i32 {
        self.strings.get(index).map_or(-2, |slot| {
            // SAFETY: no reference into a slot outlives a call on this `!Sync` owner.
            unsafe { swr_v1_core_string_set(slot, value.as_ptr().cast(), value.len()) }
        })
    }
    /// A SOCKS5 proxy's packet, lent while the guard lives.
    #[must_use]
    pub fn packet(&self) -> Option<RefMut<'_, [u8; 512]>> {
        self.packet.as_ref().map(|packet| packet.borrow_mut())
    }
    /// The packet's address, for the native views and sends; null for an HTTP proxy.
    #[must_use]
    pub fn buffer(&self) -> *mut u8 {
        self.packet
            .as_ref()
            .map_or(ptr::null_mut(), |packet| packet.as_ptr().cast())
    }
}
#[repr(C)]
pub struct ProxyCopyResult {
    pub status: i32,
    pub index: u32,
}
/// Native copy-assignment order, including its guarded allocation boundaries.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_proxy_assign(
    destination: &ProxyState,
    source: &ProxyState,
) -> ProxyCopyResult {
    if ptr::eq(destination, source) {
        return ProxyCopyResult {
            status: 0,
            index: 0,
        };
    }
    let fields = source.fields.get();
    let socks = source.packet.is_some();
    let slots = source.strings.iter().zip(&destination.strings);
    for (index, (from, to)) in slots.enumerate() {
        if (socks && index == 1) || (!socks && index == 0) {
            // The scalar members declared before this string.
            destination.fields.update(|previous| ProxyFields {
                target_port: previous.target_port,
                socket_type: previous.socket_type,
                ..fields
            });
        }
        // SAFETY: the source's slot, unchanged through the assignment.
        let value = unsafe { swr_v1_core_string_view(from) };
        // SAFETY: a distinct owner's slot, and the source view readable.
        let status = unsafe { swr_v1_core_string_set(to, value.data, value.length) };
        if status != 0 {
            return ProxyCopyResult {
                status,
                index: u32::try_from(index).unwrap_or(0),
            };
        }
    }
    // The trailing scalar members, then the packet.
    destination.fields.update(|previous| ProxyFields {
        target_port: fields.target_port,
        socket_type: if socks {
            fields.socket_type
        } else {
            previous.socket_type
        },
        ..previous
    });
    if let (Some(from), Some(mut to)) = (source.packet(), destination.packet()) {
        *to = *from;
    }
    ProxyCopyResult {
        status: 0,
        index: 0,
    }
}

/// Configure a fresh proxy after its protocol-specific credential validation.
/// # Safety
/// owner is live; texts lends host/user/password terminated byte spans.
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
    let socks = owner.packet.is_some();
    for (index, (value, slot)) in texts.iter().zip(&owner.strings).enumerate() {
        if index != 0 && !authenticated {
            break;
        }
        // SAFETY: the owner's slot and a readable input span.
        let status = unsafe { swr_v1_core_string_set(slot, value.data, value.length) };
        if status != 0 {
            return ProxyCopyResult {
                status,
                index: u32::try_from(index).unwrap_or(0),
            };
        }
        if index == 0 {
            // After the host and before the credentials.
            owner.fields.update(|mut fields| {
                fields.port = port;
                if socks {
                    fields.dns_tunnel = 1;
                    fields.socket_type = socket_type;
                }
                fields
            });
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
                assert_eq!((*source.owner).set_string(0, b"changed"), 0);
                assert_eq!((*destination.owner).string(0), b"proxy");
                assert_eq!((*source.owner).string(0), b"changed");
                if socks {
                    assert_eq!(
                        std::slice::from_raw_parts(destination.buffer, 512),
                        &[0x5a; 512]
                    );
                    *source.buffer = 0;
                    assert_eq!(*destination.buffer, 0x5a);
                    assert_eq!(
                        (*destination.owner).packet().map(|packet| packet[0]),
                        Some(0x5a)
                    );
                }
                swr_v1_core_proxy_free(source.owner);
                swr_v1_core_proxy_free(destination.owner);
            }
        }
    }
}
