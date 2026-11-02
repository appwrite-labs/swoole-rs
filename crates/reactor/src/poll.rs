//! The poll backend's bounded snapshot: a pollfd and socket per registered fd,
//! rebuilt from the registry before each `poll()`, which native performs.
use crate::owner::{release, try_owner};
use crate::registry::{self, Registry};
use std::ffi::{c_int, c_void};
use std::ptr;

const SW_EVENT_READ: c_int = 1 << 9;
const SW_EVENT_WRITE: c_int = 1 << 10;
const SW_EVENT_ERROR: c_int = 1 << 11;

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

/// `translate_events_from_poll`: the swoole events for poll `events`. ERR and HUP
/// count only without IN or OUT, whose handlers already see them.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_poll_events_from(events: i16) -> c_int {
    let mut sw_events = 0;
    if events & libc::POLLIN != 0 {
        sw_events |= SW_EVENT_READ;
    }
    if events & libc::POLLOUT != 0 {
        sw_events |= SW_EVENT_WRITE;
    }
    if events & (libc::POLLERR | libc::POLLHUP) != 0 && events & (libc::POLLIN | libc::POLLOUT) == 0
    {
        sw_events |= SW_EVENT_ERROR;
    }
    sw_events
}

pub struct PollSnapshot {
    entries: Vec<libc::pollfd>,
    sockets: Vec<*mut c_void>,
}

#[repr(C)]
pub struct PollLayout {
    pub size: usize,
    pub alignment: usize,
    pub fd: usize,
    pub events: usize,
    pub revents: usize,
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_poll_layout() -> PollLayout {
    PollLayout {
        size: size_of::<libc::pollfd>(),
        alignment: align_of::<libc::pollfd>(),
        fd: std::mem::offset_of!(libc::pollfd, fd),
        events: std::mem::offset_of!(libc::pollfd, events),
        revents: std::mem::offset_of!(libc::pollfd, revents),
    }
}

/// Fallible owner/storage allocation; no panic or native effect is executed.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_poll_new(limit: usize) -> *mut PollSnapshot {
    let (mut entries, mut sockets) = (Vec::new(), Vec::new());
    if entries.try_reserve_exact(limit).is_err() || sockets.try_reserve_exact(limit).is_err() {
        return ptr::null_mut();
    }
    try_owner(|| PollSnapshot { entries, sockets }).map_or(ptr::null_mut(), Box::into_raw)
}

/// # Safety
/// Owner is null or a live owner from new, exclusively held and no longer used
/// by native poll. Native must never delete/free the borrowed entries pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_poll_free(owner: *mut PollSnapshot) {
    // SAFETY: null or a live, exclusive owner from new, no longer used by native poll.
    unsafe { release(owner) };
}

/// # Safety
/// Owner is live and exclusively held. Result is the actual libc pollfd array;
/// native may access initialized entries and let poll mutate their revents.
/// Address stays stable until free, even across reset/rebuild and nested waits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_poll_data(owner: *mut PollSnapshot) -> *mut libc::pollfd {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let Some(owner) = (unsafe { owner.as_mut() }) else {
        return ptr::null_mut();
    };
    owner.entries.as_mut_ptr()
}

/// # Safety
/// As for [`swr_v1_reactor_poll_data`]; the result is the socket of each pollfd entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_poll_sockets(owner: *mut PollSnapshot) -> *mut *mut c_void {
    // SAFETY: `owner` is null or live, aligned and exclusively lent for this call.
    let Some(owner) = (unsafe { owner.as_mut() }) else {
        return ptr::null_mut();
    };
    owner.sockets.as_mut_ptr()
}

/// `ReactorPoll::set_events`: rebuilds the snapshot from `registry`; the entry count.
///
/// # Safety
/// `owner` is live and exclusively held; `registry` is null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_poll_fill(
    owner: *mut PollSnapshot,
    registry: *const Registry,
) -> c_int {
    // SAFETY: as the caller promises.
    let owner = unsafe { &mut *owner };
    let limit = owner.entries.capacity().min(owner.sockets.capacity());
    let (events, sockets) = (owner.entries.as_mut_ptr(), owner.sockets.as_mut_ptr());
    // SAFETY: both buffers hold `limit` entries; the registry is null or live.
    let count = unsafe { registry::poll(registry, events, sockets, limit) };
    c_int::try_from(count).unwrap_or(c_int::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_events_like_upstream() {
        assert_eq!(swr_v1_reactor_poll_events_to(0), 0);
        assert_eq!(swr_v1_reactor_poll_events_to(SW_EVENT_READ), libc::POLLIN);
        assert_eq!(
            swr_v1_reactor_poll_events_to(SW_EVENT_READ | SW_EVENT_WRITE | SW_EVENT_ERROR),
            libc::POLLIN | libc::POLLOUT
        );
        assert_eq!(
            swr_v1_reactor_poll_events_from(libc::POLLOUT),
            SW_EVENT_WRITE
        );
        assert_eq!(
            swr_v1_reactor_poll_events_from(libc::POLLHUP),
            SW_EVENT_ERROR
        );
        assert_eq!(
            swr_v1_reactor_poll_events_from(libc::POLLERR | libc::POLLIN),
            SW_EVENT_READ
        );
        assert_eq!(swr_v1_reactor_poll_events_from(libc::POLLNVAL), 0);
    }
}
