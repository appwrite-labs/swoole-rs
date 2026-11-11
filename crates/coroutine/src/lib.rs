//! The coroutine scheduler and per-coroutine state, Channel, each coroutine's
//! context (its stack and the switch onto it) and `swoole::Coroutine`'s
//! operations. C++ runs the effects: swap callbacks, timers and destructors.
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

pub mod chan;
pub mod context;
mod coroutine;
pub use coroutine::*;
#[cfg(all(feature = "upstream-symbols", not(test)))]
mod facade;
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub use facade::{
    activate, cancel, deactivate, print_socket_bound_error, swoole_coroutine_count,
    swr_v1_coroutine_create, yield_ex, yield_resumed, yield_timed,
};
mod live;
pub use live::*;
mod scheduler;
pub use scheduler::{
    Routine, coroutine_count, coroutine_get, coroutine_older, current, resume, scheduler,
    set_callback, set_stack_size, swr_v1_coroutine_bail, yield_with_cancel,
};
mod stack;
pub use stack::*;
mod thread;
pub use thread::{
    THREAD, swr_v1_thread_context_clean, swr_v1_thread_context_init, thread_context_active,
};
