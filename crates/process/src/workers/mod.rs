//! The server's worker side: event workers (`src/server/worker.cc`) and the
//! base-mode reactor processes (`src/server/reactor_process.cc`). Native code keeps
//! the objects (server, workers, reactor, ports, connections, callbacks, logs); Rust
//! runs each operation and asks native code for each fact or effect through one
//! `call(context, op, arg)`, reading facts afresh.
use core::ffi::c_void;

mod event_worker;
pub mod ops;
mod reactor_process;

/// One native fact or effect in `context`.
pub type WorkersCall = unsafe extern "C" fn(context: *mut c_void, op: u32, arg: u64) -> u64;

pub(crate) const SW_OK: i64 = 0;
pub(crate) const SW_ERR: i64 = -1;

/// `Server::Mode`.
pub(crate) const MODE_BASE: u64 = 1;
pub(crate) const MODE_PROCESS: u64 = 2;
pub(crate) const MODE_THREAD: u64 = 3;

/// What Rust asks of the native objects it runs over.
pub(crate) trait Host {
    fn op(&mut self, op: u32, arg: u64) -> u64;

    fn get(&mut self, op: u32) -> u64 {
        self.op(op, 0)
    }
    fn is(&mut self, op: u32) -> bool {
        self.get(op) != 0
    }
    fn signed(&mut self, op: u32) -> i64 {
        self.get(op).cast_signed()
    }
}

struct Native {
    context: *mut c_void,
    call: WorkersCall,
}

impl Host for Native {
    fn op(&mut self, op: u32, arg: u64) -> u64 {
        // SAFETY: a native fact or effect on the live objects in `context`.
        unsafe { (self.call)(self.context, op, arg) }
    }
}

/// Runs one worker-side operation (`ops::RUN_*`) with `arg`; the result is the
/// operation's return value as upstream's C++ function returns it.
///
/// # Safety
/// `call` acts on the objects in `context`, live for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_workers_run(
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

pub(crate) fn set_signal<H: Host>(h: &mut H, signo: i32, handler: u64) {
    h.op(
        ops::OP_SIGNAL_SET,
        u64::from(signo.cast_unsigned()) | (handler << 32),
    );
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
    }
}
