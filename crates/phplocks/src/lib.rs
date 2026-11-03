//! PHP `Swoole\Atomic`, `Swoole\Atomic\Long`, `Swoole\Lock` and (ZTS)
//! `Swoole\Thread\Atomic`, `Swoole\Thread\Atomic\Long` and `Swoole\Thread\Lock`:
//! class registration, objects and every method, over the locks crate's atomic
//! words and locks. C++ keeps the global pool the process-shared words and
//! locks live in. See docs/zend.md.
//!
//! Without the `php` feature the crate is empty.
#![cfg(feature = "php")]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

mod atomic;
mod lock;
mod thread;
mod word;

pub use atomic::*;
pub use lock::*;
pub use thread::*;
