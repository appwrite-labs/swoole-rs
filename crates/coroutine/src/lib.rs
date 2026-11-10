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
pub use facade::{resumed, swr_v1_coroutine_create, wait};
mod live;
pub use live::*;
mod scheduler;
pub use scheduler::{
    Routine, current, resume, scheduler, swr_v1_coroutine_bail, yield_with_cancel,
};
mod stack;
pub use stack::*;
mod thread;
pub use thread::{
    THREAD, swr_v1_thread_context_clean, swr_v1_thread_context_init, thread_context_active,
};
