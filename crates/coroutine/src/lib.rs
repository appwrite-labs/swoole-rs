//! The coroutine scheduler and per-coroutine state, Channel, and context stack
//! bookkeeping. C++ runs the effects: swap callbacks, stack switches, timers and
//! destructors.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

pub mod chan;
mod coroutine;
pub use coroutine::*;
mod live;
pub use live::*;
mod stack;
pub use stack::*;
