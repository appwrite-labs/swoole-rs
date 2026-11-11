//! PHP's `sleep`, `usleep`, `time_nanosleep` and `time_sleep_until` as hooked by
//! `Runtime::enableCoroutine`: argument checks, the duration and the sleep itself.
//! Inside a coroutine the native caller passes `coroutine_sleep` (`System::sleep`,
//! which yields); outside one it passes null and the thread sleeps here. Native code
//! keeps only parameter parsing, raising the reported error and building the value.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `System::sleep(seconds)`: 0, or negative when the coroutine was cancelled.
pub type CoroutineSleep = unsafe extern "C-unwind" fn(f64) -> i32;

/// What a sleep call returns to PHP.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SleepOutcome {
    /// 0 done; 1 argument `value` is negative (or out of range, for `sleep_until`);
    /// 2 interrupted, with the time left in `seconds` and `nanoseconds`; 3 warn
    /// (`time_nanosleep`: invalid nanoseconds, then true; `time_sleep_until`: the
    /// target has passed, then false).
    pub status: i32,
    /// `sleep`'s return value when done; the argument number for status 1.
    pub value: i64,
    pub seconds: i64,
    pub nanoseconds: i64,
}

const fn argument(number: i64) -> SleepOutcome {
    SleepOutcome {
        status: 1,
        value: number,
        seconds: 0,
        nanoseconds: 0,
    }
}

/// Sleeps `seconds` in the current coroutine; whether it was cancelled.
fn yielded(coroutine_sleep: CoroutineSleep, seconds: f64) -> bool {
    // SAFETY: the native caller passes System::sleep, valid inside the current coroutine.
    unsafe { coroutine_sleep(seconds) < 0 }
}

/// `sleep(seconds)`: in a coroutine, 0 or `seconds` if cancelled; otherwise what
/// sleep(3) returns.
#[must_use]
pub fn swr_v1_hooks_sleep(seconds: i64, coroutine_sleep: Option<CoroutineSleep>) -> SleepOutcome {
    if seconds < 0 {
        return argument(1);
    }
    let value = match coroutine_sleep {
        #[expect(clippy::cast_precision_loss, reason = "upstream's (double) num")]
        Some(sleep) => {
            if yielded(sleep, seconds as f64) {
                seconds
            } else {
                0
            }
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream's (unsigned int) num"
        )]
        None => i64::from(swoole_rs_corebase::sys::sleep(
            seconds.cast_unsigned() as u32
        )),
    };
    SleepOutcome {
        value,
        ..SleepOutcome::default()
    }
}

/// `usleep(microseconds)`.
#[must_use]
pub fn swr_v1_hooks_usleep(
    microseconds: i64,
    coroutine_sleep: Option<CoroutineSleep>,
) -> SleepOutcome {
    if microseconds < 0 {
        return argument(1);
    }
    match coroutine_sleep {
        #[expect(clippy::cast_precision_loss, reason = "upstream's (double) num")]
        Some(sleep) => {
            yielded(sleep, microseconds as f64 / 1_000_000.0);
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream's (unsigned int) num"
        )]
        None => {
            swoole_rs_corebase::sys::usleep(microseconds.cast_unsigned() as u32);
        }
    }
    SleepOutcome::default()
}

/// `time_nanosleep(seconds, nanoseconds)`.
#[must_use]
pub fn swr_v1_hooks_nanosleep(
    seconds: i64,
    nanoseconds: i64,
    coroutine_sleep: Option<CoroutineSleep>,
) -> SleepOutcome {
    if seconds < 0 {
        return argument(1);
    }
    if nanoseconds < 0 {
        return argument(2);
    }
    if let Some(sleep) = coroutine_sleep {
        #[expect(
            clippy::cast_precision_loss,
            reason = "upstream's (double) tv_sec + tv_nsec"
        )]
        yielded(
            sleep,
            seconds as f64 + nanoseconds as f64 / 1_000_000_000.00,
        );
        return SleepOutcome::default();
    }
    let request = libc::timespec {
        tv_sec: seconds,
        tv_nsec: nanoseconds,
    };
    let mut remaining = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if swoole_rs_corebase::sys::nanosleep(&request, &mut remaining) == 0 {
        return SleepOutcome::default();
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::EINTR) => SleepOutcome {
            status: 2,
            value: 0,
            seconds: remaining.tv_sec,
            nanoseconds: remaining.tv_nsec,
        },
        Some(libc::EINVAL) => SleepOutcome {
            status: 3,
            ..SleepOutcome::default()
        },
        _ => SleepOutcome::default(),
    }
}

/// `time_sleep_until(timestamp)`: `top` is the largest timestamp upstream accepts.
#[must_use]
pub fn swr_v1_hooks_sleep_until(
    timestamp: f64,
    top: f64,
    coroutine_sleep: Option<CoroutineSleep>,
) -> SleepOutcome {
    if !(timestamp >= 0.0 && timestamp <= top) {
        return argument(1);
    }
    let target = UNIX_EPOCH + Duration::from_secs_f64(timestamp);
    let Ok(left) = target.duration_since(SystemTime::now()) else {
        return SleepOutcome {
            status: 3,
            ..SleepOutcome::default()
        };
    };
    if left.is_zero() {
        return SleepOutcome {
            status: 3,
            ..SleepOutcome::default()
        };
    }
    match coroutine_sleep {
        Some(sleep) => {
            yielded(sleep, left.as_secs_f64());
        }
        None => std::thread::sleep(left),
    }
    SleepOutcome::default()
}

/// `int System::sleep(double sec)`: parks the current coroutine for `sec`, at least
/// the timer's millisecond (in the ring's timer with `io_uring`); -1 when it was
/// canceled.
///
/// # Safety
/// In a coroutine (outside one it raises upstream's fatal error, which does not
/// return), with the thread's reactor running.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6System5sleepEd")
)]
#[must_use]
pub unsafe extern "C-unwind" fn system_sleep(sec: f64) -> i32 {
    #[cfg(feature = "iouring")]
    {
        crate::uring::sleep(&crate::iouring::HOST, sec)
    }
    #[cfg(not(feature = "iouring"))]
    {
        use crate::wait_event::native::{coroutine_yield_ex, current};
        /// `SW_ERROR_CO_CANCELED`: what `yield_ex` sets when the coroutine was canceled.
        const CANCELED: i32 = 10017;
        let co = current();
        // SAFETY: `co` is the running coroutine.
        let resumed = unsafe { coroutine_yield_ex(co.cast(), crate::sleep_timeout(sec, 0.001)) };
        if !resumed && swoole_rs_corebase::log::last_error() == CANCELED {
            -1
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C-unwind" fn cancelled(_: f64) -> i32 {
        -1
    }
    unsafe extern "C-unwind" fn slept(seconds: f64) -> i32 {
        assert!(seconds >= 0.0, "a non-negative duration");
        0
    }

    #[test]
    fn negative_arguments_name_their_position() {
        assert_eq!(swr_v1_hooks_sleep(-1, None), argument(1));
        assert_eq!(swr_v1_hooks_usleep(-1, None), argument(1));
        assert_eq!(swr_v1_hooks_nanosleep(-1, 0, None), argument(1));
        assert_eq!(swr_v1_hooks_nanosleep(0, -1, None), argument(2));
        assert_eq!(swr_v1_hooks_sleep_until(-1.0, 1.0, None), argument(1));
        assert_eq!(swr_v1_hooks_sleep_until(2.0, 1.0, None), argument(1));
        assert_eq!(swr_v1_hooks_sleep_until(f64::NAN, 1.0, None), argument(1));
    }

    #[test]
    fn coroutine_sleep_reports_cancellation_as_upstream() {
        assert_eq!(swr_v1_hooks_sleep(7, Some(cancelled)).value, 7);
        assert_eq!(swr_v1_hooks_sleep(7, Some(slept)).value, 0);
        assert_eq!(swr_v1_hooks_usleep(5, Some(slept)), SleepOutcome::default());
        assert_eq!(
            swr_v1_hooks_nanosleep(0, 5, Some(slept)),
            SleepOutcome::default()
        );
    }

    #[test]
    fn thread_sleeps() {
        assert_eq!(swr_v1_hooks_sleep(0, None), SleepOutcome::default());
        assert_eq!(swr_v1_hooks_usleep(1, None), SleepOutcome::default());
        assert_eq!(swr_v1_hooks_nanosleep(0, 1, None), SleepOutcome::default());
        assert_eq!(swr_v1_hooks_nanosleep(0, 1_000_000_000, None).status, 3);
    }

    #[test]
    fn past_targets_warn_and_future_ones_sleep() {
        assert_eq!(swr_v1_hooks_sleep_until(1.0, f64::MAX, None).status, 3);
        let soon = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |now| now.as_secs_f64() + 0.001);
        assert_eq!(
            swr_v1_hooks_sleep_until(soon, f64::MAX, Some(slept)),
            SleepOutcome::default()
        );
    }
}
