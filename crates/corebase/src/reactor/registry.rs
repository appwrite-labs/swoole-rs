//! The reactor's socket registry (upstream `Reactor::sockets_`): each registered fd's
//! native `Socket` and the events it was registered with. Native keeps writing the
//! `Socket`'s own `events` and `removed` fields, which other modules read. Fds are
//! small and dense, so the registry is a flat table indexed by fd.
use crate::memory::flat::Table;
use crate::reactor::Heap;
use std::ffi::{c_int, c_void};

/// Only an add of a [`Heap`] socket puts one here, and freeing its record deletes it
/// first (`OwnedSocket::into_box`), so every entry is live.
type Sockets = Table<(Heap<'static>, crate::reactor::Events)>;

/// A socket's place in the registry: its descriptor, from
/// [`crate::reactor::SocketFields::registry_key`].
#[derive(Clone, Copy)]
pub struct RegistryKey(c_int);

impl RegistryKey {
    #[inline(always)]
    #[must_use]
    pub(crate) const fn new(fd: c_int) -> Self {
        Self(fd)
    }
}

#[derive(Default)]
pub struct Registry {
    sockets: Sockets,
}

impl Registry {
    /// How many fds are registered (`Reactor::get_event_num`).
    #[must_use]
    pub fn count(&self) -> usize {
        self.sockets.len()
    }

    /// The registered sockets, in fd order (`Reactor::foreach_socket`'s copy).
    #[must_use]
    pub fn sockets(&self) -> Vec<Heap<'static>> {
        self.sockets.iter().map(|(_, entry)| entry.0).collect()
    }

    /// `Reactor::get_socket`: the socket registered for `fd`, if any.
    #[must_use]
    pub fn get(&self, fd: c_int) -> Option<Heap<'static>> {
        slot(fd)
            .and_then(|fd| self.sockets.get(fd))
            .map(|entry| entry.0)
    }

    /// [`Self::get`] at a socket's key.
    #[inline]
    #[must_use]
    pub(crate) fn entry(&self, key: RegistryKey) -> Option<Heap<'static>> {
        self.get(key.0)
    }

    /// `Reactor::_add`/`_set`: maps `key` to `socket` registered for `events`; false, leaving
    /// the registry unchanged, if allocation fails or the key's fd is negative.
    pub(crate) fn put(
        &mut self,
        RegistryKey(fd): RegistryKey,
        socket: Heap<'_>,
        events: crate::reactor::Events,
    ) -> bool {
        slot(fd).is_some_and(|fd| self.sockets.insert(fd, (socket.named(), events)).is_ok())
    }

    /// `Reactor::_set`: the events `key` is registered for, if it is.
    pub(crate) fn set(&mut self, RegistryKey(fd): RegistryKey, events: crate::reactor::Events) {
        if let Some(entry) = slot(fd).and_then(|fd| self.sockets.get_mut(fd)) {
            entry.1 = events;
        }
    }

    /// `Reactor::_del`: forgets `key`.
    pub(crate) fn remove(&mut self, RegistryKey(fd): RegistryKey) {
        if let Some(fd) = slot(fd) {
            self.sockets.remove(fd);
        }
    }

    /// Each registered fd, its socket and its events, in fd order.
    pub(crate) fn registered(
        &self,
    ) -> impl Iterator<Item = (c_int, Heap<'static>, crate::reactor::Events)> {
        self.sockets
            .iter()
            .filter_map(|(fd, &(socket, events))| Some((c_int::try_from(fd).ok()?, socket, events)))
    }
}

/// The table slot for `fd`; none for a negative fd, which is never registered.
fn slot(fd: c_int) -> Option<usize> {
    usize::try_from(fd).ok()
}

/// `Reactor::get_socket`: the socket registered for `fd`, or null.
#[must_use]
pub fn swr_v1_reactor_registry_get(registry: Option<&Registry>, fd: c_int) -> *mut c_void {
    registry
        .and_then(|registry| registry.get(fd))
        .map_or(std::ptr::null_mut(), |socket| socket.as_ptr().cast())
}

/// `Reactor::get_event_num`: how many fds are registered.
#[must_use]
pub fn swr_v1_reactor_registry_count(registry: Option<&Registry>) -> usize {
    registry.map_or(0, Registry::count)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const SW_EVENT_READ: crate::reactor::Events = crate::reactor::Events::READ;
    const SW_EVENT_WRITE: crate::reactor::Events = crate::reactor::Events::WRITE;

    /// A handle at address `n`, which the registry stores and never reads through.
    pub(crate) fn fake(n: usize) -> Heap<'static> {
        Heap::from_static(
            unsafe { crate::reactor::Socket::from_ptr(n as *mut crate::reactor::SocketRecord) }
                .unwrap(),
        )
    }

    #[test]
    fn registers_and_snapshots() {
        let mut registry = Registry::default();
        let socket = fake;
        let at = |socket: Option<Heap<'static>>| socket.map(|socket| socket.as_ptr());
        assert_eq!(swr_v1_reactor_registry_count(None), 0);
        assert!(swr_v1_reactor_registry_get(None, 3).is_null());
        assert!(registry.put(RegistryKey(3), socket(30), SW_EVENT_READ));
        assert!(registry.put(RegistryKey(4), socket(40), SW_EVENT_WRITE));
        assert!(registry.put(RegistryKey(5), socket(50), SW_EVENT_READ | SW_EVENT_WRITE));
        assert!(registry.put(RegistryKey(4), socket(40), crate::reactor::Events::ERROR));
        assert!(!registry.put(RegistryKey(-1), socket(10), SW_EVENT_READ));
        registry.remove(RegistryKey(5));
        registry.remove(RegistryKey(9));
        assert_eq!(registry.count(), 2);
        assert_eq!(at(registry.get(4)), at(Some(socket(40))));
        assert!(registry.get(5).is_none());
        let sockets: Vec<_> = registry
            .sockets()
            .iter()
            .map(|socket| socket.as_ptr())
            .collect();
        assert_eq!(sockets, [socket(30).as_ptr(), socket(40).as_ptr()]);
        registry.set(RegistryKey(3), SW_EVENT_WRITE);
        let registered: Vec<_> = registry
            .registered()
            .map(|(fd, socket, events)| (fd, socket.as_ptr() as usize, events))
            .collect();
        assert_eq!(
            registered,
            [
                (3, 30, SW_EVENT_WRITE),
                (4, 40, crate::reactor::Events::ERROR)
            ]
        );
    }
}
