//! kqueue's timeout policy. Native builds the `timespec`.
#[repr(C)]
pub struct KqueueTimeout {
    pub seconds: i64,
    pub nanoseconds: i64,
    pub finite: u32,
}

/// Preserve kqueue's positive-timeout priority over deferred work, unlike the
/// poll/epoll Native `get_timeout_msec` helper. Native constructs its timespec.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_kqueue_timeout(milliseconds: i32, deferred: u32) -> KqueueTimeout {
    if milliseconds > 0 {
        KqueueTimeout {
            seconds: i64::from(milliseconds / 1000),
            nanoseconds: i64::from(milliseconds % 1000) * 1_000_000,
            finite: 1,
        }
    } else {
        KqueueTimeout {
            seconds: 0,
            nanoseconds: 0,
            finite: u32::from(deferred != 0),
        }
    }
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
