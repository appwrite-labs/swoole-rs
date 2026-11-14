//! The reactor's socket registry (upstream `Reactor::sockets_`): each registered fd's
//! native `Socket` and the events it was registered with. Native keeps writing the
//! `Socket`'s own `events` and `removed` fields, which other modules read. Fds are
//! small and dense, so the registry is a flat table indexed by fd.
use std::ffi::{c_int, c_void};
use std::ptr;
use swoole_rs_memory::flat::Table;

type Sockets = Table<(*mut c_void, c_int)>;

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
    pub fn sockets(&self) -> Vec<*mut c_void> {
        self.sockets.iter().map(|(_, entry)| entry.0).collect()
    }

    /// `Reactor::get_socket`: the socket registered for `fd`, or null.
    #[must_use]
    pub fn get(&self, fd: c_int) -> *mut c_void {
        slot(fd)
            .and_then(|fd| self.sockets.get(fd))
            .map_or(ptr::null_mut(), |entry| entry.0)
    }

    /// `Reactor::_add`/`_set`: maps `fd` to `socket` registered for `events`; false, leaving
    /// the registry unchanged, if allocation fails or `fd` is negative.
    pub(crate) fn put(&mut self, fd: c_int, socket: *mut c_void, events: c_int) -> bool {
        slot(fd).is_some_and(|fd| self.sockets.insert(fd, (socket, events)).is_ok())
    }

    /// `Reactor::_set`: the events `fd` is registered for, if it is.
    pub(crate) fn set(&mut self, fd: c_int, events: c_int) {
        if let Some(entry) = slot(fd).and_then(|fd| self.sockets.get_mut(fd)) {
            entry.1 = events;
        }
    }

    /// `Reactor::_del`: forgets `fd`.
    pub(crate) fn remove(&mut self, fd: c_int) {
        if let Some(fd) = slot(fd) {
            self.sockets.remove(fd);
        }
    }

    /// Each registered fd, its socket and its events, in fd order.
    pub(crate) fn registered(&self) -> impl Iterator<Item = (c_int, *mut c_void, c_int)> {
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
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_registry_get(
    registry: Option<&Registry>,
    fd: c_int,
) -> *mut c_void {
    registry.map_or(ptr::null_mut(), |registry| registry.get(fd))
}

/// `Reactor::get_event_num`: how many fds are registered.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_registry_count(registry: Option<&Registry>) -> usize {
    registry.map_or(0, Registry::count)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SW_EVENT_READ: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_READ as c_int;
    const SW_EVENT_WRITE: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_WRITE as c_int;

    #[test]
    fn registers_and_snapshots() {
        let mut registry = Registry::default();
        let socket = |n: usize| n as *mut c_void;
        assert_eq!(swr_v1_reactor_registry_count(None), 0);
        assert!(swr_v1_reactor_registry_get(None, 3).is_null());
        assert!(registry.put(3, socket(30), SW_EVENT_READ));
        assert!(registry.put(4, socket(40), SW_EVENT_WRITE));
        assert!(registry.put(5, socket(50), SW_EVENT_READ | SW_EVENT_WRITE));
        assert!(registry.put(4, socket(40), 1 << 11));
        assert!(!registry.put(-1, socket(10), SW_EVENT_READ));
        registry.remove(5);
        registry.remove(9);
        assert_eq!(registry.count(), 2);
        assert_eq!(registry.get(4), socket(40));
        assert!(registry.get(5).is_null());
        assert_eq!(registry.sockets(), [socket(30), socket(40)]);
        registry.set(3, SW_EVENT_WRITE);
        let registered: Vec<_> = registry
            .registered()
            .map(|(fd, socket, events)| (fd, socket as usize, events))
            .collect();
        assert_eq!(registered, [(3, 30, SW_EVENT_WRITE), (4, 40, 1 << 11)]);
    }
}
