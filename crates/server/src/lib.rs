//! `swoole::Server` operations. Native code keeps the objects (connections, sockets,
//! ports, the reactor, callbacks and logs); Rust decides every step and asks native
//! code for each fact or effect through one `call(context, op, arg)`, reading facts
//! afresh. The interface address table is Rust state (`local_addr`).
#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
use core::ffi::c_void;

pub use commands::{CommandFn, CommandReply, swr_v1_server_command_result};
pub use server_methods::{swr_v1_server_add_command, swr_v1_server_command};

#[cfg(any(test, feature = "upstream-symbols"))]
mod admin;
mod base_factory;
pub mod callbacks;
mod commands;
mod config;
mod conn_io;
mod connection;
mod control;
mod dispatch;
#[cfg(all(feature = "upstream-symbols", not(test)))]
mod dtls;
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub mod event_worker_ops;
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub mod factory;
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub mod factory_ops;
#[cfg(any(test, feature = "upstream-symbols"))]
mod json;
mod layout;
pub mod local_addr;
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub mod manager_ops;
mod master_ops;
#[cfg(all(feature = "upstream-symbols", not(test)))]
mod objects;
pub mod ops;
#[cfg(all(feature = "upstream-symbols", not(test)))]
mod pool_native;
pub mod port_config;
pub mod reactor_entry;
mod reactor_handle;
mod reactor_io;
mod reactor_ops;
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub mod reactor_process_ops;
mod reactor_thread;
mod records;
mod send;
pub mod server_methods;
#[cfg(all(feature = "upstream-symbols", not(test)))]
mod server_object;
mod shared;
mod startup;
mod state;
mod static_config;
pub mod static_handler;
pub mod static_select;
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub mod task_worker_ops;
pub mod tasks;
mod tcp_info;
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub mod thread_mode_ops;
#[cfg(all(feature = "upstream-symbols", not(test)))]
mod thread_ops;
pub mod view;
pub mod worker_methods;
pub use static_config::{
    swr_v1_server_static_config, swr_v1_server_static_mime, swr_v1_server_static_owner,
    swr_v1_server_static_owner_clone, swr_v1_server_static_owner_free,
    swr_v1_server_static_rewrite, swr_v1_server_static_rewrite_add, swr_v1_server_static_text,
};

pub use records::{
    ConnectionRecord, ServerTables, SessionRecord, swr_v1_server_connection, swr_v1_server_session,
};

pub use shared::{
    PortCounters, ServerCounters, swr_v1_server_port_connections, swr_v1_server_port_shared_init,
    swr_v1_server_shared_init,
};

pub use config::ServerConfig;

/// `SW_THREAD`: whether this build has thread mode, as the extension's MINIT says. Outside
/// `thread_mode_ops`, which needs `upstream-symbols`, since crates/phpserver sets and reads it.
pub static THREAD_SUPPORTED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

pub use state::{
    ServerState, swr_v1_server_startup_error, swr_v1_server_state_free, swr_v1_server_state_new,
    swr_v1_server_text_slot, swr_v1_server_upload_tmp_dir,
};

#[cfg(all(feature = "upstream-symbols", not(test)))]
pub use admin::swr_v1_server_admin_register;
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub use server_object::{swr_v1_server_delete, swr_v1_server_new};
pub use tcp_info::swr_v1_server_tcp_info;

pub use local_addr::swr_v1_server_local_addr_index;

/// One native fact or effect on the server in `context`.
pub type ServerCall = unsafe extern "C" fn(context: *mut c_void, op: u32, arg: u64) -> u64;

pub(crate) const SW_OK: i64 = 0;
pub(crate) const SW_ERR: i64 = -1;

/// `Server::Mode`; native asserts each.
pub(crate) const MODE_BASE: u64 = 1;
pub(crate) const MODE_PROCESS: u64 = 2;
pub(crate) const MODE_THREAD: u64 = 3;

/// Abnormal native effects stop the current operation without unwinding Rust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NativeFault(pub u32);
pub(crate) type Effect<T> = Result<T, NativeFault>;

/// What Rust asks of the server it runs on.
pub(crate) trait Host {
    fn op(&mut self, op: u32, arg: u64) -> Effect<u64>;
    // Inlined so each caller's constant op reaches `op`'s fact tables (#1435).
    #[inline(always)]
    fn get(&mut self, op: u32) -> Effect<u64> {
        self.op(op, 0)
    }
    #[inline(always)]
    fn is(&mut self, op: u32) -> Effect<bool> {
        Ok(self.get(op)? != 0)
    }
    #[inline(always)]
    fn signed(&mut self, op: u32) -> Effect<i64> {
        Ok(self.get(op)?.cast_signed())
    }
    fn float(&mut self, op: u32) -> Effect<f64> {
        Ok(f64::from_bits(self.get(op)?))
    }
    fn state(&mut self) -> Effect<&ServerState>;
    /// `swoole_get_worker_id()`.
    fn worker_id(&mut self) -> Effect<u64> {
        self.get(ops::OP_WORKER_ID)
    }
    /// A base-mode send made without native code (see `send::send_now`); `None`
    /// leaves all of it to `send_to_connection`.
    fn send_now(&mut self, _send: &base_factory::SendData) -> Effect<Option<i64>> {
        Ok(None)
    }
}

/// The state of a server without one, which native code never passes.
static NO_STATE: std::sync::LazyLock<ServerState> = std::sync::LazyLock::new(ServerState::new);

trait NativeCall {
    /// # Safety
    /// The context is live for the callback, whose native unwind contract holds.
    unsafe fn invoke(&self, context: *mut c_void, op: u32, arg: u64) -> Effect<u64>;
}
impl NativeCall for ServerCall {
    unsafe fn invoke(&self, context: *mut c_void, op: u32, arg: u64) -> Effect<u64> {
        // SAFETY: the caller lends the context under the legacy nonthrowing contract.
        Ok(unsafe { self(context, op, arg) })
    }
}
struct Native<C> {
    context: *mut c_void,
    call: C,
    state: *const ServerState,
    /// master.cc's context, whose records `master_ops` answers ops from; null
    /// for the other runners.
    master: *mut server_methods::NativeContext,
    /// master.cc's reactor-thread context, whose records `reactor_ops` answers ops
    /// from; null for the other runners.
    reactor: *mut reactor_ops::ReactorContext,
}

impl<C: NativeCall> Host for Native<C> {
    // Constant opcodes erase unrelated fact-table branches before native effects.
    #[inline(always)]
    fn op(&mut self, op: u32, arg: u64) -> Effect<u64> {
        if !self.state.is_null() {
            // SAFETY: run_state lends live synchronized records; this leaf access ends before callbacks.
            if let Some(value) = unsafe { config::native_fact(self.state, op, arg) } {
                return Ok(value);
            }
            // SAFETY: the same call lends synchronized shared records, without holding callbacks.
            if let Some(value) = unsafe { shared::native_fact(self.state, op, arg) } {
                return Ok(value);
            }
            // SAFETY: as above.
            let state = unsafe { &*self.state };
            // A slot is read or cleared, never called.
            if let Some(value) = callbacks::native_fact(state, op, arg) {
                return Ok(value);
            }
            // SAFETY: the root path op lends its terminated path.
            if let Some(value) = unsafe { state::native_fact(state, op, arg) } {
                return Ok(value);
            }
            if !self.reactor.is_null() {
                // SAFETY: the reactor runner lends master.cc's live context with its state.
                let answer = unsafe { reactor_ops::answer(self.reactor, self.state, op, arg) };
                if let Some(value) = answer {
                    return Ok(value);
                }
            }
        }
        // Only the method runner lends master.cc's context; the hot runners skip the call.
        if !self.master.is_null() {
            // SAFETY: the method runner lends master.cc's live context with its state.
            let answer = unsafe { master_ops::answer(self.master, self.state, op, arg) };
            if let Some(value) = answer {
                return Ok(value);
            }
        }
        // SAFETY: the entrypoint lends this context with C's native effect contract.
        unsafe { self.call.invoke(self.context, op, arg) }
    }

    fn worker_id(&mut self) -> Effect<u64> {
        // The process crate owns the worker record; no native call is needed.
        Ok(u64::from(swoole_rs_process::methods::swoole_get_worker_id()))
    }

    fn send_now(&mut self, send: &base_factory::SendData) -> Effect<Option<i64>> {
        // SAFETY: the entrypoint lends a null or live state; native code lends `send`.
        Ok(unsafe { send::send_now(self.state, send) })
    }

    // A null test on the hot runners; out of line it cost ~33 instructions a message (#1435).
    #[inline(always)]
    fn state(&mut self) -> Effect<&ServerState> {
        if !self.state.is_null() {
            // SAFETY: run_state lends this state for the entire operation.
            return Ok(unsafe { &*self.state });
        }
        let address = usize::try_from(self.get(ops::OP_STATE)?).unwrap_or(0);
        let state = core::ptr::with_exposed_provenance::<ServerState>(address);
        // SAFETY: native code passes `Server::state_`, which lives as long as the server.
        Ok(unsafe { state.as_ref() }.unwrap_or(&NO_STATE))
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
            master: core::ptr::null_mut(),
            reactor: core::ptr::null_mut(),
        },
        operation,
        arg,
    )
    .unwrap_or(SW_ERR)
}

/// Runs a reactor-thread operation over a `ReactorContext`, whose records Rust
/// reads, and the server's owned records.
///
/// # Safety
/// `state` is live for the call; `context` is a live `ReactorContext` lending
/// synchronized server objects.
pub(crate) unsafe fn run_reactor(
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
            master: core::ptr::null_mut(),
            reactor: context.cast(),
        },
        operation,
        arg,
    )
    .unwrap_or(SW_ERR)
}

/// An op no arm answers: 0, as master.cc's `default:` was.
pub(crate) unsafe extern "C" fn unanswered(_: *mut c_void, _: u32, _: u64) -> u64 {
    0
}

/// `operation` over a reactor-thread context without [`run`]'s match: the reads,
/// writes and dispatches every message takes, whose detour through `run`'s frame
/// cost about 120 instructions a message.
///
/// # Safety
/// As [`run_reactor`].
#[inline(always)]
pub(crate) unsafe fn run_reactor_with(
    context: &mut reactor_ops::ReactorContext,
    state: &ServerState,
    operation: impl FnOnce(&mut Native<ServerCall>) -> Effect<i64>,
) -> i64 {
    let reactor = core::ptr::from_mut(context);
    operation(&mut Native {
        context: reactor.cast(),
        call: unanswered,
        state,
        master: core::ptr::null_mut(),
        reactor,
    })
    .unwrap_or(SW_ERR)
}

/// Runs one operation for a `Server` method (`server_methods`) over its context,
/// whose records `master_ops` answers. No effect here calls back into C++.
///
/// # Safety
/// `ctx` is a live context whose `serv` is null or a live `Server`.
#[cfg_attr(test, expect(dead_code, reason = "server_methods' tests run a fake"))]
pub(crate) unsafe fn run_method(
    operation: u32,
    arg: u64,
    ctx: *mut server_methods::NativeContext,
) -> i64 {
    // SAFETY: per the caller.
    let serv = unsafe { (*ctx).serv };
    let state = if serv.is_null() {
        core::ptr::null()
    } else {
        // SAFETY: a live server, per the caller.
        unsafe { view::Server::from_raw(serv) }.state_ptr()
    };
    run(
        &mut Native {
            context: ctx.cast(),
            call: unanswered as ServerCall,
            state,
            master: ctx,
            reactor: core::ptr::null_mut(),
        },
        operation,
        arg,
    )
    .unwrap_or(SW_ERR)
}

fn run<H: Host>(h: &mut H, operation: u32, arg: u64) -> crate::Effect<i64> {
    use ops::{
        RUN_ACCEPT, RUN_ADD_CONNECTION, RUN_CHILD_EXIT, RUN_CREATE, RUN_DESTROY,
        RUN_DISABLE_ACCEPT, RUN_ENABLE_ACCEPT, RUN_IDLE_TIMEOUT, RUN_INCOMING, RUN_MASTER_TIMER,
        RUN_SET_MAX_CONNECTION, RUN_SHUTDOWN, RUN_SIGNAL, RUN_SIGNAL_SHUTDOWN, RUN_START_CHECK,
        RUN_START_MASTER_THREAD, RUN_STOP_MASTER_THREAD, RUN_STORE_LISTEN_SOCKET,
    };
    Ok(match operation {
        ops::RUN_NATIVE_OP => h
            .op(u32::try_from(arg).unwrap_or(u32::MAX), 0)?
            .cast_signed(),
        // Every message sent: matched here, not after the fallback chain below.
        ops::RUN_ST_SEND => i64::from(state::send(
            h,
            core::ptr::with_exposed_provenance(usize::try_from(arg).unwrap_or(0)),
        )?),
        ops::RUN_CLOSE_CONNECTION => conn_io::close_connection(h)?,
        RUN_ACCEPT => connection::accept(h)?,
        RUN_ADD_CONNECTION => i64::from(connection::add_connection(h)?),
        RUN_INCOMING => connection::incoming(h)?,
        RUN_IDLE_TIMEOUT => {
            connection::idle_timeout(h)?;
            SW_OK
        }
        RUN_DISABLE_ACCEPT => {
            connection::disable_accept(h)?;
            SW_OK
        }
        RUN_ENABLE_ACCEPT => {
            connection::enable_accept(h)?;
            SW_OK
        }
        RUN_SET_MAX_CONNECTION => {
            connection::set_max_connection(h, u32::try_from(arg).unwrap_or(u32::MAX))?;
            SW_OK
        }
        RUN_SHUTDOWN => i64::from(control::shutdown(h)?),
        RUN_SIGNAL => {
            control::signal(h, i32::try_from(arg).unwrap_or(0))?;
            SW_OK
        }
        RUN_SIGNAL_SHUTDOWN => i64::from(control::signal_shutdown(h)?),
        RUN_CHILD_EXIT => i64::from(control::child_exit(h)?),
        RUN_STOP_MASTER_THREAD => {
            control::stop_master_thread(h)?;
            SW_OK
        }
        RUN_MASTER_TIMER => {
            control::master_timer(h)?;
            SW_OK
        }
        RUN_CREATE => startup::create(h)?,
        RUN_START_CHECK => startup::start_check(h)?,
        ops::RUN_START => startup::start(h)?,
        RUN_START_MASTER_THREAD => startup::start_master_thread(h)?,
        RUN_STORE_LISTEN_SOCKET => {
            startup::store_listen_socket(h)?;
            SW_OK
        }
        RUN_DESTROY => {
            startup::destroy(h)?;
            SW_OK
        }
        _ => match reactor_io::run(h, operation, arg)? {
            Some(result) => result,
            None => match state::run(h, operation, arg)? {
                Some(result) => result,
                None if operation == ops::RUN_SC_SEND => send::send_to_connection(h, arg, None)?,
                None => reactor_thread::run(h, operation, arg)?,
            },
        },
    })
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
        fn op(&mut self, op: u32, arg: u64) -> crate::Effect<u64> {
            self.log.push((op, arg));
            if let Some(answers) = self.queued.get_mut(&op)
                && !answers.is_empty()
            {
                return Ok(answers.remove(0));
            }
            Ok(self.facts.get(&op).copied().unwrap_or(0))
        }
        fn state(&mut self) -> crate::Effect<&crate::ServerState> {
            Ok(&self.state)
        }
    }
}

#[cfg(test)]
mod fault_tests;

#[cfg(test)]
mod mode_tests {
    use super::{Host, Native, ops::OP_MODE};
    use core::ffi::c_void;

    /// Native code is never asked: any call fails the test.
    unsafe extern "C" fn unreachable(_: *mut c_void, op: u32, _: u64) -> u64 {
        panic!("native op {op}");
    }

    #[test]
    fn the_mode_is_the_configs() {
        let state = crate::swr_v1_server_state_new();
        // SAFETY: a fresh state owns its config.
        unsafe { &mut *(*state).config() }.mode_ = 2;
        let mut host = Native {
            context: core::ptr::null_mut(),
            call: unreachable as super::ServerCall,
            state,
            master: core::ptr::null_mut(),
            reactor: core::ptr::null_mut(),
        };
        assert_eq!(host.get(OP_MODE), Ok(2));
        // SAFETY: created above and not used again.
        unsafe { crate::swr_v1_server_state_free(state) };
    }
}

/// `message_bus->move_packet()`: the current packet's block, now the caller's.
///
/// # Safety
/// `message_bus` is a live `MessageBus` that only this thread uses meanwhile.
pub unsafe fn bus_move_packet(message_bus: *mut core::ffi::c_void) -> *mut core::ffi::c_char {
    // SAFETY: as the caller guarantees; a `MessageBus` starts with its Rust bus.
    let bus = unsafe { *message_bus.cast::<*mut swoole_rs_bus::MessageBus>() };
    // SAFETY: that live bus, only this thread's meanwhile.
    unsafe { &mut *bus }.move_packet()
}
