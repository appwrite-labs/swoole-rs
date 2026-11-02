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

mod channel;
pub use channel::*;
mod coroutine;
pub use coroutine::*;
mod stack;
pub use stack::*;
