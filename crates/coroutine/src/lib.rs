//! The coroutine scheduler and per-coroutine state, Channel, each coroutine's
//! context (its stack and the switch onto it) and `swoole::Coroutine`'s
//! operations. C++ runs the effects: swap callbacks, timers and destructors.
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
mod live;
pub use live::*;
mod stack;
pub use stack::*;
