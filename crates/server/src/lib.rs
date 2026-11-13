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
#[cfg(all(feature = "upstream-symbols", not(test)))]
mod dtls;
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub mod event_worker_ops;
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub mod factory;
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub mod factory_ops;
#[cfg(test)]
mod fixture;
mod hybrid;
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
pub mod reactor_process;
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
pub mod task_worker;
mod tcp_info;
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub mod thread_mode_ops;
#[cfg(all(feature = "upstream-symbols", not(test)))]
mod thread_ops;
pub mod view;
pub mod worker_methods;
pub use static_config::{
    swr_v1_server_static_config, swr_v1_server_static_mime, swr_v1_server_static_owner,
    swr_v1_server_static_owner_clone, swr_v1_server_static_owner_free, swr_v1_server_static_text,
};

pub use records::{
    Connection, ConnectionRecord, ServerTables, Session, SessionRecord, Slot,
    swr_v1_server_connection, swr_v1_server_session,
};

pub use shared::{
    PortCounters, ServerCounters, swr_v1_server_port_shared_init, swr_v1_server_shared_init,
};

pub use config::ServerConfig;

/// `SW_THREAD`: whether this build has thread mode, as the extension's MINIT says. Outside
/// `thread_mode_ops`, which needs `upstream-symbols`, since crates/phpserver sets and reads it.
pub static THREAD_SUPPORTED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

pub use state::{ServerState, swr_v1_server_state_free, swr_v1_server_state_new};

#[cfg(all(feature = "upstream-symbols", not(test)))]
pub use admin::admin_register;
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub use server_object::{SharedServer, swr_v1_server_delete, swr_v1_server_new};
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
pub use tcp_info::tcp_fields;

pub(crate) const SW_OK: i64 = 0;
pub(crate) const SW_ERR: i64 = -1;

/// `Server::Mode`; native asserts each.
pub(crate) const MODE_BASE: u64 = 1;
pub(crate) const MODE_PROCESS: u64 = 2;
pub(crate) const MODE_THREAD: u64 = 3;

/// `message_bus->move_packet()`: the current packet's block, now the caller's.
///
/// # Safety
/// `message_bus` is a live `MessageBus` that only this thread uses meanwhile.
pub unsafe fn bus_move_packet(message_bus: *mut core::ffi::c_void) -> *mut core::ffi::c_char {
    // SAFETY: as the caller guarantees; a `MessageBus` starts with its Rust bus.
    let bus = unsafe { *message_bus.cast::<*mut swoole_rs_bus::MessageBus>() };
    // SAFETY: that live bus, only this thread's meanwhile.
    unsafe { &*bus }.move_packet()
}
