//! `Server::command`'s routing: whether this process may send a command, which
//! process type and id it goes to once upstream's aliases are applied, and
//! which check rejects it. Native code keeps the command tables and callbacks
//! and performs the route (a pipe write, a task, a manager message or the
//! master's own handler).

/// `Server::Command::ProcessType`.
pub const MASTER: i32 = 1 << 1;
pub const REACTOR_THREAD: i32 = 1 << 2;
pub const EVENT_WORKER: i32 = 1 << 3;
pub const TASK_WORKER: i32 = 1 << 4;
pub const MANAGER: i32 = 1 << 5;

/// Where a command goes (`CommandRoute::action`).
pub const FAIL: i32 = 0;
pub const TO_REACTOR_THREAD: i32 = 1;
pub const TO_EVENT_WORKER: i32 = 2;
pub const TO_TASK_WORKER: i32 = 3;
pub const TO_MANAGER: i32 = 4;
pub const TO_MASTER: i32 = 5;

/// The notice native code logs for a rejected command.
pub const LOG_NONE: i32 = 0;
/// `Unknown command[%s]`.
pub const LOG_UNKNOWN: i32 = 1;
/// `command() can only be used in master process`.
pub const LOG_NOT_MASTER: i32 = 2;
/// `command() can only be used in worker process 0`.
pub const LOG_NOT_WORKER_0: i32 = 3;
/// `unsupported [process_type]`.
pub const LOG_UNSUPPORTED_TYPE: i32 = 4;
/// `unsupported [server_mode]`.
pub const LOG_UNSUPPORTED_MODE: i32 = 5;
/// `invalid thread_id[%d]`.
pub const LOG_INVALID_THREAD: i32 = 6;
/// `invalid worker_id[%d]`.
pub const LOG_INVALID_WORKER: i32 = 7;
/// `invalid task_worker_id[%d]`.
pub const LOG_INVALID_TASK_WORKER: i32 = 8;
/// `message is too large, ...`.
pub const LOG_TOO_LARGE: i32 = 9;

/// What `Server::command` knows when it is called.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CommandInput {
    pub started: bool,
    /// `commands.find(name)` found it.
    pub found: bool,
    pub process_mode: bool,
    pub base_mode: bool,
    pub master: bool,
    /// The command's `accepted_process_types`.
    pub accepted_types: i32,
    /// In base mode, this worker's id.
    pub worker_id: u32,
    pub process_id: u32,
    pub process_type: i32,
    pub reactor_num: u32,
    pub worker_num: u32,
    pub task_worker_num: u32,
    /// The message's length, and the most a manager message holds.
    pub length: usize,
    pub manager_capacity: usize,
}

/// The route.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandRoute {
    /// `TO_*`, or `FAIL`.
    pub action: i32,
    /// `LOG_*` to report before acting.
    pub log: i32,
    /// The target process id once aliases are applied (the log's `%d`).
    pub process_id: u32,
    /// Whether upstream took a request id and stored the callback (it erases
    /// the callback again on failure).
    pub request: bool,
}

const fn fail(log: i32, process_id: u32, request: bool) -> CommandRoute {
    CommandRoute {
        action: FAIL,
        log,
        process_id,
        request,
    }
}

/// Route a command as `Server::command` does.
#[must_use]
pub fn route(input: &CommandInput) -> CommandRoute {
    let mut process_id = input.process_id;
    if !input.started {
        return fail(LOG_NONE, process_id, false);
    }
    if !input.found {
        return fail(LOG_UNKNOWN, process_id, false);
    }
    if input.process_mode && !input.master {
        return fail(LOG_NOT_MASTER, process_id, false);
    } else if input.base_mode && input.worker_id != 0 {
        return fail(LOG_NOT_WORKER_0, process_id, false);
    }
    let mut process_type = input.process_type;
    if input.base_mode && process_type == EVENT_WORKER && process_id == 0 {
        process_type = MASTER;
    }
    if input.process_mode && process_type == REACTOR_THREAD && process_id == input.reactor_num {
        process_type = MASTER;
        process_id = 0;
    }
    if process_type & input.accepted_types == 0 {
        return fail(LOG_UNSUPPORTED_TYPE, process_id, true);
    }
    let (action, limit, log) = match process_type {
        REACTOR_THREAD if !input.process_mode => {
            return fail(LOG_UNSUPPORTED_MODE, process_id, true);
        }
        REACTOR_THREAD => (TO_REACTOR_THREAD, input.reactor_num, LOG_INVALID_THREAD),
        EVENT_WORKER => (TO_EVENT_WORKER, input.worker_num, LOG_INVALID_WORKER),
        TASK_WORKER => (
            TO_TASK_WORKER,
            input.task_worker_num,
            LOG_INVALID_TASK_WORKER,
        ),
        MANAGER if input.length >= input.manager_capacity => {
            return fail(LOG_TOO_LARGE, process_id, true);
        }
        MANAGER => (TO_MANAGER, u32::MAX, LOG_NONE),
        MASTER => (TO_MASTER, u32::MAX, LOG_NONE),
        _ => return fail(LOG_UNSUPPORTED_TYPE, process_id, true),
    };
    if process_id >= limit {
        return fail(log, process_id, true);
    }
    CommandRoute {
        action,
        log: LOG_NONE,
        process_id,
        request: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(mode_process: bool) -> CommandInput {
        CommandInput {
            started: true,
            found: true,
            process_mode: mode_process,
            base_mode: !mode_process,
            master: true,
            accepted_types: MASTER | REACTOR_THREAD | EVENT_WORKER | TASK_WORKER | MANAGER,
            reactor_num: 2,
            worker_num: 4,
            task_worker_num: 1,
            length: 10,
            manager_capacity: 8180,
            ..CommandInput::default()
        }
    }

    #[test]
    fn rejections_before_a_request_take_none() {
        let mut input = started(true);
        input.started = false;
        assert_eq!(route(&input), fail(LOG_NONE, 0, false));
        let mut input = started(true);
        input.found = false;
        assert_eq!(route(&input).log, LOG_UNKNOWN);
        let mut input = started(true);
        input.master = false;
        assert_eq!(route(&input), fail(LOG_NOT_MASTER, 0, false));
        let mut input = started(false);
        input.worker_id = 1;
        assert_eq!(route(&input), fail(LOG_NOT_WORKER_0, 0, false));
    }

    #[test]
    fn aliases_route_to_the_master() {
        let mut input = started(false);
        input.process_type = EVENT_WORKER;
        assert_eq!(route(&input).action, TO_MASTER);
        let mut input = started(true);
        input.process_type = REACTOR_THREAD;
        input.process_id = 2;
        let routed = route(&input);
        assert_eq!((routed.action, routed.process_id), (TO_MASTER, 0));
    }

    #[test]
    fn targets_are_bounded() {
        let mut input = started(true);
        input.process_type = EVENT_WORKER;
        input.process_id = 3;
        assert_eq!(route(&input).action, TO_EVENT_WORKER);
        input.process_id = 4;
        assert_eq!(route(&input), fail(LOG_INVALID_WORKER, 4, true));
        input.process_type = TASK_WORKER;
        input.process_id = 1;
        assert_eq!(route(&input), fail(LOG_INVALID_TASK_WORKER, 1, true));
        input.process_type = REACTOR_THREAD;
        input.process_id = 3;
        assert_eq!(route(&input), fail(LOG_INVALID_THREAD, 3, true));
        let mut base = started(false);
        base.process_type = REACTOR_THREAD;
        assert_eq!(route(&base), fail(LOG_UNSUPPORTED_MODE, 0, true));
        let mut input = started(true);
        input.process_type = MANAGER;
        assert_eq!(route(&input).action, TO_MANAGER);
        input.length = 8180;
        assert_eq!(route(&input), fail(LOG_TOO_LARGE, 0, true));
    }

    #[test]
    fn unaccepted_or_unknown_types_are_unsupported() {
        let mut input = started(true);
        input.process_type = TASK_WORKER;
        input.accepted_types = EVENT_WORKER;
        assert_eq!(route(&input), fail(LOG_UNSUPPORTED_TYPE, 0, true));
        input.process_type = EVENT_WORKER | TASK_WORKER;
        input.accepted_types = EVENT_WORKER | TASK_WORKER;
        assert_eq!(route(&input), fail(LOG_UNSUPPORTED_TYPE, 0, true));
    }
}
