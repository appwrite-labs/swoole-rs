//! Task workers (upstream's `src/server/task_worker.cc`): the choices
//! `Server::init_task_workers` and `Server::finish` make. The pool's callbacks, the
//! task loop and the sends that answer a task are in `native`.
#![cfg_attr(
    not(any(test, feature = "upstream-symbols")),
    expect(dead_code, reason = "only the hybrid runs task workers")
)]
use crate::TaskIpcMode;
use swoole_rs_corebase::DataHead;
use swoole_rs_corebase::ServerEvent;
use swoole_rs_corebase::log;
use swoole_rs_process::task_sync::ResultSlot;

#[cfg(all(feature = "upstream-symbols", not(test)))]
pub mod native;

/// `serv->task_results[worker_id]`: the event worker's result slot, which task
/// workers answer its `taskwait`s in; `None` before the server made the slots, and
/// past the last.
#[must_use]
pub fn result_slot(serv: crate::view::Server<'_>, worker_id: u16) -> Option<ResultSlot<'_>> {
    const WORDS: usize = swoole_rs_corebase::constants::SW_IPC_MAX_SIZE as usize / 4;
    let start = usize::from(worker_id) * WORDS;
    let words = crate::shared::words(&serv.config().task_results).get(start..start + WORDS)?;
    words.try_into().ok().map(ResultSlot::of)
}

/// `swoole::TaskFlag` bits.
const TASK_CALLBACK: u16 = 1 << 3;
const TASK_COROUTINE: u16 = 1 << 5;
const TASK_NOREPLY: u16 = 1 << 7;
/// `SW_LOG_WARNING`, `SW_ERROR_WRONG_OPERATION`.
const SW_LOG_WARNING: i32 = swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as i32;
const WRONG_OPERATION: i32 = 509;

/// Whether task workers over IPC `mode` may run coroutines: a coroutine task worker
/// reads its pipe in an event loop, so not from a message queue (logged).
fn coroutines_allowed(mode: Option<TaskIpcMode>, coroutine: bool) -> bool {
    let queue = matches!(mode, Some(TaskIpcMode::MsgQueue | TaskIpcMode::Preemptive));
    let allowed = !coroutine || !queue;
    if !allowed {
        log::error_log(
            SW_LOG_WARNING,
            WRONG_OPERATION,
            c"Server::init_task_workers",
            format_args!("cannot use msgqueue when task_enable_coroutine is enable"),
        );
    }
    allowed
}

/// Whether task workers over IPC `mode` take their tasks from a System V queue.
fn schedule_by_sysvmsg(mode: Option<TaskIpcMode>) -> bool {
    mode == Some(TaskIpcMode::Preemptive)
}

/// The task `Server::finish` answers: `task`, unless there are no task workers or no
/// task, or it is a pipe message or wants no reply (each logged).
fn answerable(task_workers: u32, task: Option<&DataHead>) -> Option<&DataHead> {
    let refusal = if task_workers < 1 {
        "cannot use Server::task()/Server::finish() method, because no set [task_worker_num]"
    } else {
        let task = task?;
        if task.event() == Some(ServerEvent::PipeMessage) {
            "Server::task()/Server::finish() is not supported in onPipeMessage callback"
        } else if task.ext_flags & TASK_NOREPLY != 0 {
            "Server::finish() can only be used in the worker process"
        } else {
            return Some(task);
        }
    };
    log::warning(c"Server::finish", format_args!("{refusal}"));
    None
}

/// The flags a nonblocking result adds to `flags`: the task's callback flag, else its
/// coroutine flag.
fn nonblocking_flags(flags: u16, task_flags: u16) -> u16 {
    flags
        | if task_flags & TASK_CALLBACK != 0 {
            TASK_CALLBACK
        } else {
            task_flags & TASK_COROUTINE
        }
}

/// Writes the notification with `write`, again each time `would_block` says the pipe
/// was full and `writable` waited until it is not: the last write's result.
fn notify(
    mut write: impl FnMut() -> isize,
    mut would_block: impl FnMut() -> bool,
    mut writable: impl FnMut() -> bool,
) -> isize {
    loop {
        let written = write();
        if written < 0 && would_block() && writable() {
            continue;
        }
        break written;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TASK_NONBLOCK: u16 = 1 << 2;

    #[test]
    fn init_rejects_coroutines_over_message_queues() {
        for (mode, coroutine, ok, sysvmsg) in [
            (TaskIpcMode::UnixSock, false, true, false),
            (TaskIpcMode::Preemptive, false, true, true),
            (TaskIpcMode::MsgQueue, true, false, false),
            (TaskIpcMode::Preemptive, true, false, true),
            (TaskIpcMode::UnixSock, true, true, false),
        ] {
            assert_eq!(coroutines_allowed(Some(mode), coroutine), ok);
            assert_eq!(schedule_by_sysvmsg(Some(mode)), sysvmsg);
        }
    }

    #[test]
    fn finish_refuses_pipe_messages_noreply_and_no_task() {
        let task = DataHead::default();
        assert!(answerable(0, Some(&task)).is_none());
        assert!(answerable(1, None).is_none());
        let pipe = DataHead {
            kind: ServerEvent::PipeMessage.code(),
            ..DataHead::default()
        };
        assert!(answerable(1, Some(&pipe)).is_none());
        let noreply = DataHead {
            ext_flags: TASK_NOREPLY,
            ..DataHead::default()
        };
        assert!(answerable(1, Some(&noreply)).is_none());
        assert!(answerable(1, Some(&task)).is_some());
    }

    #[test]
    fn finish_nonblocking_adds_the_callback_or_coroutine_flag() {
        for (task_flags, added) in [
            (TASK_NONBLOCK, 0),
            (TASK_NONBLOCK | TASK_COROUTINE, TASK_COROUTINE),
            (
                TASK_NONBLOCK | TASK_CALLBACK | TASK_COROUTINE,
                TASK_CALLBACK,
            ),
        ] {
            assert_eq!(nonblocking_flags(1, task_flags), 1 | added);
        }
    }

    #[test]
    fn finish_notifies_again_once_a_full_pipe_is_writable() {
        let mut writes = [-1, 8].into_iter();
        let mut waits = 0;
        let written = notify(
            || writes.next().unwrap_or(0),
            || true,
            || {
                waits += 1;
                true
            },
        );
        assert_eq!((written, waits), (8, 1));
        // A failure that is not a full pipe, or a wait that fails, ends it.
        assert_eq!(notify(|| -1, || false, || true), -1);
        assert_eq!(notify(|| -1, || true, || false), -1);
    }
}
