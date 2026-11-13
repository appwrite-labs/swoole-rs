//! The worker side's warnings, logged from Rust under the upstream function that
//! logged each. An operation names one with its `LOG_*` number and up to two
//! values ([`super::Host::log`]); a scripted host records the number instead.
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::log::{error_log, info, sys_error, sys_warning, warning};

// The numbers native code never sees: tests tell the logs apart by them.
pub(super) const LOG_INVALID_WORKER: u32 = 123;
pub(super) const LOG_UNKNOWN_MESSAGE: u32 = 214;
pub(super) const LOG_FORK_MANAGER: u32 = 221;
pub(super) const LOG_START_TASK_POOL: u32 = 224;
pub(super) const LOG_FORK_EVENT_WORKER: u32 = 227;
pub(super) const LOG_FORK_USER_WORKER: u32 = 229;
pub(super) const LOG_WAIT_FAILED: u32 = 267;
pub(super) const LOG_RELOADING: u32 = 269;
pub(super) const LOG_NO_TASK_RELOAD: u32 = 274;
pub(super) const LOG_KILL_TIMEOUT: u32 = 281;
pub(super) const LOG_UNKNOWN_CHILD: u32 = 285;
pub(super) const LOG_UNKNOWN_WORKER_TYPE: u32 = 291;
pub(super) const LOG_RELOAD_SINGLE_PROCESS: u32 = 294;
pub(super) const LOG_CANNOT_RELOAD: u32 = 382;
pub(super) const LOG_NOTIFY_MANAGER: u32 = 384;
pub(super) const LOG_MANAGER_ABNORMAL: u32 = 400;
pub(super) const LOG_THREAD_UNSUPPORTED: u32 = 406;

/// `SW_LOG_*`.
const WARNING: i32 = 4;
const ERROR: i32 = 5;
/// `SW_ERROR_*`.
const OPERATION_NOT_SUPPORT: i32 = 507;
const SERVER_WORKER_EXIT_TIMEOUT: i32 = 9101;

/// Logs `op` with its values `a` and `b`, as upstream's C++ formatted them.
pub(super) fn emit(op: u32, a: i64, b: i64) {
    let which = if a != 0 { "all" } else { "task" };
    match op {
        LOG_INVALID_WORKER => warning(
            c"ReactorProcess_onPipeRead",
            format_args!("invalid worker_id={a}"),
        ),
        LOG_THREAD_UNSUPPORTED => error_log(
            ERROR,
            OPERATION_NOT_SUPPORT,
            c"Server::create_thread_factory",
            format_args!("Thread support is not enabled, cannot create server with MODE_THREAD"),
        ),
        LOG_MANAGER_ABNORMAL => warning(
            c"ThreadFactory::spawn_manager_thread",
            format_args!("Fatal Error: manager thread exits abnormally"),
        ),
        LOG_CANNOT_RELOAD => error_log(
            WARNING,
            OPERATION_NOT_SUPPORT,
            c"ThreadFactory::reload",
            format_args!("Cannot reload {which} workers, task workers are not started"),
        ),
        LOG_NOTIFY_MANAGER => info(format_args!(
            "Send a notification to the manager process to prepare for restarting {which} \
             worker processes."
        )),
        LOG_RELOADING => info(format_args!("Server is reloading {which} workers now")),
        LOG_FORK_MANAGER => sys_warning(
            c"Server::start_manager_process",
            format_args!("failed fork manager process"),
        ),
        LOG_START_TASK_POOL => sys_error(
            c"Server::start_manager_process",
            format_args!("failed to start task worker"),
        ),
        LOG_FORK_EVENT_WORKER => sys_error(
            c"Server::start_manager_process",
            format_args!("failed to fork event worker"),
        ),
        LOG_FORK_USER_WORKER => sys_error(
            c"Server::start_manager_process",
            format_args!("failed to fork user worker"),
        ),
        LOG_WAIT_FAILED => sys_warning(c"Manager::wait", format_args!("wait() failed")),
        LOG_NO_TASK_RELOAD => warning(
            c"Manager::wait",
            format_args!("cannot reload task workers, task workers is not started"),
        ),
        LOG_KILL_TIMEOUT => error_log(
            WARNING,
            SERVER_WORKER_EXIT_TIMEOUT,
            c"Manager::terminate_all_worker",
            format_args!("wait timeout, all worker processes will be forcibly terminated"),
        ),
        LOG_UNKNOWN_CHILD => warning(
            c"Server::restart_worker_process",
            format_args!("Unknown type child process [{}] exited", a.c_cast::<i32>()),
        ),
        LOG_UNKNOWN_WORKER_TYPE => warning(
            c"Server::restart_worker_process",
            format_args!("Unknown worker [pid={}] type [{b}]", a.c_cast::<i32>()),
        ),
        LOG_UNKNOWN_MESSAGE => warning(
            c"Server::read_worker_message",
            format_args!("unknown worker message type[{a}]"),
        ),
        LOG_RELOAD_SINGLE_PROCESS => error_log(
            WARNING,
            OPERATION_NOT_SUPPORT,
            c"Server::reload",
            format_args!("not supported with single process mode"),
        ),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::methods::tests::{install, logged};

    /// `swr_v1_core_log_host`'s kinds.
    const SYS_WARNING: i32 = 0;
    const PLAIN_WARNING: i32 = 1;
    const ERROR_LOG: i32 = 3;
    const INFO: i32 = 4;
    const SYS_ERROR: i32 = 5;

    #[test]
    fn each_log_keeps_its_origin_level_code_and_values() {
        install();
        emit(LOG_UNKNOWN_WORKER_TYPE, 4242, 9);
        assert_eq!(
            logged("Server::restart_worker_process"),
            [(
                PLAIN_WARNING,
                0,
                0,
                "Unknown worker [pid=4242] type [9]".to_owned()
            )]
        );
    }

    #[test]
    fn reload_logs_name_all_or_task_workers() {
        install();
        emit(LOG_CANNOT_RELOAD, 0, 0);
        assert_eq!(
            logged("ThreadFactory::reload"),
            [(
                ERROR_LOG,
                WARNING,
                OPERATION_NOT_SUPPORT,
                "Cannot reload task workers, task workers are not started".to_owned()
            )]
        );
        emit(LOG_RELOADING, 1, 0);
        emit(LOG_NOTIFY_MANAGER, 0, 0);
        let infos: Vec<_> = logged("")
            .into_iter()
            .filter(|entry| entry.0 == INFO)
            .map(|entry| entry.3)
            .collect();
        assert!(infos.contains(&"Server is reloading all workers now".to_owned()));
        assert!(infos.contains(
            &"Send a notification to the manager process to prepare for restarting task worker \
              processes."
                .to_owned()
        ));
    }

    #[test]
    fn fork_failures_are_system_warnings_and_errors() {
        install();
        emit(LOG_FORK_MANAGER, 0, 0);
        emit(LOG_START_TASK_POOL, 0, 0);
        let kinds: Vec<_> = logged("Server::start_manager_process")
            .into_iter()
            .map(|(kind, _, _, message)| (kind, message))
            .collect();
        assert_eq!(
            kinds,
            [
                (SYS_WARNING, "failed fork manager process".to_owned()),
                (SYS_ERROR, "failed to start task worker".to_owned()),
            ]
        );
    }
}
