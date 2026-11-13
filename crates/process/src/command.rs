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

/// Where a command goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    ReactorThread,
    EventWorker,
    TaskWorker,
    Manager,
    Master,
}

/// Why a command was rejected: the notice native code logs, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    /// The server has not started; nothing is logged.
    NotStarted,
    /// `Unknown command[%s]`.
    Unknown,
    /// `command() can only be used in master process`.
    NotMaster,
    /// `command() can only be used in worker process 0`.
    NotWorker0,
    /// `unsupported [process_type]`.
    UnsupportedType,
    /// `unsupported [server_mode]`.
    UnsupportedMode,
    /// `invalid thread_id[%d]`.
    InvalidThread,
    /// `invalid worker_id[%d]`.
    InvalidWorker,
    /// `invalid task_worker_id[%d]`.
    InvalidTaskWorker,
    /// `message is too large, ...`.
    TooLarge,
}

impl Rejection {
    /// Whether upstream had already taken a request id and stored the callback
    /// (it erases the callback again).
    #[must_use]
    pub const fn took_request(self) -> bool {
        !matches!(
            self,
            Self::NotStarted | Self::Unknown | Self::NotMaster | Self::NotWorker0
        )
    }
}

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

/// A command's target process, once aliases are applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandRoute {
    pub target: Target,
    pub process_id: u32,
}

/// A rejected command, with the process id its notice reports (`%d`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rejected {
    pub reason: Rejection,
    pub process_id: u32,
}

const fn fail(reason: Rejection, process_id: u32) -> Result<CommandRoute, Rejected> {
    Err(Rejected { reason, process_id })
}

/// Route a command as `Server::command` does.
///
/// # Errors
///
/// The check that rejects it.
pub fn route(input: &CommandInput) -> Result<CommandRoute, Rejected> {
    let mut process_id = input.process_id;
    if !input.started {
        return fail(Rejection::NotStarted, process_id);
    }
    if !input.found {
        return fail(Rejection::Unknown, process_id);
    }
    if input.process_mode && !input.master {
        return fail(Rejection::NotMaster, process_id);
    } else if input.base_mode && input.worker_id != 0 {
        return fail(Rejection::NotWorker0, process_id);
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
        return fail(Rejection::UnsupportedType, process_id);
    }
    // The limit and its rejection; no id reaches `u32::MAX`.
    let (target, limit, log) = match process_type {
        REACTOR_THREAD if !input.process_mode => {
            return fail(Rejection::UnsupportedMode, process_id);
        }
        REACTOR_THREAD => (
            Target::ReactorThread,
            input.reactor_num,
            Rejection::InvalidThread,
        ),
        EVENT_WORKER => (
            Target::EventWorker,
            input.worker_num,
            Rejection::InvalidWorker,
        ),
        TASK_WORKER => (
            Target::TaskWorker,
            input.task_worker_num,
            Rejection::InvalidTaskWorker,
        ),
        MANAGER if input.length >= input.manager_capacity => {
            return fail(Rejection::TooLarge, process_id);
        }
        MANAGER => (Target::Manager, u32::MAX, Rejection::TooLarge),
        MASTER => (Target::Master, u32::MAX, Rejection::TooLarge),
        _ => return fail(Rejection::UnsupportedType, process_id),
    };
    if process_id >= limit {
        return fail(log, process_id);
    }
    Ok(CommandRoute { target, process_id })
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
        assert_eq!(route(&input), fail(Rejection::NotStarted, 0));
        let mut input = started(true);
        input.found = false;
        assert_eq!(route(&input), fail(Rejection::Unknown, 0));
        let mut input = started(true);
        input.master = false;
        assert_eq!(route(&input), fail(Rejection::NotMaster, 0));
        let mut input = started(false);
        input.worker_id = 1;
        assert_eq!(route(&input), fail(Rejection::NotWorker0, 0));
        assert!(!Rejection::NotWorker0.took_request());
        assert!(Rejection::UnsupportedType.took_request());
    }

    #[test]
    fn aliases_route_to_the_master() {
        let mut input = started(false);
        input.process_type = EVENT_WORKER;
        assert_eq!(route(&input).map(|r| r.target), Ok(Target::Master));
        let mut input = started(true);
        input.process_type = REACTOR_THREAD;
        input.process_id = 2;
        let master = CommandRoute {
            target: Target::Master,
            process_id: 0,
        };
        assert_eq!(route(&input), Ok(master));
    }

    #[test]
    fn targets_are_bounded() {
        let mut input = started(true);
        input.process_type = EVENT_WORKER;
        input.process_id = 3;
        assert_eq!(route(&input).map(|r| r.target), Ok(Target::EventWorker));
        input.process_id = 4;
        assert_eq!(route(&input), fail(Rejection::InvalidWorker, 4));
        input.process_type = TASK_WORKER;
        input.process_id = 1;
        assert_eq!(route(&input), fail(Rejection::InvalidTaskWorker, 1));
        input.process_type = REACTOR_THREAD;
        input.process_id = 3;
        assert_eq!(route(&input), fail(Rejection::InvalidThread, 3));
        let mut base = started(false);
        base.process_type = REACTOR_THREAD;
        assert_eq!(route(&base), fail(Rejection::UnsupportedMode, 0));
        let mut input = started(true);
        input.process_type = MANAGER;
        assert_eq!(route(&input).map(|r| r.target), Ok(Target::Manager));
        input.length = 8180;
        assert_eq!(route(&input), fail(Rejection::TooLarge, 0));
    }

    #[test]
    fn unaccepted_or_unknown_types_are_unsupported() {
        let mut input = started(true);
        input.process_type = TASK_WORKER;
        input.accepted_types = EVENT_WORKER;
        assert_eq!(route(&input), fail(Rejection::UnsupportedType, 0));
        input.process_type = EVENT_WORKER | TASK_WORKER;
        input.accepted_types = EVENT_WORKER | TASK_WORKER;
        assert_eq!(route(&input), fail(Rejection::UnsupportedType, 0));
    }
}
