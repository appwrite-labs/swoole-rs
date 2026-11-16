//! The poll backend's bounded snapshot: a pollfd and socket per registered fd,
//! rebuilt from the registry before each `poll()`.
use crate::registry::Registry;

/// `translate_events_to_poll`: the poll bits for swoole `events`.
#[must_use]
pub fn swr_v1_reactor_poll_events_to(events: crate::Events) -> i16 {
    let mut poll_events = 0;
    if events.intersects(crate::Events::READ) {
        poll_events |= libc::POLLIN;
    }
    if events.intersects(crate::Events::WRITE) {
        poll_events |= libc::POLLOUT;
    }
    poll_events
}

/// Room for a fixed number of entries, allocated once: the registry's sockets as it
/// held them before the wait, so an entry's socket is live: freeing a record deletes it from the registry first.
pub struct PollSnapshot {
    entries: Vec<libc::pollfd>,
    sockets: Vec<crate::Socket<'static>>,
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
    pub(crate) fn fill(&mut self, registry: &Registry) -> usize {
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
        self.entries.len()
    }

    /// The filled pollfd entries, for `poll()`.
    pub(crate) fn filled(&mut self) -> &mut [libc::pollfd] {
        &mut self.entries
    }

    /// Entry `index` as `poll()` left it, and the socket it was built from.
    #[inline]
    pub(crate) fn entry(&self, index: usize) -> Option<(libc::pollfd, crate::Socket<'static>)> {
        Some((*self.entries.get(index)?, *self.sockets.get(index)?))
    }

    /// Where the pollfd array is, which never moves.
    #[cfg(test)]
    pub(crate) fn entries(&self) -> *const libc::pollfd {
        self.entries.as_ptr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SW_EVENT_READ: crate::Events = crate::Events::READ;
    const SW_EVENT_WRITE: crate::Events = crate::Events::WRITE;

    #[test]
    fn translates_events_like_upstream() {
        const SW_EVENT_ERROR: crate::Events = crate::Events::ERROR;
        assert_eq!(swr_v1_reactor_poll_events_to(crate::Events::NONE), 0);
        assert_eq!(swr_v1_reactor_poll_events_to(SW_EVENT_READ), libc::POLLIN);
        assert_eq!(
            swr_v1_reactor_poll_events_to(SW_EVENT_READ | SW_EVENT_WRITE | SW_EVENT_ERROR),
            libc::POLLIN | libc::POLLOUT
        );
    }

    #[test]
    fn fills_within_its_room_and_stays_put() {
        let mut registry = Registry::default();
        let socket = crate::registry::tests::fake;
        assert!(registry.put(3, socket(30), SW_EVENT_READ));
        assert!(registry.put(4, socket(40), SW_EVENT_WRITE));
        let mut snapshot = PollSnapshot::new(1);
        let entries = snapshot.entries();
        assert_eq!(snapshot.fill(&registry), 1);
        assert_eq!(snapshot.entries(), entries, "the array does not move");
        let (entry, held) = snapshot.entry(0).unwrap();
        assert_eq!(
            (entry.fd, entry.events, entry.revents),
            (3, libc::POLLIN, 0)
        );
        assert_eq!(held.as_ptr(), socket(30).as_ptr());
    }
}
