//! The server's worker side: event workers (upstream's `src/server/worker.cc`), the
//! manager (upstream's `src/server/manager.cc`) and thread mode (upstream's
//! `src/server/thread.cc`).
//! crates/server keeps the objects (server, workers, reactor, ports, connections, callbacks, logs); Rust
//! runs each operation and asks crates/server for each fact or effect through its
//! context's [`Answer`], reading facts afresh.
mod log;
pub mod ops;
pub mod thread;

pub(crate) const SW_OK: i64 = 0;
pub(crate) const SW_ERR: i64 = -1;

/// `SW_EVENT_WORKER`.
pub(crate) const EVENT_WORKER: u64 = crate::worker_record::WorkerType::Event as u64;

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

/// The `OP_*` facts and effects over thread- and process-wide state (the event
/// loop, the timer, the last error, the worker globals) rather than the objects
/// in a context. They mean the same in every context, so Rust does them; `None`
/// for the rest, which native code does.
#[cfg(all(feature = "upstream-symbols", not(test)))]
#[inline(always)]
fn effect(op: u32) -> Option<u64> {
    use core::ffi::{c_int, c_void};
    use swoole_rs_corebase::log::set_last_error;
    // crates/reactor's and crates/timer's safe functions, under their
    // upstream symbols and signatures. Declarations, since crates/reactor (and
    // through it crates/timer) depends on this crate.
    unsafe extern "C-unwind" {
        #[link_name = "_Z17swoole_event_initi"]
        safe fn swoole_event_init(flags: c_int) -> c_int;
        #[link_name = "_Z17swoole_event_freev"]
        safe fn swoole_event_free() -> c_int;
        #[link_name = "_Z25swoole_event_is_availablev"]
        safe fn swoole_event_is_available() -> bool;
        #[link_name = "_Z17swoole_event_waitv"]
        safe fn swoole_event_wait() -> c_int;
        #[link_name = "_Z19swoole_timer_createb"]
        safe fn swoole_timer_create(manually_trigger: bool) -> *mut c_void;
        #[link_name = "_Z25swoole_timer_is_availablev"]
        safe fn swoole_timer_is_available() -> bool;
        #[link_name = "_Z17swoole_timer_freev"]
        safe fn swoole_timer_free();
        #[link_name = "_Z19swoole_timer_selectv"]
        safe fn swoole_timer_select();
        #[link_name = "_Z26swoole_timer_get_next_msecv"]
        safe fn swoole_timer_get_next_msec() -> i64;

    }
    /// `SW_ERROR_OPERATION_NOT_SUPPORT`.
    const NOT_SUPPORT: c_int = 507;
    let int = |value: c_int| i64::from(value).cast_unsigned();
    Some(match op {
        ops::OP_EVENT_INIT => int(swoole_event_init(0)),
        ops::OP_EVENT_FREE => {
            swoole_event_free();
            0
        }
        ops::OP_EVENT_AVAILABLE => u64::from(swoole_event_is_available()),
        ops::OP_EVENT_WAIT => int(swoole_event_wait()),
        ops::OP_TIMER_CREATE => {
            swoole_timer_create(true);
            0
        }
        ops::OP_TIMER_AVAILABLE | ops::OP_HAS_TIMER => u64::from(swoole_timer_is_available()),
        ops::OP_TIMER_FREE => {
            // In the function that closes the timer, the scheduler is called again;
            // therefore, it is essential to set the scheduler to null after the timer has been consumed.
            if swoole_timer_is_available() {
                swoole_timer_free();
            }
            0
        }
        ops::OP_TIMER_SELECT => {
            swoole_timer_select();
            0
        }
        ops::OP_TIMER_NEXT_MSEC => swoole_timer_get_next_msec().cast_unsigned(),
        ops::OP_ROOT_USER => u64::from(swoole_rs_ossupport::swr_v1_os_is_root_user()),
        ops::OP_DEATH_SIGNAL => {
            swoole_rs_ossupport::swr_v1_os_set_process_death_signal(libc::SIGTERM);
            0
        }
        ops::OP_SET_NOT_SUPPORTED => {
            set_last_error(NOT_SUPPORT);
            0
        }
        _ => return None,
    })
}

/// Runs one worker-side operation (`ops::RUN_*`) with `arg`; the result is the
/// operation's return value as upstream's C++ function returns it.
fn run<H: Host>(h: &mut H, operation: u32, arg: u64) -> i64 {
    match operation {
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
        _ => SW_ERR,
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
