//! The coroutine scheduler and per-coroutine state, Channel, and each coroutine's
//! context: its stack and the switch onto it. C++ runs the effects: swap
//! callbacks, timers and destructors.
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
mod live;
pub use live::*;
mod stack;
pub use stack::*;
