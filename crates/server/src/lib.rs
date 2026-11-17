//! `swoole::Server` operations. Rust decides every step and answers each fact or
//! effect from the records it lays out: the server's state, master.cc's method
//! context and the reactor-thread context, reading facts afresh. The interface
//! address table is Rust state (`local_addr`).
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
pub use base_factory::SendData;
pub use commands::{CommandCallback, CommandHandler};
pub use server_methods::{
    swr_v1_server_add_command, swr_v1_server_command, swr_v1_server_command_result,
};

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
#[cfg(feature = "upstream-symbols")]
mod dtls;
pub mod event_worker;
#[cfg(feature = "upstream-symbols")]
pub mod factory;
#[cfg(test)]
mod fixture;
// Linked for the upstream symbols the hybrid's modules call (Cargo.toml).
#[cfg(test)]
use swoole_rs_hooks as _;
mod hybrid;
#[cfg(any(test, feature = "upstream-symbols"))]
mod json;
pub mod local_addr;
pub mod manager;
mod master_ops;
#[cfg(feature = "upstream-symbols")]
mod objects;
#[cfg(feature = "upstream-symbols")]
mod pool_native;
pub mod process_worker;
#[cfg(feature = "upstream-symbols")]
pub use pool_native::process_pool_native;
pub mod port_config;
mod process_factory;
pub mod reactor_entry;
mod reactor_handle;
mod reactor_io;
mod reactor_ops;
pub mod reactor_process;
mod reactor_thread;
mod records;
mod send;
pub mod server_methods;
#[cfg(feature = "upstream-symbols")]
mod server_object;
mod shared;
mod startup;
mod state;
mod static_config;
pub mod static_handler;
pub mod static_select;
pub mod task_worker;
mod tcp_info;
pub mod thread_mode;
#[cfg(feature = "upstream-symbols")]
mod thread_ops;
pub mod view;
pub mod worker_methods;
pub use static_config::Collection as StaticCollection;

pub use records::{Connection, ConnectionRecord, ServerTables, Session, SessionRecord, Slot};

pub use shared::{PortCounters, ServerCounters, swr_v1_server_shared_init};

pub use config::{DispatchMode, Mode, ServerConfig, Shared, TaskIpcMode};

/// `SW_THREAD`: whether this build has thread mode, as the extension's MINIT says. Outside
/// `thread_mode::native`, which needs `upstream-symbols`, since crates/phpserver sets and reads it.
pub static THREAD_SUPPORTED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
pub(crate) use state::TestServer;
pub use state::{Ports, PortsIter, ServerState, ServerText};

#[cfg(feature = "upstream-symbols")]
pub use admin::admin_register;
#[cfg(feature = "upstream-symbols")]
pub use server_object::SharedServer;
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
pub use tcp_info::tcp_fields;

pub use reactor_ops::{pool_bus_move_packet, pool_bus_set_allocator};
