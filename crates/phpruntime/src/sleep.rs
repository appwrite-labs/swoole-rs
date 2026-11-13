//! The sleep family's replacements (`HOOK_SLEEP`): a coroutine sleeps by
//! yielding (`System::sleep`), anything else as PHP would. The checks, the
//! duration and the sleep are crates/hooks'.
use crate::{Report, in_coroutine, report};
use swoole_rs_corebase::CCast;
use swoole_rs_hooks::sleep::{
    CoroutineSleep, SleepOutcome, swr_v1_hooks_nanosleep, swr_v1_hooks_sleep,
    swr_v1_hooks_sleep_until, swr_v1_hooks_usleep,
};
use swoole_rs_zend::{Call, E_WARNING, message};

swoole_rs_zend::handlers! {
    zif_swoole_sleep => sleep;
    zif_swoole_usleep => usleep;
    zif_swoole_time_nanosleep => time_nanosleep;
    zif_swoole_time_sleep_until => time_sleep_until;
}

// `System::sleep(seconds)` (crates/hooks).
use swoole_rs_hooks::sleep::system_sleep as coroutine_sleep;

fn sleeper() -> Option<CoroutineSleep> {
    in_coroutine().then_some(coroutine_sleep)
}

/// `zend_argument_value_error(number, "must be greater than or equal to 0")`.
fn negative(call: &Call, number: u32) {
    call.argument_value_error(number, c"must be greater than or equal to 0");
}

/// `sleep(int $seconds): int`.
fn sleep(call: &Call) {
    let Ok(seconds) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        return;
    };
    match swr_v1_hooks_sleep(seconds, sleeper()) {
        SleepOutcome::Negative(number) => negative(call, number),
        SleepOutcome::Done(value) => call.set_long(value),
        SleepOutcome::Interrupted { .. } | SleepOutcome::Warn => call.set_long(0),
    }
}

/// `usleep(int $microseconds): void`.
fn usleep(call: &Call) {
    let Ok(microseconds) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return;
    };
    if let SleepOutcome::Negative(number) = swr_v1_hooks_usleep(microseconds, sleeper()) {
        negative(call, number);
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
    if let SleepOutcome::Negative(number) = outcome {
        negative(call, number);
        return;
    }
    if outcome == SleepOutcome::Warn {
        swoole_rs_zend::bailout::defer(report(
            Report::Swoole,
            E_WARNING,
            c"nanoseconds was not in the range 0 to 999 999 999 or seconds was negative",
        ));
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
    let outcome = swr_v1_hooks_sleep_until(timestamp, TOP.c_cast::<f64>(), sleeper());
    match outcome {
        SleepOutcome::Negative(_) => {
            let message = message!("must be between 0 and {TOP}");
            call.argument_value_error(1, message.as_cstr());
        }
        SleepOutcome::Warn => {
            swoole_rs_zend::bailout::defer(swoole_rs_zend::report(
                E_WARNING,
                c"Argument #1 ($timestamp) must be greater than or equal to the current time",
            ));
            call.set_bool(false);
        }
        SleepOutcome::Done(_) | SleepOutcome::Interrupted { .. } => call.set_bool(true),
    }
}
