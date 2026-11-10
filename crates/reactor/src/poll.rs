//! The poll backend's bounded snapshot: a pollfd and socket per registered fd,
//! rebuilt from the registry before each `poll()`.
use crate::registry::Registry;
use std::ffi::{c_int, c_void};

const SW_EVENT_READ: c_int = 1 << 9;
const SW_EVENT_WRITE: c_int = 1 << 10;

/// `translate_events_to_poll`: the poll bits for swoole `events`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_poll_events_to(events: c_int) -> i16 {
    let mut poll_events = 0;
    if events & SW_EVENT_READ != 0 {
        poll_events |= libc::POLLIN;
    }
    if events & SW_EVENT_WRITE != 0 {
        poll_events |= libc::POLLOUT;
    }
    poll_events
}

/// Room for a fixed number of entries, allocated once, so the arrays never move:
/// the wait loop keeps pointers to them across waits.
pub struct PollSnapshot {
    entries: Vec<libc::pollfd>,
    sockets: Vec<*mut c_void>,
}

impl PollSnapshot {
    /// Room for `limit` entries, aborting as upstream's `bad_alloc` when it could not
    /// be allocated.
    pub(crate) fn new(limit: usize) -> Self {
        let (mut entries, mut sockets) = (Vec::new(), Vec::new());
        if entries.try_reserve_exact(limit).is_err() || sockets.try_reserve_exact(limit).is_err() {
            std::alloc::handle_alloc_error(std::alloc::Layout::new::<libc::pollfd>());
        }
        Self { entries, sockets }
    }

    /// `ReactorPoll::set_events`: rebuilds the snapshot from `registry`, within the
    /// room it was made with; the entry count.
    pub(crate) fn fill(&mut self, registry: &Registry) -> c_int {
        let limit = self.entries.capacity().min(self.sockets.capacity());
        self.entries.clear();
        self.sockets.clear();
        for (fd, socket, events) in registry.registered().take(limit) {
            self.entries.push(libc::pollfd {
                fd,
                events: swr_v1_reactor_poll_events_to(events),
                revents: 0,
            });
            self.sockets.push(socket);
        }
        c_int::try_from(self.entries.len()).unwrap_or(c_int::MAX)
    }

    /// The pollfd array, which `poll()` fills in.
    pub(crate) fn entries(&mut self) -> *mut libc::pollfd {
        self.entries.as_mut_ptr()
    }

    /// The socket of each pollfd entry.
    pub(crate) fn sockets(&mut self) -> *mut *mut c_void {
        self.sockets.as_mut_ptr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_events_like_upstream() {
        const SW_EVENT_ERROR: c_int = 1 << 11;
        assert_eq!(swr_v1_reactor_poll_events_to(0), 0);
        assert_eq!(swr_v1_reactor_poll_events_to(SW_EVENT_READ), libc::POLLIN);
        assert_eq!(
            swr_v1_reactor_poll_events_to(SW_EVENT_READ | SW_EVENT_WRITE | SW_EVENT_ERROR),
            libc::POLLIN | libc::POLLOUT
        );
    }

    #[test]
    fn fills_within_its_room_and_stays_put() {
        let mut registry = Registry::default();
        let socket = |n: usize| n as *mut c_void;
        assert_eq!(registry.put(3, socket(30), SW_EVENT_READ), 0);
        assert_eq!(registry.put(4, socket(40), SW_EVENT_WRITE), 0);
        let mut snapshot = PollSnapshot::new(1);
        let entries = snapshot.entries();
        assert_eq!(snapshot.fill(&registry), 1);
        assert_eq!(snapshot.entries(), entries, "the array does not move");
        // SAFETY: `fill` wrote the first entry.
        let (entry, held) = unsafe { (entries.read(), snapshot.sockets().read()) };
        assert_eq!(
            (entry.fd, entry.events, entry.revents),
            (3, libc::POLLIN, 0)
        );
        assert_eq!(held, socket(30));
    }
}
