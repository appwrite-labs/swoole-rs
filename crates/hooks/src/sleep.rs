//! PHP's `sleep`, `usleep`, `time_nanosleep` and `time_sleep_until` as hooked by
//! `Runtime::enableCoroutine`: argument checks, the duration and the sleep itself.
//! Inside a coroutine the native caller passes `coroutine_sleep` (`System::sleep`,
//! which yields); outside one it passes null and the thread sleeps here. Native code
//! keeps only parameter parsing, raising the reported error and building the value.
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use swoole_rs_corebase::CCast;

/// `System::sleep(seconds)`: 0, or negative when the coroutine was cancelled.
pub type CoroutineSleep = extern "C-unwind" fn(f64) -> i32;

/// What a sleep call returns to PHP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SleepOutcome {
    /// Done, with `sleep`'s return value (0 for the others).
    Done(i64),
    /// This argument is negative (or out of range, for `sleep_until`).
    Negative(u32),
    /// Interrupted, with the time left.
    Interrupted { seconds: i64, nanoseconds: i64 },
    /// `time_nanosleep`: invalid nanoseconds, then true; `time_sleep_until`: the
    /// target has passed, then false.
    Warn,
}

const DONE: SleepOutcome = SleepOutcome::Done(0);

/// Sleeps `seconds` in the current coroutine; whether it was cancelled.
fn yielded(coroutine_sleep: CoroutineSleep, seconds: f64) -> bool {
    coroutine_sleep(seconds) < 0
}

/// `sleep(seconds)`: in a coroutine, 0 or `seconds` if cancelled; otherwise what
/// sleep(3) returns.
#[must_use]
pub fn swr_v1_hooks_sleep(seconds: i64, coroutine_sleep: Option<CoroutineSleep>) -> SleepOutcome {
    if seconds < 0 {
        return SleepOutcome::Negative(1);
    }
    let value = match coroutine_sleep {
        Some(sleep) => {
            if yielded(sleep, seconds.c_cast::<f64>()) {
                seconds
            } else {
                0
            }
        }
        None => i64::from(swoole_rs_corebase::sys::sleep(
            seconds.cast_unsigned().c_cast::<u32>(),
        )),
    };
    SleepOutcome::Done(value)
}

/// `usleep(microseconds)`.
#[must_use]
pub fn swr_v1_hooks_usleep(
    microseconds: i64,
    coroutine_sleep: Option<CoroutineSleep>,
) -> SleepOutcome {
    if microseconds < 0 {
        return SleepOutcome::Negative(1);
    }
    match coroutine_sleep {
        Some(sleep) => {
            yielded(sleep, microseconds.c_cast::<f64>() / 1_000_000.0);
        }
        None => {
            swoole_rs_corebase::sys::usleep(microseconds.cast_unsigned().c_cast::<u32>());
        }
    }
    DONE
}

/// `time_nanosleep(seconds, nanoseconds)`.
#[must_use]
pub fn swr_v1_hooks_nanosleep(
    seconds: i64,
    nanoseconds: i64,
    coroutine_sleep: Option<CoroutineSleep>,
) -> SleepOutcome {
    if seconds < 0 {
        return SleepOutcome::Negative(1);
    }
    if nanoseconds < 0 {
        return SleepOutcome::Negative(2);
    }
    if let Some(sleep) = coroutine_sleep {
        yielded(
            sleep,
            seconds.c_cast::<f64>() + nanoseconds.c_cast::<f64>() / 1_000_000_000.00,
        );
        return DONE;
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
        return DONE;
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::EINTR) => SleepOutcome::Interrupted {
            seconds: remaining.tv_sec,
            nanoseconds: remaining.tv_nsec,
        },
        Some(libc::EINVAL) => SleepOutcome::Warn,
        _ => DONE,
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
        return SleepOutcome::Negative(1);
    }
    let target = UNIX_EPOCH + Duration::from_secs_f64(timestamp);
    let Ok(left) = target.duration_since(SystemTime::now()) else {
        return SleepOutcome::Warn;
    };
    if left.is_zero() {
        return SleepOutcome::Warn;
    }
    match coroutine_sleep {
        Some(sleep) => {
            yielded(sleep, left.as_secs_f64());
        }
        None => std::thread::sleep(left),
    }
    DONE
}

/// `int System::sleep(double sec)`: parks the current coroutine for `sec`, at least
/// the timer's millisecond (in the ring's timer with `io_uring`); -1 when it was
/// canceled. Outside a coroutine it raises upstream's fatal error, which does not
/// return.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6System5sleepEd")
)]
#[must_use]
pub extern "C-unwind" fn system_sleep(sec: f64) -> i32 {
    #[cfg(feature = "iouring")]
    {
        crate::uring::sleep(&crate::iouring::HOST, sec)
    }
    #[cfg(not(feature = "iouring"))]
    {
        /// `SW_ERROR_CO_CANCELED`: what `yield_ex` sets when the coroutine was canceled.
        const CANCELED: i32 = 10017;
        let resumed = crate::resolve::park_for(crate::sleep_timeout(sec, 0.001));
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

    extern "C-unwind" fn cancelled(_: f64) -> i32 {
        -1
    }
    extern "C-unwind" fn slept(seconds: f64) -> i32 {
        assert!(seconds >= 0.0, "a non-negative duration");
        0
    }

    #[test]
    fn negative_arguments_name_their_position() {
        assert_eq!(swr_v1_hooks_sleep(-1, None), SleepOutcome::Negative(1));
        assert_eq!(swr_v1_hooks_usleep(-1, None), SleepOutcome::Negative(1));
        assert_eq!(
            swr_v1_hooks_nanosleep(-1, 0, None),
            SleepOutcome::Negative(1)
        );
        assert_eq!(
            swr_v1_hooks_nanosleep(0, -1, None),
            SleepOutcome::Negative(2)
        );
        assert_eq!(
            swr_v1_hooks_sleep_until(-1.0, 1.0, None),
            SleepOutcome::Negative(1)
        );
        assert_eq!(
            swr_v1_hooks_sleep_until(2.0, 1.0, None),
            SleepOutcome::Negative(1)
        );
        assert_eq!(
            swr_v1_hooks_sleep_until(f64::NAN, 1.0, None),
            SleepOutcome::Negative(1)
        );
    }

    #[test]
    fn coroutine_sleep_reports_cancellation_as_upstream() {
        assert_eq!(
            swr_v1_hooks_sleep(7, Some(cancelled)),
            SleepOutcome::Done(7)
        );
        assert_eq!(swr_v1_hooks_sleep(7, Some(slept)), DONE);
        assert_eq!(swr_v1_hooks_usleep(5, Some(slept)), DONE);
        assert_eq!(swr_v1_hooks_nanosleep(0, 5, Some(slept)), DONE);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sleep")]
    fn thread_sleeps() {
        assert_eq!(swr_v1_hooks_sleep(0, None), DONE);
        assert_eq!(swr_v1_hooks_usleep(1, None), DONE);
        assert_eq!(swr_v1_hooks_nanosleep(0, 1, None), DONE);
        assert_eq!(
            swr_v1_hooks_nanosleep(0, 1_000_000_000, None),
            SleepOutcome::Warn
        );
    }

    #[test]
    fn past_targets_warn_and_future_ones_sleep() {
        assert_eq!(
            swr_v1_hooks_sleep_until(1.0, f64::MAX, None),
            SleepOutcome::Warn
        );
        // A minute ahead, so a slow run (Miri) still reaches the call before it; the
        // coroutine sleep returns at once.
        let soon = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |now| now.as_secs_f64() + 60.0);
        assert_eq!(swr_v1_hooks_sleep_until(soon, f64::MAX, Some(slept)), DONE);
    }
}
