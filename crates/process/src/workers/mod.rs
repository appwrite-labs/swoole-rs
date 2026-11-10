//! The server's worker side: event workers (upstream's `src/server/worker.cc`), the
//! base-mode reactor processes (upstream's `src/server/reactor_process.cc`), task workers
//! (upstream's `src/server/task_worker.cc`), the manager (upstream's `src/server/manager.cc`) and the
//! process-mode factory (upstream's `src/server/process.cc`), and thread mode
//! (upstream's `src/server/thread.cc`). Native code keeps
//! the objects (server, workers, reactor, ports, connections, callbacks, logs); Rust
//! runs each operation and asks native code for each fact or effect through one
//! `call(context, op, arg)`, reading facts afresh.
use core::ffi::c_void;

pub mod entry;
mod event_worker;
mod factory;
pub mod factory_methods;
mod log;
mod manager;
pub mod ops;
mod reactor_process;
mod task_worker;
pub mod thread;

/// One native fact or effect in `context`.
///
/// Effects run native callbacks (`onUserWorkerStart`, `onBeforeReload`, ...). A
/// PHP bailout longjmps over these frames to the PHP entry's `zend_try`, as it does
/// over upstream's C++ frames. A C++ exception has no first-party handler above
/// `Server::start` upstream, so it terminates the process; `C-unwind` makes that
/// defined here: under `panic = "abort"` it aborts at this call instead of
/// unwinding a `C` frame.
pub type WorkersCall = unsafe extern "C-unwind" fn(context: *mut c_void, op: u32, arg: u64) -> u64;

pub(crate) const SW_OK: i64 = 0;
pub(crate) const SW_ERR: i64 = -1;

/// `Server::Mode`.
pub(crate) const MODE_BASE: u64 = 1;
pub(crate) const MODE_PROCESS: u64 = 2;
pub(crate) const MODE_THREAD: u64 = 3;

/// What Rust asks of the native objects it runs over.
pub(crate) trait Host {
    fn op(&mut self, op: u32, arg: u64) -> u64;

    #[inline(always)]
    fn get(&mut self, op: u32) -> u64 {
        self.op(op, 0)
    }
    #[inline(always)]
    fn is(&mut self, op: u32) -> bool {
        self.get(op) != 0
    }
    #[inline(always)]
    fn signed(&mut self, op: u32) -> i64 {
        self.get(op).cast_signed()
    }
    /// Logs `log::LOG_*` with its values.
    fn log(&mut self, op: u32, a: i64, b: i64) {
        log::emit(op, a, b);
    }
}

/// What crates/server answers each fact or effect with, over a context it built.
pub trait Answer {
    fn answer(&mut self, op: u32, arg: u64) -> u64;
}

/// A context crates/server built, which Rust runs an operation over.
struct Over<'a, A: Answer + ?Sized>(&'a mut A);

impl<A: Answer + ?Sized> Host for Over<'_, A> {
    #[inline(always)]
    fn op(&mut self, op: u32, arg: u64) -> u64 {
        // Unit tests link no upstream objects, so native does these ops there.
        #[cfg(all(feature = "upstream-symbols", not(test)))]
        if let Some(result) = effect(op) {
            return result;
        }
        self.0.answer(op, arg)
    }
}

/// Runs `operation` with `arg` over `context`.
pub fn run_over(context: &mut dyn Answer, operation: u32, arg: u64) -> i64 {
    run(&mut Over(context), operation, arg)
}

/// Runs a task-worker `operation` with `arg` over `context`, compiled for its
/// type: each step's op is a constant, so an inlined answer folds to its arm.
pub fn run_task_worker<A: Answer>(context: &mut A, operation: u32, arg: u64) -> i64 {
    task_worker::run(&mut Over(context), operation, arg)
}

/// [`run_task_worker`] for an event-worker `operation`.
pub fn run_event_worker<A: Answer>(context: &mut A, operation: u32, arg: u64) -> i64 {
    event_worker::run(&mut Over(context), operation, arg)
}

struct Native {
    context: *mut c_void,
    call: WorkersCall,
}

impl Host for Native {
    fn op(&mut self, op: u32, arg: u64) -> u64 {
        // Unit tests link no upstream objects, so native does these ops there.
        #[cfg(all(feature = "upstream-symbols", not(test)))]
        if let Some(result) = effect(op) {
            return result;
        }
        // SAFETY: a native fact or effect on the live objects in `context`.
        unsafe { (self.call)(self.context, op, arg) }
    }
}

/// The `OP_*` facts and effects over thread- and process-wide state (the event
/// loop, the timer, the last error, the worker globals) rather than the objects
/// in a context. They mean the same in every context, so Rust does them; `None`
/// for the rest, which native code does.
#[cfg(all(feature = "upstream-symbols", not(test)))]
#[inline(always)]
fn effect(op: u32) -> Option<u64> {
    use core::ffi::c_int;
    use swoole_rs_corebase::log::{last_error, set_last_error};
    // SAFETY: crates/reactor's and crates/timer's functions, under their upstream
    // symbols. Declarations, since crates/reactor (and through it crates/timer)
    // depends on this crate.
    unsafe extern "C-unwind" {
        #[link_name = "_Z17swoole_event_initi"]
        fn swoole_event_init(flags: c_int) -> c_int;
        #[link_name = "_Z17swoole_event_freev"]
        fn swoole_event_free() -> c_int;
        #[link_name = "_Z25swoole_event_is_availablev"]
        fn swoole_event_is_available() -> bool;
        #[link_name = "_Z17swoole_event_waitv"]
        fn swoole_event_wait() -> c_int;
        #[link_name = "_Z19swoole_timer_createb"]
        fn swoole_timer_create(manually_trigger: bool) -> *mut c_void;
        #[link_name = "_Z25swoole_timer_is_availablev"]
        fn swoole_timer_is_available() -> bool;
        #[link_name = "_Z17swoole_timer_freev"]
        fn swoole_timer_free();
        #[link_name = "_Z19swoole_timer_selectv"]
        fn swoole_timer_select();
        #[link_name = "_Z26swoole_timer_get_next_msecv"]
        fn swoole_timer_get_next_msec() -> i64;

    }
    /// `SW_ERROR_OPERATION_NOT_SUPPORT`, `SW_ERROR_OUTPUT_SEND_YIELD`, `SW_ERROR_SOCKET_POLL_TIMEOUT`.
    const NOT_SUPPORT: c_int = 507;
    const SEND_YIELD: c_int = 1010;
    const POLL_TIMEOUT: c_int = 6002;
    let int = |value: c_int| i64::from(value).cast_unsigned();
    let globals = crate::worker_globals::thread_globals();
    Some(match op {
        // SAFETY: this thread's event loop.
        ops::OP_EVENT_INIT => int(unsafe { swoole_event_init(0) }),
        ops::OP_EVENT_FREE => {
            // SAFETY: as above.
            unsafe { swoole_event_free() };
            0
        }
        // SAFETY: as above.
        ops::OP_EVENT_AVAILABLE => u64::from(unsafe { swoole_event_is_available() }),
        // SAFETY: as above.
        ops::OP_EVENT_WAIT => int(unsafe { swoole_event_wait() }),
        ops::OP_TIMER_CREATE => {
            // SAFETY: this thread's timer.
            unsafe { swoole_timer_create(true) };
            0
        }
        ops::OP_TIMER_AVAILABLE | ops::OP_HAS_TIMER => {
            // SAFETY: as above.
            u64::from(unsafe { swoole_timer_is_available() })
        }
        ops::OP_TIMER_FREE => {
            // In the function that closes the timer, the scheduler is called again;
            // therefore, it is essential to set the scheduler to null after the timer has been consumed.
            // SAFETY: as above.
            if unsafe { swoole_timer_is_available() } {
                // SAFETY: the thread's timer, freed once.
                unsafe { swoole_timer_free() };
            }
            0
        }
        ops::OP_TIMER_SELECT => {
            // SAFETY: as above.
            unsafe { swoole_timer_select() };
            0
        }
        // SAFETY: as above.
        ops::OP_TIMER_NEXT_MSEC => unsafe { swoole_timer_get_next_msec() }.cast_unsigned(),
        // SAFETY: a plain syscall.
        ops::OP_ROOT_USER => u64::from(unsafe { libc::geteuid() } == 0),
        ops::OP_DEATH_SIGNAL => {
            swoole_rs_ossupport::swr_v1_os_set_process_death_signal(libc::SIGTERM);
            0
        }
        // SAFETY: this thread's worker globals.
        ops::OP_HAS_CURRENT_WORKER => u64::from(!unsafe { (*globals).worker }.is_null()),
        // SAFETY: as above.
        ops::OP_EXIT_TIME => unsafe { (*globals).exit_time }.cast_unsigned(),
        ops::OP_WORKER_STOPPED => {
            crate::worker_globals::worker_global_stopped();
            0
        }
        ops::OP_CLEAR_LAST_ERROR => {
            set_last_error(0);
            swoole_rs_error::swoole_clear_last_error_msg();
            0
        }
        ops::OP_SET_SEND_YIELD => {
            set_last_error(SEND_YIELD);
            0
        }
        ops::OP_SET_NOT_SUPPORTED => {
            set_last_error(NOT_SUPPORT);
            0
        }
        ops::OP_SEND_TIMED_OUT => {
            let errno = std::io::Error::last_os_error().raw_os_error();
            u64::from(
                errno == Some(libc::EAGAIN)
                    || errno == Some(libc::ETIMEDOUT)
                    || last_error() == POLL_TIMEOUT,
            )
        }
        _ => return None,
    })
}

/// Runs one worker-side operation (`ops::RUN_*`) with `arg`; the result is the
/// operation's return value as upstream's C++ function returns it.
///
/// # Safety
/// `call` acts on the objects in `context`, live for the call.
pub unsafe fn workers_run(
    operation: u32,
    arg: u64,
    context: *mut c_void,
    call: WorkersCall,
) -> i64 {
    run(&mut Native { context, call }, operation, arg)
}

fn run<H: Host>(h: &mut H, operation: u32, arg: u64) -> i64 {
    match operation {
        ops::RUN_REACTOR_PROCESS_START
        | ops::RUN_REACTOR_PROCESS_PIPE_READ
        | ops::RUN_REACTOR_PROCESS_MAIN
        | ops::RUN_REACTOR_PROCESS_CLOSE
        | ops::RUN_REACTOR_PROCESS_TIMEOUT => reactor_process::run(h, operation),
        ops::RUN_INIT_TASK_WORKERS
        | ops::RUN_TASK_COMMAND
        | ops::RUN_TASK_ON_TASK
        | ops::RUN_TASK_ON_START
        | ops::RUN_TASK_ON_STOP
        | ops::RUN_TASK_ON_PIPE
        | ops::RUN_TASK_LOOP_ASYNC
        | ops::RUN_FINISH => task_worker::run(h, operation, arg),
        ops::RUN_START_MANAGER
        | ops::RUN_MANAGER_MAIN
        | ops::RUN_MANAGER_SIGNAL
        | ops::RUN_RESTART_WORKER
        | ops::RUN_READ_WORKER_MESSAGE
        | ops::RUN_RELOAD => manager::run(h, operation, arg),
        ops::RUN_CREATE_PROCESS_FACTORY
        | ops::RUN_DESTROY_PROCESS_FACTORY
        | ops::RUN_KILL_TASK_WORKERS
        | ops::RUN_SPAWN_EVENT_WORKER
        | ops::RUN_SPAWN_USER_WORKER
        | ops::RUN_CHECK_EXIT_STATUS
        | ops::RUN_PROCESS_FACTORY_SHUTDOWN
        | ops::RUN_CREATE_WORKER_PIPES
        | ops::RUN_PROCESS_FACTORY_START
        | ops::RUN_PROCESS_DISPATCH
        | ops::RUN_PROCESS_FINISH
        | ops::RUN_PROCESS_END => factory::run(h, operation, arg),
        ops::RUN_THREAD_MANAGER_WAIT
        | ops::RUN_THREAD_RELOAD
        | ops::RUN_THREAD_TERMINATE_MANAGER
        | ops::RUN_START_WORKER_THREADS
        | ops::RUN_STOP_WORKER_THREADS
        | ops::RUN_THREAD_BODY
        | ops::RUN_THREAD_MAIN
        | ops::RUN_THREAD_FACTORY_START
        | ops::RUN_CREATE_THREAD_FACTORY
        | ops::RUN_DESTROY_THREAD_FACTORY
        | ops::RUN_THREAD_ID => thread::run(h, operation, arg),
        _ => event_worker::run(h, operation, arg),
    }
}

/// `Server::foreach_connection`: each valid connection from `min_fd` to `max_fd`
/// (both read afresh each step), selected in turn as the context's connection.
pub(crate) fn for_each_connection<H: Host>(h: &mut H, mut each: impl FnMut(&mut H)) {
    let mut fd = h.signed(ops::OP_MINFD);
    while fd <= h.signed(ops::OP_MAXFD) {
        if h.op(ops::OP_CONN_AT, fd.cast_unsigned()) != 0 {
            each(h);
        }
        fd += 1;
    }
}

/// `signal_init`'s handler kinds for `OP_SIGNAL_SET`.
pub(crate) const HANDLER_NONE: u64 = 0;
pub(crate) const HANDLER_IGNORE: u64 = 1;
pub(crate) const HANDLER_OWN: u64 = 2;
/// `OP_SIGNAL_SET`'s stand-in for SIGRTMIN, which only native code can name.
pub(crate) const SIGNAL_RTMIN: i32 = -1;

pub(crate) fn set_signal<H: Host + ?Sized>(h: &mut H, signo: i32, handler: u64) {
    h.op(
        ops::OP_SIGNAL_SET,
        u64::from(signo.cast_unsigned()) | (handler << 32),
    );
}

/// `set_signal` for each (signal, handler) in turn.
pub(crate) fn set_signal_all<H: Host + ?Sized>(h: &mut H, signals: &[(i32, u64)]) {
    for &(signo, handler) in signals {
        set_signal(h, signo, handler);
    }
}

#[cfg(test)]
pub(crate) mod script {
    //! A scripted native side: answers facts from a table and records every op.
    use super::Host;
    use std::collections::HashMap;

    #[derive(Default)]
    pub(crate) struct Script {
        pub facts: HashMap<u32, u64>,
        /// Answers consumed in order before `facts` for ops called repeatedly.
        pub queued: HashMap<u32, Vec<u64>>,
        pub log: Vec<(u32, u64)>,
    }

    impl Script {
        pub(crate) fn with(facts: &[(u32, u64)]) -> Self {
            Self {
                facts: facts.iter().copied().collect(),
                ..Self::default()
            }
        }
        pub(crate) fn queue(mut self, op: u32, answers: &[u64]) -> Self {
            self.queued.insert(op, answers.to_vec());
            self
        }
        pub(crate) fn called(&self, op: u32) -> usize {
            self.log.iter().filter(|(o, _)| *o == op).count()
        }
        pub(crate) fn args(&self, op: u32) -> Vec<u64> {
            self.log
                .iter()
                .filter(|(o, _)| *o == op)
                .map(|(_, a)| *a)
                .collect()
        }
        /// The ops called, in order, without the facts read (`get`s of `reads`).
        pub(crate) fn effects(&self, reads: &[u32]) -> Vec<u32> {
            self.log
                .iter()
                .map(|(o, _)| *o)
                .filter(|o| !reads.contains(o))
                .collect()
        }
    }

    impl Host for Script {
        fn op(&mut self, op: u32, arg: u64) -> u64 {
            self.log.push((op, arg));
            if let Some(answers) = self.queued.get_mut(&op)
                && !answers.is_empty()
            {
                return answers.remove(0);
            }
            self.facts.get(&op).copied().unwrap_or(0)
        }
        fn log(&mut self, op: u32, a: i64, _: i64) {
            self.log.push((op, a.cast_unsigned()));
        }
    }
}
