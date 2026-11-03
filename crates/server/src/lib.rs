//! `swoole::Server` operations. Native code keeps the objects (connections, sockets,
//! ports, the reactor, callbacks and logs); Rust decides every step and asks native
//! code for each fact or effect through one `call(context, op, arg)`, reading facts
//! afresh. The interface address table is Rust state (`local_addr`).
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
use core::ffi::c_void;

mod connection;
mod control;
mod dispatch;
pub mod local_addr;
pub mod ops;
mod reactor_io;
mod reactor_thread;
mod startup;

pub use local_addr::{
    swr_v1_server_local_addr, swr_v1_server_local_addr_index, swr_v1_server_local_addrs_free,
    swr_v1_server_local_addrs_new,
};

/// One native fact or effect on the server in `context`.
pub type ServerCall = unsafe extern "C" fn(context: *mut c_void, op: u32, arg: u64) -> u64;

pub(crate) const SW_OK: i64 = 0;
pub(crate) const SW_ERR: i64 = -1;

/// `Server::Mode`; native asserts each.
pub(crate) const MODE_BASE: u64 = 1;
pub(crate) const MODE_PROCESS: u64 = 2;
pub(crate) const MODE_THREAD: u64 = 3;

/// What Rust asks of the server it runs on.
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
    fn float(&mut self, op: u32) -> f64 {
        f64::from_bits(self.get(op))
    }
}

struct Native {
    context: *mut c_void,
    call: ServerCall,
}

impl Host for Native {
    fn op(&mut self, op: u32, arg: u64) -> u64 {
        // SAFETY: a native fact or effect on the live server in `context`.
        unsafe { (self.call)(self.context, op, arg) }
    }
}

/// Runs one `Server` operation (`ops::RUN_*`) with `arg`; the result is the
/// operation's return value as upstream's C++ method returns it.
///
/// # Safety
/// `call` acts on the server objects in `context`, live for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_run(
    operation: u32,
    arg: u64,
    context: *mut c_void,
    call: ServerCall,
) -> i64 {
    run(&mut Native { context, call }, operation, arg)
}

fn run<H: Host>(h: &mut H, operation: u32, arg: u64) -> i64 {
    use ops::{
        RUN_ACCEPT, RUN_ADD_CONNECTION, RUN_CHILD_EXIT, RUN_CREATE, RUN_DESTROY,
        RUN_DISABLE_ACCEPT, RUN_ENABLE_ACCEPT, RUN_HEALTHY, RUN_IDLE_TIMEOUT, RUN_INCOMING,
        RUN_INIT_SIGNALS, RUN_MASTER_TIMER, RUN_SET_MAX_CONNECTION, RUN_SHUTDOWN, RUN_SIGNAL,
        RUN_SIGNAL_SHUTDOWN, RUN_START_CHECK, RUN_START_MASTER_THREAD, RUN_STOP_MASTER_THREAD,
        RUN_STORE_LISTEN_SOCKET,
    };
    match operation {
        RUN_ACCEPT => connection::accept(h),
        RUN_ADD_CONNECTION => i64::from(connection::add_connection(h)),
        RUN_INCOMING => connection::incoming(h),
        RUN_IDLE_TIMEOUT => {
            connection::idle_timeout(h);
            SW_OK
        }
        RUN_DISABLE_ACCEPT => {
            connection::disable_accept(h);
            SW_OK
        }
        RUN_ENABLE_ACCEPT => {
            connection::enable_accept(h);
            SW_OK
        }
        RUN_SET_MAX_CONNECTION => {
            connection::set_max_connection(h, u32::try_from(arg).unwrap_or(u32::MAX));
            SW_OK
        }
        RUN_HEALTHY => i64::from(connection::healthy(h, f64::from_bits(arg))),
        RUN_SHUTDOWN => i64::from(control::shutdown(h)),
        RUN_SIGNAL => {
            control::signal(h, i32::try_from(arg).unwrap_or(0));
            SW_OK
        }
        RUN_SIGNAL_SHUTDOWN => i64::from(control::signal_shutdown(h)),
        RUN_CHILD_EXIT => i64::from(control::child_exit(h)),
        RUN_STOP_MASTER_THREAD => {
            control::stop_master_thread(h);
            SW_OK
        }
        RUN_MASTER_TIMER => {
            control::master_timer(h);
            SW_OK
        }
        RUN_INIT_SIGNALS => {
            control::init_signals(h);
            SW_OK
        }
        RUN_CREATE => startup::create(h),
        RUN_START_CHECK => startup::start_check(h),
        RUN_START_MASTER_THREAD => startup::start_master_thread(h),
        RUN_STORE_LISTEN_SOCKET => {
            startup::store_listen_socket(h);
            SW_OK
        }
        RUN_DESTROY => {
            startup::destroy(h);
            SW_OK
        }
        _ => match reactor_io::run(h, operation, arg) {
            Some(result) => result,
            None => reactor_thread::run(h, operation, arg),
        },
    }
}

#[cfg(test)]
pub(crate) mod script {
    //! A scripted server: answers facts from a table and records every op.
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
