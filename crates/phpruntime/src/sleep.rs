//! The sleep family's replacements (`HOOK_SLEEP`): a coroutine sleeps by
//! yielding (`System::sleep`), anything else as PHP would. The checks, the
//! duration and the sleep are crates/hooks'.
use crate::{in_coroutine, ops};
use swoole_rs_hooks::sleep::{
    CoroutineSleep, swr_v1_hooks_nanosleep, swr_v1_hooks_sleep, swr_v1_hooks_sleep_until,
    swr_v1_hooks_usleep,
};
use swoole_rs_zend::{Call, E_WARNING, message, php_error, sys::zend_argument_value_error};

swoole_rs_zend::handlers! {
    zif_swoole_sleep => sleep;
    zif_swoole_usleep => usleep;
    zif_swoole_time_nanosleep => time_nanosleep;
    zif_swoole_time_sleep_until => time_sleep_until;
}

/// Statuses of a `SleepOutcome`.
const INVALID: i32 = 1;
const WARN: i32 = 3;

fn sleeper() -> Option<CoroutineSleep> {
    in_coroutine().then_some(ops().coroutine_sleep)
}

/// `zend_argument_value_error(number, "must be greater than or equal to 0")`.
fn negative(number: i64) {
    #[expect(clippy::cast_possible_truncation, reason = "an argument number")]
    #[expect(clippy::cast_sign_loss, reason = "an argument number")]
    // SAFETY: a handler's context; the format has no conversions.
    unsafe {
        zend_argument_value_error(
            number as u32,
            c"must be greater than or equal to 0".as_ptr(),
        );
    }
}

/// `sleep(int $seconds): int`.
fn sleep(call: &Call) {
    let Ok(seconds) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        return;
    };
    let outcome = swr_v1_hooks_sleep(seconds, sleeper());
    if outcome.status == INVALID {
        negative(outcome.value);
        return;
    }
    call.set_long(outcome.value);
}

/// `usleep(int $microseconds): void`.
fn usleep(call: &Call) {
    let Ok(microseconds) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return;
    };
    let outcome = swr_v1_hooks_usleep(microseconds, sleeper());
    if outcome.status == INVALID {
        negative(outcome.value);
    }
}

/// `time_nanosleep(int $seconds, int $nanoseconds)`: upstream builds the
/// interrupted case's array and then returns true over it, so true it is.
fn time_nanosleep(call: &Call) {
    let parsed = call
        .arity(2, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, 0)?)));
    let Ok((seconds, nanoseconds)) = parsed else {
        call.set_bool(false);
        return;
    };
    let outcome = swr_v1_hooks_nanosleep(seconds, nanoseconds, sleeper());
    if outcome.status == INVALID {
        negative(outcome.value);
        return;
    }
    if outcome.status == WARN {
        // SAFETY: a NUL-terminated message.
        unsafe {
            (ops().error)(
                E_WARNING,
                c"nanoseconds was not in the range 0 to 999 999 999 or seconds was negative"
                    .as_ptr(),
            );
        }
    }
    call.set_bool(true);
}

/// `time_sleep_until(float $timestamp): bool`.
fn time_sleep_until(call: &Call) {
    /// `(double) (UINT64_MAX / 1000000000)`.
    const TOP: u64 = u64::MAX / 1_000_000_000;
    let Ok(timestamp) = call.arity(1, 1).and_then(|()| call.double(1, 0.0)) else {
        call.set_bool(false);
        return;
    };
    #[expect(clippy::cast_precision_loss, reason = "upstream's (double) cast")]
    let outcome = swr_v1_hooks_sleep_until(timestamp, TOP as f64, sleeper());
    match outcome.status {
        INVALID => {
            let message = message!("must be between 0 and {TOP}");
            // SAFETY: a handler's context; "%s" takes the NUL-terminated message.
            unsafe { zend_argument_value_error(1, c"%s".as_ptr(), message.as_cstr().as_ptr()) };
        }
        WARN => {
            // SAFETY: a handler's context.
            unsafe {
                php_error(
                    E_WARNING,
                    c"Argument #1 ($timestamp) must be greater than or equal to the current time",
                );
            }
            call.set_bool(false);
        }
        _ => call.set_bool(true),
    }
}
