//! kqueue's timeout policy.

/// kevent's timeout, or none to block: a positive timeout takes priority over
/// deferred work, unlike the poll/epoll `get_timeout_msec` helper.
#[must_use]
pub fn kqueue_timeout(milliseconds: i32, deferred: bool) -> Option<libc::timespec> {
    if milliseconds > 0 {
        Some(libc::timespec {
            tv_sec: libc::time_t::from(milliseconds / 1000),
            tv_nsec: libc::c_long::from(milliseconds % 1000) * 1_000_000,
        })
    } else {
        deferred.then_some(libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        })
    }
}
