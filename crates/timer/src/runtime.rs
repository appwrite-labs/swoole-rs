//! The timer's backend choice and clock arithmetic, and the system timer
//! (`setitimer`, upstream `src/os/timer.cc`).
use libc::c_int;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeTime {
    pub seconds: i64,
    pub fraction: i64,
}

/// `Timer::init`'s mode: manual=0, reactor=1, system=2. No worker-role inference.
#[must_use]
pub fn swr_v1_timer_backend(manual: u32, reactor: u32) -> u32 {
    if manual != 0 {
        0
    } else if reactor != 0 {
        1
    } else {
        2
    }
}

/// Nonpositive milliseconds disarm; positive values produce timeval microseconds.
fn system_interval(milliseconds: i64) -> NativeTime {
    if milliseconds <= 0 {
        NativeTime {
            seconds: 0,
            fraction: 0,
        }
    } else {
        NativeTime {
            seconds: milliseconds / 1000,
            fraction: milliseconds % 1000 * 1000,
        }
    }
}

/// Upstream `SystemTimer_set`: `ITIMER_REAL` fires every `milliseconds` (zero or
/// less disarms it). `SW_OK`, or `SW_ERR` with errno from `setitimer`.
#[expect(
    clippy::must_use_candidate,
    reason = "Timer::set ignores the status, as upstream's set() callers do"
)]
pub fn swr_v1_timer_system_set(milliseconds: i64) -> c_int {
    let interval = system_interval(milliseconds);
    let every = libc::timeval {
        tv_sec: interval.seconds,
        // Under a million microseconds.
        tv_usec: libc::suseconds_t::try_from(interval.fraction).unwrap_or_default(),
    };
    let value = libc::itimerval {
        it_interval: every,
        it_value: every,
    };
    // SAFETY: `value` is a live itimerval; the old value is not wanted.
    if unsafe { libc::setitimer(libc::ITIMER_REAL, &raw const value, std::ptr::null_mut()) } < 0 {
        -1
    } else {
        0
    }
}

/// Upstream `swoole::realtime_add(timespec *, int64_t)`: adds milliseconds to a
/// realtime deadline, carrying whole seconds out of `tv_nsec`.
/// # Safety
/// `time` is a live, writable `timespec`.
#[cfg_attr(
    target_os = "macos",
    unsafe(export_name = "_ZN6swoole12realtime_addEP8timespecx")
)]
#[cfg_attr(
    not(target_os = "macos"),
    unsafe(export_name = "_ZN6swoole12realtime_addEP8timespecl")
)]
pub unsafe extern "C" fn realtime_add(time: *mut libc::timespec, milliseconds: i64) {
    // SAFETY: the caller passes a live, writable timespec.
    let time = unsafe { &mut *time };
    time.tv_sec += milliseconds / 1000;
    time.tv_nsec += milliseconds % 1000 * 1_000_000;
    if time.tv_nsec >= 1_000_000_000 {
        time.tv_sec += time.tv_nsec / 1_000_000_000;
        time.tv_nsec %= 1_000_000_000;
    }
}

/// `Timer::get_absolute_msec()`: `std::chrono::steady_clock` in milliseconds.
#[must_use]
pub fn absolute_msec() -> i64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    #[cfg(target_vendor = "apple")]
    let clock = libc::CLOCK_UPTIME_RAW;
    #[cfg(not(target_vendor = "apple"))]
    let clock = libc::CLOCK_MONOTONIC;
    // SAFETY: a writable timespec.
    unsafe { libc::clock_gettime(clock, &raw mut now) };
    now.tv_sec * 1000 + now.tv_nsec / 1_000_000
}

/// `Timer::get_relative_msec`'s subtraction; overflow was outside upstream's defined input.
#[must_use]
pub fn swr_v1_timer_relative_msec(absolute: i64, base: i64) -> i64 {
    absolute.wrapping_sub(base)
}

/// Preserve the armed timeout unless an accepted add needs an earlier wakeup.
#[must_use]
pub fn swr_v1_timer_rearm_after_add(current: i64, delay: i64) -> i64 {
    if current < 0 || current > delay {
        delay
    } else {
        current
    }
}

/// `Timer::delay`'s new deadline, `delay` after `now` (or after `previous` without a
/// clock), unless it overflows or falls before zero.
#[must_use]
pub fn swr_v1_timer_delay_deadline(now: i64, previous: i64, delay: i64) -> Option<i64> {
    let base = if now < 0 { previous } else { now };
    base.checked_add(delay).filter(|value| *value >= 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Upstream's `time.get_ms_steady`, over `Timer::get_absolute_msec`'s clock.
    #[test]
    fn time_get_ms_steady() {
        let us = 3000;
        let ms1 = absolute_msec();
        std::thread::sleep(std::time::Duration::from_micros(us));
        let ms2 = absolute_msec();
        assert!(ms2 - ms1 >= i64::try_from(us / 1000).unwrap());
    }

    #[test]
    fn native_modes_and_armed_timeout_are_independent_of_queue_state() {
        assert_eq!(swr_v1_timer_backend(1, 1), 0);
        assert_eq!(swr_v1_timer_backend(1, 0), 0);
        assert_eq!(swr_v1_timer_backend(0, 1), 1);
        assert_eq!(swr_v1_timer_backend(0, 0), 2);
        for current in [-1, 0, 1, 999, i64::MAX] {
            for delay in [1, 2, 999, i64::MAX] {
                assert_eq!(
                    swr_v1_timer_rearm_after_add(current, delay),
                    if current < 0 {
                        delay
                    } else {
                        current.min(delay)
                    }
                );
            }
        }
    }

    #[test]
    fn system_interval_disarms_or_splits_milliseconds() {
        for value in [
            i64::MIN,
            -1_000_000_001,
            -1,
            0,
            1,
            999,
            1000,
            1_000_000_001,
            i64::MAX,
        ] {
            let interval = system_interval(value);
            if value <= 0 {
                assert_eq!(
                    interval,
                    NativeTime {
                        seconds: 0,
                        fraction: 0
                    }
                );
            } else {
                assert_eq!(interval.seconds * 1000 + interval.fraction / 1000, value);
                assert!((0..1_000_000).contains(&interval.fraction));
            }
        }
    }

    // core-tests/src/os/timer.cpp TEST(timer, realtime_add).
    #[test]
    fn realtime_add_carries_seconds() {
        let mut ts = libc::timespec {
            tv_sec: 1,
            tv_nsec: 900 * 1_000_000,
        };
        unsafe { realtime_add(&raw mut ts, 1905) };
        assert_eq!(ts.tv_sec, 3);
        assert_eq!(ts.tv_nsec, 805 * 1_000_000);
    }

    #[test]
    fn system_timer_arms_and_disarms() {
        assert_eq!(swr_v1_timer_system_set(-1), 0);
        let mut current = libc::itimerval {
            it_interval: libc::timeval {
                tv_sec: 1,
                tv_usec: 1,
            },
            it_value: libc::timeval {
                tv_sec: 1,
                tv_usec: 1,
            },
        };
        unsafe { libc::getitimer(libc::ITIMER_REAL, &raw mut current) };
        assert_eq!(
            (current.it_interval.tv_sec, current.it_interval.tv_usec),
            (0, 0)
        );
        assert_eq!((current.it_value.tv_sec, current.it_value.tv_usec), (0, 0));
    }

    #[test]
    fn delay_checks_limits_and_keeps_native_clock_fallback() {
        assert_eq!(swr_v1_timer_delay_deadline(-1, 20, -5), Some(15));
        for (now, old, delay) in [(i64::MAX, 0, 1), (-1, i64::MIN, -1), (0, 0, -1)] {
            assert_eq!(swr_v1_timer_delay_deadline(now, old, delay), None);
        }
        assert_eq!(swr_v1_timer_delay_deadline(10, 0, 5), Some(15));
    }
}
