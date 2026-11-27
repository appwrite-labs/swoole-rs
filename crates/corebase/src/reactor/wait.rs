//! kqueue's timeout policy.

/// Deferred work polls immediately. Otherwise a positive timeout limits the wait.
#[must_use]
pub fn kqueue_timeout(milliseconds: i32, deferred: bool) -> Option<libc::timespec> {
    let timeout = if deferred { 0 } else { milliseconds };
    (deferred || timeout > 0).then(|| libc::timespec {
        tv_sec: (timeout / 1000).into(),
        tv_nsec: libc::c_long::from(timeout % 1000) * 1_000_000,
    })
}

/// Coroutine `flock` and `waitpid`'s next poll delay: double up to `maximum`, then
/// start over at `initial`.
#[must_use]
pub fn backoff(current: f64, initial: f64, maximum: f64) -> f64 {
    if current >= maximum {
        initial
    } else {
        let doubled = current * 2.0;
        // Preserve native SW_MIN rather than f64::min's different NaN policy.
        if doubled < maximum { doubled } else { maximum }
    }
}
