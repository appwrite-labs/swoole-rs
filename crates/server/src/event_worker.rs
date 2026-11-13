//! Event workers (upstream's `src/server/worker.cc`): the choices a worker makes
//! about its signals, the data it receives and how long it waits to exit. The event
//! switch, the start and stop callbacks, the async stop and the process-mode main loop
//! are in `native`.
#![cfg_attr(
    not(any(test, feature = "upstream-symbols")),
    expect(dead_code, reason = "only the hybrid runs event workers")
)]
use core::ffi::c_int;

#[cfg(all(feature = "upstream-symbols", not(test)))]
pub mod native;

/// `signal_init`'s handlers: none, `SIG_IGN` or the worker's own.
pub(crate) const HANDLER_NONE: u64 = 0;
pub(crate) const HANDLER_IGNORE: u64 = 1;
pub(crate) const HANDLER_OWN: u64 = 2;
/// The stand-in for SIGRTMIN, which only Linux names.
pub(crate) const SIGNAL_RTMIN: c_int = -1;

/// `Server::worker_signal_init`'s signals in upstream's order, each with its handler;
/// `sigpipe` is SIGPIPE's (event workers ignore it, task workers leave it).
pub(crate) fn signals(sigpipe: u64) -> [(c_int, u64); 7] {
    [
        (libc::SIGHUP, HANDLER_NONE),
        (libc::SIGPIPE, sigpipe),
        (libc::SIGUSR1, HANDLER_NONE),
        (libc::SIGUSR2, HANDLER_NONE),
        (libc::SIGTERM, HANDLER_OWN),
        (libc::SIGWINCH, HANDLER_OWN),
        (SIGNAL_RTMIN, HANDLER_OWN),
    ]
}

/// Whether received data is discarded: `closed` is `None` without a connection, else
/// whether the server closed it. Without one, only `disable_notify` keeps the data,
/// unless timed-out requests are discarded too.
fn discard(closed: Option<bool>, disable_notify: bool, discard_timeout_request: bool) -> bool {
    closed.unwrap_or(!disable_notify || discard_timeout_request)
}

/// What `Worker_reactor_try_to_exit` does while the reactor can't exit yet.
#[derive(Debug, PartialEq, Eq)]
enum Wait {
    /// `max_wait_time` has passed: stop the reactor.
    Stop,
    /// Wait at most this many milliseconds for the reactor's next event.
    Timeout(i32),
    /// The reactor's timeout is already within the time left.
    Keep,
}

/// [`Wait`] at `now`, the stop having started at `exit_time`, with `max_wait` seconds
/// allowed and the reactor's `timeout` in milliseconds (negative: none). Upstream's
/// `int` arithmetic over `time_t`.
fn exit_wait(now: i64, exit_time: i64, max_wait: u32, timeout: i32) -> Wait {
    let remaining = (i64::from(max_wait) - (now - exit_time)).clamp(0, i64::from(i32::MAX));
    let remaining = i32::try_from(remaining).unwrap_or(0);
    if remaining == 0 {
        return Wait::Stop;
    }
    let limit = remaining.wrapping_mul(1000);
    if timeout < 0 || timeout > limit {
        Wait::Timeout(limit)
    } else {
        Wait::Keep
    }
}

/// The workers whose master pipes thread-mode reactor `reactor` of `reactors` serves.
fn reactor_pipes(workers: u32, reactors: u16, reactor: u16) -> impl Iterator<Item = u32> {
    let (reactors, reactor) = (u32::from(reactors), u32::from(reactor));
    (0..workers).filter(move |worker| reactors != 0 && worker % reactors == reactor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recv_data_discards_for_closed_or_unknown_connections() {
        // (connection, disable_notify, discard_timeout_request) -> discarded
        for (closed, disable_notify, discard_timeout, discarded) in [
            (Some(false), false, false, false),
            (Some(true), false, false, true),
            (None, false, false, true),
            (None, true, false, false),
            (None, true, true, true),
        ] {
            assert_eq!(discard(closed, disable_notify, discard_timeout), discarded);
        }
    }

    #[test]
    fn signals_install_in_upstream_order() {
        assert_eq!(
            signals(HANDLER_IGNORE),
            [
                (libc::SIGHUP, HANDLER_NONE),
                (libc::SIGPIPE, HANDLER_IGNORE),
                (libc::SIGUSR1, HANDLER_NONE),
                (libc::SIGUSR2, HANDLER_NONE),
                (libc::SIGTERM, HANDLER_OWN),
                (libc::SIGWINCH, HANDLER_OWN),
                (SIGNAL_RTMIN, HANDLER_OWN),
            ]
        );
        assert_eq!(signals(HANDLER_NONE)[1], (libc::SIGPIPE, HANDLER_NONE));
    }

    #[test]
    fn try_exit_bounds_the_wait_by_max_wait_time() {
        // Just stopped, 3 seconds allowed, no timeout: wait at most 3 seconds.
        assert_eq!(exit_wait(100, 100, 3, -1), Wait::Timeout(3000));
        assert_eq!(exit_wait(101, 100, 3, 5000), Wait::Timeout(2000));
        // Past the limit: stop.
        assert_eq!(exit_wait(110, 100, 3, -1), Wait::Stop);
        // A shorter timeout stands.
        assert_eq!(exit_wait(100, 100, 30, 500), Wait::Keep);
    }

    #[test]
    fn stop_async_in_thread_mode_unreads_this_reactors_pipes() {
        assert_eq!(reactor_pipes(5, 2, 1).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(reactor_pipes(5, 0, 0).count(), 0);
    }
}
