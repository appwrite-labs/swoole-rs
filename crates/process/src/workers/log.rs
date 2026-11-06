//! The worker side's warnings, logged from Rust under the upstream function that
//! logged each. An operation names one with its `LOG_*` number and up to two
//! values ([`super::Host::log`]); a scripted host records the number instead.
use swoole_rs_corebase::log::{error_log, info, sys_error, sys_warning, warning};

// The numbers native code never sees: tests tell the logs apart by them.
pub(super) const LOG_DISCARD: u32 = 19;
pub(super) const LOG_BAD_EVENT: u32 = 31;
pub(super) const LOG_UNPROCESSED: u32 = 47;
pub(super) const LOG_KILL_NO_ID: u32 = 51;
pub(super) const LOG_KILL_BAD_ID: u32 = 54;
pub(super) const LOG_PUSH_STOP: u32 = 72;
pub(super) const LOG_EXIT_TIMEOUT: u32 = 85;
pub(super) const LOG_INVALID_WORKER: u32 = 123;
pub(super) const LOG_COROUTINE_MSGQUEUE: u32 = 167;
pub(super) const LOG_NO_TASK_WORKERS: u32 = 187;
pub(super) const LOG_FINISH_IN_PIPE: u32 = 189;
pub(super) const LOG_FINISH_NOREPLY: u32 = 191;
pub(super) const LOG_LARGE_PACK: u32 = 195;
pub(super) const LOG_SEND_RESULT: u32 = 212;
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
pub(super) const LOG_FORK_EVENT: u32 = 317;
pub(super) const LOG_FORK_USER: u32 = 325;
pub(super) const LOG_INVALID_TARGET: u32 = 330;
pub(super) const LOG_CONN_NOT_ACTIVE: u32 = 334;
pub(super) const LOG_TOO_LARGE: u32 = 341;
pub(super) const LOG_SESSION_NOT_EXIST: u32 = 343;
pub(super) const LOG_SESSION_CLOSED: u32 = 345;
pub(super) const LOG_OVERFLOW: u32 = 351;
pub(super) const LOG_END_NOT_EXIST: u32 = 353;
pub(super) const LOG_SESSION_CLOSING: u32 = 362;
pub(super) const LOG_END_CLOSED: u32 = 364;
pub(super) const LOG_CANNOT_RELOAD: u32 = 382;
pub(super) const LOG_NOTIFY_MANAGER: u32 = 384;
pub(super) const LOG_MANAGER_ABNORMAL: u32 = 400;
pub(super) const LOG_THREAD_UNSUPPORTED: u32 = 406;

/// `SW_LOG_*`.
const TRACE: i32 = 1;
const NOTICE: i32 = 3;
const WARNING: i32 = 4;
const ERROR: i32 = 5;
/// `SW_ERROR_*`.
const INVALID_PARAMS: i32 = 505;
const OPERATION_NOT_SUPPORT: i32 = 507;
const WRONG_OPERATION: i32 = 509;
const SESSION_CLOSING: i32 = 1003;
const SESSION_CLOSED: i32 = 1004;
const SESSION_NOT_EXIST: i32 = 1005;
const SESSION_DISCARD_TIMEOUT_DATA: i32 = 1007;
const OUTPUT_BUFFER_OVERFLOW: i32 = 1009;
const DATA_LENGTH_TOO_LARGE: i32 = 1203;
const SERVER_SEND_TO_WOKER_TIMEOUT: i32 = 9014;
const SERVER_WORKER_EXIT_TIMEOUT: i32 = 9101;
const SERVER_WORKER_UNPROCESSED_DATA: i32 = 9103;

/// Logs `op` with its values `a` and `b`, as upstream's C++ formatted them.
#[expect(
    clippy::cast_possible_truncation,
    reason = "each value is the C int, uint32 or int16 upstream printed, widened to i64"
)]
#[expect(clippy::too_many_lines, reason = "one arm per upstream log statement")]
pub(super) fn emit(op: u32, a: i64, b: i64) {
    let which = if a != 0 { "all" } else { "task" };
    match op {
        LOG_FORK_EVENT => sys_warning(
            c"Factory::spawn_event_worker",
            format_args!("failed to fork event worker"),
        ),
        LOG_FORK_USER => sys_warning(
            c"Factory::spawn_user_worker",
            format_args!("failed to spawn the user worker"),
        ),
        LOG_INVALID_TARGET => warning(
            c"ProcessFactory::dispatch",
            format_args!("invalid target worker id[{}]", a as i32),
        ),
        LOG_CONN_NOT_ACTIVE => warning(
            c"ProcessFactory::dispatch",
            format_args!(
                "dispatch[type={a}] failed, connection#{} is not active",
                b as i32
            ),
        ),
        LOG_TOO_LARGE => error_log(
            WARNING,
            DATA_LENGTH_TOO_LARGE,
            c"ProcessFactory::finish",
            format_args!(
                "The length of data [{a}] exceeds the output buffer size[{b}], please use the \
                 sendfile, chunked transfer mode or adjust the output_buffer_size"
            ),
        ),
        LOG_SESSION_NOT_EXIST => error_log(
            TRACE,
            SESSION_NOT_EXIST,
            c"ProcessFactory::finish",
            format_args!("session#{a} does not exists"),
        ),
        LOG_SESSION_CLOSED => error_log(
            TRACE,
            SESSION_CLOSED,
            c"ProcessFactory::finish",
            format_args!(
                "send {} bytes failed, because session#{b} is closed",
                a as i32
            ),
        ),
        LOG_OVERFLOW => error_log(
            WARNING,
            OUTPUT_BUFFER_OVERFLOW,
            c"ProcessFactory::finish",
            format_args!("send failed, session={a} output buffer overflow"),
        ),
        LOG_END_NOT_EXIST => error_log(
            TRACE,
            SESSION_NOT_EXIST,
            c"ProcessFactory::end",
            format_args!("session#{a} does not exists"),
        ),
        LOG_SESSION_CLOSING => error_log(
            NOTICE,
            SESSION_CLOSING,
            c"ProcessFactory::end",
            format_args!("session#{a} is closing"),
        ),
        LOG_END_CLOSED => error_log(
            TRACE,
            SESSION_CLOSED,
            c"ProcessFactory::end",
            format_args!("session#{a} is closed"),
        ),
        LOG_DISCARD => error_log(
            WARNING,
            SESSION_DISCARD_TIMEOUT_DATA,
            c"Worker_discard_data",
            format_args!("[2] ignore data[{a} bytes] received from session#{b}"),
        ),
        LOG_BAD_EVENT => warning(
            c"Server::worker_accept_event",
            format_args!("[Worker] error event[type={a}]"),
        ),
        LOG_UNPROCESSED => error_log(
            WARNING,
            SERVER_WORKER_UNPROCESSED_DATA,
            c"Server::call_worker_stop_callback",
            format_args!("unprocessed data in the worker process buffer"),
        ),
        LOG_KILL_NO_ID => error_log(
            WARNING,
            WRONG_OPERATION,
            c"Server::kill_worker",
            format_args!("kill worker in non worker process requires specifying an id"),
        ),
        LOG_KILL_BAD_ID => error_log(
            WARNING,
            INVALID_PARAMS,
            c"Server::kill_worker",
            format_args!("the worker_id[{}] is invalid", a as i32),
        ),
        LOG_PUSH_STOP => sys_warning(
            c"Server::stop_async_worker",
            format_args!("failed to push WORKER_STOP message"),
        ),
        LOG_EXIT_TIMEOUT => error_log(
            WARNING,
            SERVER_WORKER_EXIT_TIMEOUT,
            c"Worker_reactor_try_to_exit",
            format_args!("worker exit timeout, forced termination"),
        ),
        LOG_INVALID_WORKER => warning(
            c"ReactorProcess_onPipeRead",
            format_args!("invalid worker_id={a}"),
        ),
        LOG_COROUTINE_MSGQUEUE => error_log(
            WARNING,
            WRONG_OPERATION,
            c"Server::init_task_workers",
            format_args!("cannot use msgqueue when task_enable_coroutine is enable"),
        ),
        LOG_NO_TASK_WORKERS => warning(
            c"Server::finish",
            format_args!(
                "cannot use Server::task()/Server::finish() method, because no set [task_worker_num]"
            ),
        ),
        LOG_FINISH_IN_PIPE => warning(
            c"Server::finish",
            format_args!(
                "Server::task()/Server::finish() is not supported in onPipeMessage callback"
            ),
        ),
        LOG_FINISH_NOREPLY => warning(
            c"Server::finish",
            format_args!("Server::finish() can only be used in the worker process"),
        ),
        LOG_LARGE_PACK => warning(c"Server::finish", format_args!("large task pack failed()")),
        LOG_SEND_RESULT if a != 0 => error_log(
            WARNING,
            SERVER_SEND_TO_WOKER_TIMEOUT,
            c"Server::finish",
            format_args!("send result to worker timed out"),
        ),
        LOG_SEND_RESULT => sys_warning(
            c"Server::finish",
            format_args!("send result to worker failed"),
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
            format_args!("Unknown type child process [{}] exited", a as i32),
        ),
        LOG_UNKNOWN_WORKER_TYPE => warning(
            c"Server::restart_worker_process",
            format_args!("Unknown worker [pid={}] type [{b}]", a as i32),
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
        emit(LOG_SESSION_CLOSED, 11, 7);
        emit(LOG_TOO_LARGE, 4_194_305, 2_097_152);
        emit(LOG_SESSION_CLOSING, 9, 0);
        assert_eq!(
            logged("ProcessFactory::finish"),
            [
                (
                    ERROR_LOG,
                    TRACE,
                    SESSION_CLOSED,
                    "send 11 bytes failed, because session#7 is closed".to_owned()
                ),
                (
                    ERROR_LOG,
                    WARNING,
                    DATA_LENGTH_TOO_LARGE,
                    "The length of data [4194305] exceeds the output buffer size[2097152], please \
                     use the sendfile, chunked transfer mode or adjust the output_buffer_size"
                        .to_owned()
                ),
            ]
        );
        assert_eq!(
            logged("ProcessFactory::end"),
            [(
                ERROR_LOG,
                NOTICE,
                SESSION_CLOSING,
                "session#9 is closing".to_owned()
            )]
        );
        emit(LOG_CONN_NOT_ACTIVE, 0, 12);
        emit(LOG_INVALID_TARGET, -3, 0);
        assert_eq!(
            logged("ProcessFactory::dispatch"),
            [
                (
                    PLAIN_WARNING,
                    0,
                    0,
                    "dispatch[type=0] failed, connection#12 is not active".to_owned()
                ),
                (
                    PLAIN_WARNING,
                    0,
                    0,
                    "invalid target worker id[-3]".to_owned()
                ),
            ]
        );
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
