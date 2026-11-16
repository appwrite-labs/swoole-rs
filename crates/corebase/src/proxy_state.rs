//! Shared HTTP/SOCKS proxy values and packet storage. Native objects only lend
//! references to these stable slots.
use crate::StringSlot;
use std::cell::Cell;

#[derive(Clone, Copy, Default)]
pub struct ProxyFields {
    pub state: u8,
    pub dont_handshake: bool,
    pub version: u8,
    pub method: u8,
    pub dns_tunnel: u8,
    pub port: i32,
    pub target_port: i32,
    pub socket_type: i32,
}

/// An HTTP or SOCKS5 proxy's values: its scalars, four strings (0 host, 1 username,
/// 2 password, 3 target host) and, for SOCKS5, its packet. A string or the packet is
/// taken out and put back around plain Rust work, never around a call that yields or
/// reaches the proxy again.
pub struct ProxyState {
    fields: Cell<ProxyFields>,
    strings: [Cell<StringSlot>; 4],
    socks: bool,
    /// Boxed, so a take moves a pointer; none for an HTTP proxy or while lent.
    packet: Cell<Option<Box<[u8; 512]>>>,
}
impl ProxyState {
    /// An empty proxy; a SOCKS5 one (`socks`) has a packet.
    #[must_use]
    pub fn new(socks: bool) -> Box<Self> {
        Box::new(Self {
            fields: Cell::new(ProxyFields::default()),
            strings: std::array::from_fn(|_| Cell::default()),
            socks,
            packet: Cell::new(socks.then(|| Box::new([0; 512]))),
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
    /// The scalars, for a caller that writes them in place.
    pub fn fields_ptr(&self) -> *mut ProxyFields {
        self.fields.as_ptr()
    }
    /// `read` over the slot at `index`, none past the four.
    fn with_slot<R>(&self, index: usize, read: impl FnOnce(&mut StringSlot) -> R) -> Option<R> {
        let cell = self.strings.get(index)?;
        let mut slot = cell.take();
        let result = read(&mut slot);
        cell.set(slot);
        Some(result)
    }
    /// A copy of the string at `index`, or empty past the four.
    pub fn string(&self, index: usize) -> Vec<u8> {
        self.with_slot(index, |slot| slot.bytes().to_vec())
            .unwrap_or_default()
    }
    /// [`Self::string`] and the NUL after it.
    pub fn string_with_nul(&self, index: usize) -> Vec<u8> {
        self.with_slot(index, |slot| slot.bytes_with_nul().to_vec())
            .unwrap_or_else(|| vec![0])
    }
    /// Sets the string at `index`: false when the allocation fails or past the four.
    pub fn set_string(&self, index: usize, value: &[u8]) -> bool {
        self.with_slot(index, |slot| slot.set(value))
            .unwrap_or(false)
    }
    /// `pack` over the SOCKS5 packet; none for an HTTP proxy, or while `pack` of an
    /// outer call has it.
    pub fn with_packet<R>(&self, pack: impl FnOnce(&mut [u8; 512]) -> R) -> Option<R> {
        let mut packet = self.packet.take()?;
        let result = pack(&mut packet);
        self.packet.set(Some(packet));
        Some(result)
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
                let socks = self.socks;
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
        owner.with_packet(|packet| packet[7] = 9).unwrap();
        assert_eq!(owner.with_packet(|packet| packet[7]), Some(9));
        // While lent, the packet reads as none.
        assert_eq!(owner.with_packet(|_| owner.with_packet(|_| ())), Some(None));
        assert!(ProxyState::new(false).with_packet(|_| ()).is_none());
    }
}
