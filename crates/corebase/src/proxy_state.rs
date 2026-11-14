//! Shared HTTP/SOCKS proxy values and packet storage. Native objects only lend
//! references to these stable slots.
use crate::StringSlot;
use std::cell::{Cell, Ref, RefCell, RefMut};

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
/// 2 password, 3 target host) and, for SOCKS5, its packet, which Rust borrows through
/// the methods.
pub struct ProxyState {
    fields: Cell<ProxyFields>,
    strings: RefCell<[StringSlot; 4]>,
    packet: Option<Box<RefCell<[u8; 512]>>>,
}
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
    /// Sets the string at `index`: false when the allocation fails or past the four.
    ///
    /// # Panics
    /// While a string is borrowed.
    pub fn set_string(&self, index: usize, value: &[u8]) -> bool {
        self.strings
            .borrow_mut()
            .get_mut(index)
            .is_some_and(|slot| slot.set(value) == 0)
    }
    /// The SOCKS5 packet, none for an HTTP proxy.
    ///
    /// # Panics
    /// While the packet is borrowed.
    pub fn packet(&self) -> Option<RefMut<'_, [u8; 512]>> {
        self.packet.as_deref().map(RefCell::borrow_mut)
    }
    /// Configures a fresh proxy after its protocol's credential check: the host,
    /// then the port (and for SOCKS5 the tunnel and socket type), then the
    /// credentials when `authenticated`.
    ///
    /// # Errors
    ///
    /// The index of the string whose allocation failed.
    pub fn configure(
        &self,
        texts: [&[u8]; 3],
        port: i32,
        authenticated: bool,
        socket_type: i32,
    ) -> Result<(), usize> {
        for (index, value) in texts.into_iter().enumerate() {
            if index != 0 && !authenticated {
                break;
            }
            if !self.set_string(index, value) {
                return Err(index);
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configure_sets_the_scalars_and_strings() {
        let owner = ProxyState::new(true);
        assert_eq!(owner.configure([b"host", b"", b""], 80, false, 1), Ok(()));
        assert_eq!(owner.fields().port, 80);
        assert_eq!(&*owner.string(0), b"host");
        assert_eq!(&*owner.string_with_nul(0), b"host\0");
        owner.packet().unwrap()[7] = 9;
        assert_eq!(owner.packet().unwrap()[7], 9);
    }
}
