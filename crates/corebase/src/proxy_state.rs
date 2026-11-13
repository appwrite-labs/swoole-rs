//! Shared HTTP/SOCKS proxy values and packet storage. Native objects only lend
//! references to these stable slots.
use crate::{CoreStringView, StringSlot};
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::ptr;

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
/// An HTTP or SOCKS5 proxy's values: its scalars, four strings (0 host, 1 username,
/// 2 password, 3 target host) and, for SOCKS5, its packet. Native views reach the
/// parts through [`PreparedProxy`]; Rust borrows them through the methods.
pub struct ProxyState {
    fields: Cell<ProxyFields>,
    strings: RefCell<[StringSlot; 4]>,
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
    let state = Box::leak(ProxyState::new(socks));
    PreparedProxy {
        buffer: state
            .packet
            .as_ref()
            .map_or(ptr::null_mut(), |packet| RefCell::as_ptr(packet).cast()),
        fields: state.fields.as_ptr(),
        strings: std::array::from_fn(|index| {
            RefCell::as_ptr(&state.strings)
                .cast::<StringSlot>()
                .wrapping_add(index)
                .cast_const()
        }),
        // Last, and with no call: a `&mut` reborrow would end the views.
        owner: &raw mut *state,
    }
}
/// Frees the proxy `swr_v1_core_proxy_new` leaked, or nothing for null: native code
/// hands its `owner` back once, after its field, string and packet views end.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_proxy_free(owner: Option<Box<ProxyState>>) {
    drop(owner);
}
#[repr(C)]
pub struct ProxyCopyResult {
    pub status: i32,
    pub index: u32,
}
const COPIED: ProxyCopyResult = ProxyCopyResult {
    status: 0,
    index: 0,
};
impl ProxyState {
    /// An empty proxy; a SOCKS5 one (`socks`) has a packet.
    #[must_use]
    pub fn new(socks: bool) -> Box<Self> {
        Box::new(Self {
            fields: Cell::new(ProxyFields::default()),
            strings: RefCell::new(std::array::from_fn(|_| StringSlot::default())),
            packet: socks.then(|| Box::new(RefCell::new([0; 512]))),
        })
    }
    /// A copy of the scalars.
    pub fn fields(&self) -> ProxyFields {
        self.fields.get()
    }
    /// Changes the scalars.
    pub fn update(&self, change: impl FnOnce(&mut ProxyFields)) {
        let mut fields = self.fields.get();
        change(&mut fields);
        self.fields.set(fields);
    }
    /// The scalars, for a native view.
    pub fn fields_ptr(&self) -> *mut ProxyFields {
        self.fields.as_ptr()
    }
    /// The string at `index` (NUL-terminated past its end), or empty past the four.
    ///
    /// # Panics
    /// While a string is being set.
    pub fn string(&self, index: usize) -> Ref<'_, [u8]> {
        Ref::map(self.strings.borrow(), |strings| {
            strings.get(index).map_or(&[][..], StringSlot::bytes)
        })
    }
    /// [`Self::string`] and the NUL after it.
    ///
    /// # Panics
    /// While a string is being set.
    pub fn string_with_nul(&self, index: usize) -> Ref<'_, [u8]> {
        Ref::map(self.strings.borrow(), |strings| {
            strings
                .get(index)
                .map_or(&[0][..], StringSlot::bytes_with_nul)
        })
    }
    /// Sets the string at `index`: 0, or -2 when the allocation fails or past the
    /// four.
    ///
    /// # Panics
    /// While a string is borrowed.
    pub fn set_string(&self, index: usize, value: &[u8]) -> i32 {
        self.strings
            .borrow_mut()
            .get_mut(index)
            .map_or(-2, |slot| slot.set(value))
    }
    /// The SOCKS5 packet, none for an HTTP proxy.
    ///
    /// # Panics
    /// While the packet is borrowed.
    pub fn packet(&self) -> Option<RefMut<'_, [u8; 512]>> {
        self.packet.as_deref().map(RefCell::borrow_mut)
    }
    /// Native copy-assignment order, including its guarded allocation boundaries;
    /// both have the same kind.
    pub fn assign(&self, source: &Self) -> ProxyCopyResult {
        if ptr::eq(self, source) {
            return COPIED;
        }
        let fields = source.fields();
        let socks = source.packet.is_some();
        for index in 0..4 {
            if (socks && index == 1) || (!socks && index == 0) {
                // The trailing members are assigned after the strings.
                self.update(|current| {
                    *current = ProxyFields {
                        target_port: current.target_port,
                        socket_type: current.socket_type,
                        ..fields
                    };
                });
            }
            let status = self.set_string(index, &source.string(index));
            if status != 0 {
                return ProxyCopyResult {
                    status,
                    index: u32::try_from(index).unwrap_or(0),
                };
            }
        }
        self.update(|current| current.target_port = fields.target_port);
        if socks {
            self.update(|current| current.socket_type = fields.socket_type);
            if let (Some(mut to), Some(from)) = (self.packet(), source.packet()) {
                *to = *from;
            }
        }
        COPIED
    }
    /// Configures a fresh proxy after its protocol's credential check: the host,
    /// then the port (and for SOCKS5 the tunnel and socket type), then the
    /// credentials when `authenticated`.
    pub fn configure(
        &self,
        texts: [&[u8]; 3],
        port: i32,
        authenticated: bool,
        socket_type: i32,
    ) -> ProxyCopyResult {
        for (index, value) in texts.into_iter().enumerate() {
            if index != 0 && !authenticated {
                break;
            }
            let status = self.set_string(index, value);
            if status != 0 {
                return ProxyCopyResult {
                    status,
                    index: u32::try_from(index).unwrap_or(0),
                };
            }
            if index == 0 {
                let socks = self.packet.is_some();
                self.update(|fields| {
                    fields.port = port;
                    if socks {
                        fields.dns_tunnel = 1;
                        fields.socket_type = socket_type;
                    }
                });
            }
        }
        COPIED
    }
}
/// [`ProxyState::assign`] for native code.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_proxy_assign(
    destination: &ProxyState,
    source: &ProxyState,
) -> ProxyCopyResult {
    destination.assign(source)
}

/// [`ProxyState::configure`] for native code.
/// # Safety
/// texts lends host/user/password terminated byte spans.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_proxy_configure(
    owner: &ProxyState,
    texts: &[CoreStringView; 3],
    port: i32,
    authenticated: bool,
    socket_type: i32,
) -> ProxyCopyResult {
    let texts = texts.map(|view| {
        // SAFETY: each view lends `length` readable bytes for the call.
        unsafe { std::slice::from_raw_parts(view.data.cast::<u8>(), view.length) }
    });
    owner.configure(texts, port, authenticated, socket_type)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proxy_copies_keep_scalar_packet_and_string_values_independent() {
        for socks in [false, true] {
            let source = ProxyState::new(socks);
            let destination = ProxyState::new(socks);
            let texts: [&[u8]; 3] = [b"proxy", b"user", b"pass"];
            let kind = if socks { 1 } else { -1 };
            assert_eq!(source.configure(texts, 1234, true, kind).status, 0);
            source.update(|fields| fields.target_port = 4321);
            assert_eq!(source.set_string(3, b"target"), 0);
            if let Some(mut packet) = source.packet() {
                packet.fill(0x5a);
            }
            assert_eq!(destination.assign(&source).status, 0);
            assert_eq!(destination.fields().port, 1234);
            assert_eq!(destination.fields().target_port, 4321);
            for index in 0..4 {
                assert_eq!(*source.string(index), *destination.string(index));
            }
            assert_eq!(source.set_string(0, b"changed"), 0);
            assert_eq!(&*destination.string(0), b"proxy");
            if socks {
                assert_eq!(*destination.packet().unwrap(), [0x5a; 512]);
                source.packet().unwrap()[0] = 0;
                assert_eq!(destination.packet().unwrap()[0], 0x5a);
            }
        }
    }

    #[test]
    fn native_views_see_rust_writes() {
        let prepared = swr_v1_core_proxy_new(true);
        let owner = unsafe { &*prepared.owner };
        let texts = [c"host", c"", c""].map(|text| CoreStringView {
            data: text.as_ptr(),
            length: text.count_bytes(),
        });
        let configured = unsafe { swr_v1_core_proxy_configure(owner, &texts, 80, false, 1) };
        assert_eq!(configured.status, 0);
        assert_eq!(unsafe { (*prepared.fields).port }, 80);
        let host = unsafe { crate::swr_v1_core_string_view(&*prepared.strings[0]) };
        assert_eq!(unsafe { std::ffi::CStr::from_ptr(host.data) }, c"host");
        owner.packet().unwrap()[7] = 9;
        assert_eq!(unsafe { *prepared.buffer.add(7) }, 9);
        swr_v1_core_proxy_free(Some(unsafe { Box::from_raw(prepared.owner) }));
    }
}
