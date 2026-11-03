//! Copied worker/process transitions. Native alone executes callbacks and owns stores.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
pub mod affinity;
pub mod command;
pub mod pool;
pub mod reaper;
pub mod reload_task;
pub mod signal;
#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
pub mod signal_kqueue;
#[cfg(target_os = "linux")]
pub mod signalfd;
pub mod task_file;
pub mod task_sync;
pub mod wait;
pub mod worker_record;
pub mod worker_registry;
pub mod workers;
use std::ffi::c_int;
///0 ordinary task,1 pipe message,2 shutdown,3 command. No callback executes here.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_process_task_action(pipe: bool, shutdown: bool, command: bool) -> u32 {
    if pipe {
        1
    } else if shutdown {
        2
    } else if command {
        3
    } else {
        0
    }
}
/// Pipe-message prohibition precedes NOREPLY, matching native diagnostic order.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_process_finish_error(pipe: bool, no_reply: bool) -> u32 {
    if pipe {
        1
    } else if no_reply {
        2
    } else {
        0
    }
}
/// The callback result flag wins over the coroutine result flag.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_process_result_flags(
    flags: i32,
    callback: bool,
    coroutine: bool,
    callback_flag: i32,
    coroutine_flag: i32,
) -> i32 {
    flags
        | if callback {
            callback_flag
        } else if coroutine {
            coroutine_flag
        } else {
            0
        }
}
/// Ordinary manager-loop reload selection:0none,1all workers,2task workers.
/// Signal callbacks and `Server::reload` itself stay native.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_process_reload_action(all: bool, task: bool) -> u32 {
    if all {
        1
    } else if task {
        2
    } else {
        0
    }
}
/// SIGRTMIN is a runtime value on Linux and undefined elsewhere (upstream guards
/// it with `#ifdef SIGRTMIN`).
#[cfg(target_os = "linux")]
fn is_sigrtmin(signo: c_int) -> bool {
    signo == libc::SIGRTMIN()
}
#[cfg(not(target_os = "linux"))]
fn is_sigrtmin(_signo: c_int) -> bool {
    false
}
/// `Manager::signal_handler`'s action for `signo`:0 none,1 stop (SIGTERM),
/// 2 reload all workers (SIGUSR1),3 reload workers only (SIGUSR2),4 read a worker
/// message (SIGIO),5 force-kill if armed (SIGALRM),6 reopen the logger (SIGWINCH or
/// SIGRTMIN). The manager-pid guard and every effect stay native.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_process_manager_signal_action(signo: c_int) -> u32 {
    if signo == libc::SIGTERM {
        1
    } else if signo == libc::SIGUSR1 {
        2
    } else if signo == libc::SIGUSR2 {
        3
    } else if signo == libc::SIGIO {
        4
    } else if signo == libc::SIGALRM {
        5
    } else if signo == libc::SIGWINCH || is_sigrtmin(signo) {
        6
    } else {
        0
    }
}
/// Held by tests that install process-wide signal state (handlers, the signalfd
/// mask), which `cargo test`'s parallel threads would otherwise race on (#534).
#[cfg(test)]
pub(crate) fn signal_state_lock() -> std::sync::MutexGuard<'static, ()> {
    static STATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worker_task_and_reload_decisions_match_the_native_expressions() {
        for bits in 0..16 {
            let [a, b, c] = [bits & 1 != 0, bits & 2 != 0, bits & 4 != 0];
            assert_eq!(
                swr_v1_process_task_action(a, b, c),
                if a {
                    1
                } else if b {
                    2
                } else {
                    u32::from(c) * 3
                }
            );
            assert_eq!(
                swr_v1_process_finish_error(a, b),
                if a { 1 } else { u32::from(b) * 2 }
            );
            assert_eq!(
                swr_v1_process_reload_action(a, b),
                if a { 1 } else { u32::from(b) * 2 }
            );
        }
    }
    #[test]
    fn result_flags_add_the_callback_or_coroutine_flag() {
        let mut random = 0x0518_bc19_u32;
        for _ in 0..100_000 {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            let flags = random.cast_signed();
            for (callback, coroutine) in
                [(false, false), (true, false), (false, true), (true, true)]
            {
                let expected = if callback {
                    flags | 4
                } else if coroutine {
                    flags | 8
                } else {
                    flags
                };
                assert_eq!(
                    swr_v1_process_result_flags(flags, callback, coroutine, 4, 8),
                    expected
                );
            }
        }
    }
    #[test]
    fn diagnostic_and_mode_precedence() {
        assert_eq!(swr_v1_process_finish_error(true, true), 1);
        assert_eq!(swr_v1_process_task_action(true, true, true), 1);
        assert_eq!(swr_v1_process_reload_action(true, true), 1);
        assert_eq!(swr_v1_process_reload_action(false, true), 2);
    }
}
