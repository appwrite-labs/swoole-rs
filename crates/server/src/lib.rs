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

pub use commands::{
    CommandInfo, swr_v1_server_command_add, swr_v1_server_command_at, swr_v1_server_command_attach,
    swr_v1_server_command_callback, swr_v1_server_command_drain, swr_v1_server_command_find,
    swr_v1_server_command_handler, swr_v1_server_command_request,
};

mod admin;
mod commands;
mod config;
mod conn_io;
mod connection;
mod control;
mod dispatch;
mod json;
mod layout;
pub mod local_addr;
pub mod ops;
pub mod port_config;
mod reactor_io;
mod reactor_thread;
mod records;
mod send;
mod shared;
mod startup;
mod state;
mod static_config;
pub use static_config::{
    swr_v1_server_static_config, swr_v1_server_static_mime, swr_v1_server_static_owner,
    swr_v1_server_static_owner_clone, swr_v1_server_static_owner_free, swr_v1_server_static_text,
};

pub use records::{
    ConnectionRecord, ServerTables, SessionRecord, swr_v1_server_connection, swr_v1_server_session,
    swr_v1_server_session_fd, swr_v1_server_tables_get,
};

pub use shared::{
    PortCounters, ServerCounters, swr_v1_server_port_connections, swr_v1_server_port_shared_init,
    swr_v1_server_shared_init,
};

pub use config::{ServerConfig, swr_v1_server_config_get, swr_v1_server_config_init};

pub use state::{
    ServerState, swr_v1_server_startup_error, swr_v1_server_state_free, swr_v1_server_state_new,
};

pub use admin::swr_v1_server_tcp_info;
pub use admin::{swr_v1_server_admin_command, swr_v1_server_admin_run};

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
    /// The server's Rust-owned state.
    fn state(&mut self) -> &ServerState;
}

/// The state of a server without one, which native code never passes.
static NO_STATE: ServerState = ServerState::new();

struct Native {
    context: *mut c_void,
    call: ServerCall,
    state: *const ServerState,
}

impl Host for Native {
    fn op(&mut self, op: u32, arg: u64) -> u64 {
        if !self.state.is_null() {
            // SAFETY: run_state lends live synchronized records; this leaf access ends before callbacks.
            let fact = unsafe { config::native_fact(self.state, op, arg) };
            // SAFETY: the same call lends synchronized shared records, without holding callbacks.
            let fact = fact.or_else(|| unsafe { shared::native_fact(self.state, op, arg) });
            if let Some(value) = fact {
                return value;
            }
        }
        // SAFETY: a native fact or effect on the live server in `context`.
        unsafe { (self.call)(self.context, op, arg) }
    }
    fn state(&mut self) -> &ServerState {
        if !self.state.is_null() {
            // SAFETY: run_state lends this state for the entire operation.
            return unsafe { &*self.state };
        }
        let address = usize::try_from(self.get(ops::OP_STATE)).unwrap_or(0);
        let state = core::ptr::with_exposed_provenance::<ServerState>(address);
        // SAFETY: native code passes `Server::state_`, which lives as long as the server.
        unsafe { state.as_ref() }.unwrap_or(&NO_STATE)
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
    run(
        &mut Native {
            context,
            call,
            state: core::ptr::null(),
        },
        operation,
        arg,
    )
}

/// Runs a server operation directly over the server's owned records.
/// Native callbacks are used only for effects/loans outside those records.
///
/// # Safety
/// `state` is null or live for the call; `context` and callbacks lend synchronized
/// server objects and catch any native/PHP bailout before returning to Rust.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_run_state(
    operation: u32,
    arg: u64,
    context: *mut c_void,
    call: ServerCall,
    state: *const ServerState,
) -> i64 {
    run(
        &mut Native {
            context,
            call,
            state,
        },
        operation,
        arg,
    )
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
        ops::RUN_RT_READ => conn_io::read(h, arg),
        ops::RUN_RT_WRITE => conn_io::write(h, arg),
        ops::RUN_PEER_CLOSE => conn_io::peer_close(h, arg),
        ops::RUN_CLOSE_CONNECTION => conn_io::close_connection(h),
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
        ops::RUN_REOPEN_LOGGER => {
            control::reopen_logger(h);
            1
        }
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
            None => match state::run(h, operation, arg) {
                Some(result) => result,
                None if operation == ops::RUN_SC_SEND => send::send_to_connection(h, arg),
                None => reactor_thread::run(h, operation, arg),
            },
        },
    }
}

#[cfg(test)]
pub(crate) mod script {
    //! A scripted server: answers facts from a table and records every op.
    use super::Host;
    use crate::ServerState;
    use std::collections::HashMap;

    #[derive(Default)]
    pub(crate) struct Script {
        pub facts: HashMap<u32, u64>,
        /// Answers consumed in order before `facts` for ops called repeatedly.
        pub queued: HashMap<u32, Vec<u64>>,
        pub log: Vec<(u32, u64)>,
        pub state: ServerState,
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
        fn state(&mut self) -> &ServerState {
            &self.state
        }
    }
}
