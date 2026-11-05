//! Coroutine I/O policy over copied scalars, and whole hooked operations moving
//! into Rust (`sleep`). Policy functions make no native callbacks, syscalls or
//! allocations and retain no borrows.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

pub mod address;
pub mod buffered;
pub mod co_io;
pub mod co_socket;
pub mod connect;
pub mod dns_cache;
pub mod fd_control;
pub mod file_io;
pub mod flock;
pub mod lifecycle;
pub mod offload;
pub mod packet;
pub mod poll;
pub mod resolve;
pub mod sendfile;
pub mod setup;
pub mod shutdown;
pub mod sleep;
pub mod socket_extra;
pub mod socket_map;
pub mod socket_members;
pub mod socket_wait;
pub mod stdio;
pub mod timers;
pub mod transfer;
#[cfg(feature = "iouring")]
pub mod uring;
#[cfg(feature = "iouring")]
pub mod uring_socket;
#[cfg(feature = "iouring")]
pub mod uring_tls;
pub mod vector;
pub mod wait_event;
pub mod wait_signal;

/// The original per-hook guard. Some DNS hooks intentionally do not use it.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_no_coro(reactor: bool, coroutine: bool) -> bool {
    !reactor || !coroutine
}

/// 1 install, -1 restore, 0 retain. Requested flags and installed flags differ.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_transition(requested: u32, installed: u32, bit: u32) -> i32 {
    match (requested & bit != 0, installed & bit != 0) {
        (true, false) => 1,
        (false, true) => -1,
        _ => 0,
    }
}

/// Filter only unavailable families, after native before-hook callbacks.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_filter(flags: u32, unavailable: u32) -> u32 {
    flags & !unavailable
}

/// File operation result: open's fd zero rejection is deliberate pinned policy.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_file_result(open: bool, result: i64) -> bool {
    if open { result > 0 } else { result == 0 }
}

/// `wait_for` and flock's next delay: double up to `maximum`, then start over at `initial`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_backoff(current: f64, initial: f64, maximum: f64) -> f64 {
    if current >= maximum {
        initial
    } else {
        let doubled = current * 2.0;
        // Preserve native SW_MIN rather than f64::min's different NaN policy.
        if doubled < maximum { doubled } else { maximum }
    }
}

/// Return the native partial-progress result, preserving unsigned `size_t` to
/// `ssize_t` conversion on the supported LP64 targets.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_transfer_result(result: i64, total: u64) -> i64 {
    if result < 0 && total == 0 {
        -1
    } else {
        total.cast_signed()
    }
}

#[repr(C)]
#[derive(Debug, PartialEq, Eq)]
pub struct Transfer {
    pub total: u64,
    pub again: bool,
}

/// Receive/send-all barrier. The native catch-error call is evaluated first
/// only for a negative result; its copied `SW_WAIT` fact comes here afterward.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_transfer(
    result: i64,
    total: u64,
    requested: u64,
    wait: bool,
) -> Transfer {
    let total = total.wrapping_add(if result > 0 { result.unsigned_abs() } else { 0 });
    Transfer {
        total,
        again: (result < 0 && wait) || (result > 0 && total < requested),
    }
}

#[repr(C)]
#[derive(Debug, PartialEq, Eq)]
pub struct EventPlan {
    pub revents: u32,
    pub wake: bool,
}

/// Native `EventWaiter` queues at most one deferred wake before publishing bits.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_event_waiter(received: u32, event: u32) -> EventPlan {
    EventPlan {
        revents: received | event,
        wake: received == 0,
    }
}

/// `System::wait_event` strips error readiness and adds only requested IO bits.
#[must_use]
pub fn swr_v1_hook_wait_revents(requested: u32, received: u32, read_write: u32, error: u32) -> u32 {
    if received & error != 0 {
        (received ^ error) | (requested & read_write)
    } else {
        received
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CurlPlan {
    pub flags: u32,
    pub conflict: bool,
}

/// Local-by-value normalization; native warning must finish before installed
/// hook flags are observed again because its PHP handler may reenter hooks.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_curl_flags(flags: u32, native: u32, php: u32) -> CurlPlan {
    let conflict = flags & native != 0 && flags & php != 0;
    CurlPlan {
        flags: if conflict { flags ^ php } else { flags },
        conflict,
    }
}

/// Preserve the pinned two divisions rather than changing timeout rounding.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_timeval(seconds: f64, micros: f64) -> f64 {
    seconds + micros / 1000.0 / 1000.0
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_dns_error(
    error: i32,
    asynchronous_timeout: i32,
    resolve_timeout: i32,
) -> i32 {
    if error == asynchronous_timeout {
        resolve_timeout
    } else {
        error
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_sleep_timeout(seconds: f64, minimum: f64) -> f64 {
    if seconds < minimum { minimum } else { seconds }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transfer_preserves_partial_error_and_wrap() {
        assert_eq!(swr_v1_hook_transfer_result(-1, 0), -1);
        assert_eq!(swr_v1_hook_transfer_result(-1, 12), 12);
        assert_eq!(
            swr_v1_hook_transfer(3, 4, 10, false),
            Transfer {
                total: 7,
                again: true
            }
        );
        assert_eq!(
            swr_v1_hook_transfer(3, 7, 10, false),
            Transfer {
                total: 10,
                again: false
            }
        );
        assert_eq!(
            swr_v1_hook_transfer(-1, 7, 10, true),
            Transfer {
                total: 7,
                again: true
            }
        );
        assert_eq!(
            swr_v1_hook_transfer(0, 7, 10, true),
            Transfer {
                total: 7,
                again: false
            }
        );
        assert_eq!(swr_v1_hook_transfer(2, u64::MAX, 10, false).total, 1);
    }
    #[test]
    fn flags_are_installed_vs_requested() {
        assert_eq!(swr_v1_hook_transition(1, 0, 1), 1);
        assert_eq!(swr_v1_hook_transition(0, 1, 1), -1);
        assert_eq!(swr_v1_hook_transition(1, 1, 1), 0);
        assert_eq!(swr_v1_hook_filter(u32::MAX, 5), u32::MAX ^ 5);
    }
}

mod socket_scope;
pub use socket_scope::*;
