//! PHP `Swoole\Thread`, `Swoole\Thread\Map`, `ArrayList`, `Queue` and `Barrier`
//! (ZTS builds): values copied out of one thread's request memory into tables
//! and queues every thread shares, and copied back into PHP values on read.
//! Upstream's `ArrayItem` and `ZendArray` live here, as does each PHP
//! thread's request; the Lock and Atomic resources a value may hold are
//! crates/phplocks'.
//! See docs/zend.md.
//!
//! Without the `php` feature the crate is empty.
#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
#![cfg(feature = "php")]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

mod array;
mod barrier;
mod classes;
mod host;
mod item;
mod queue;
mod request;
mod run;
mod table;
mod thread;

pub use array::*;
pub use barrier::*;
pub use classes::*;
pub use run::*;
pub use thread::*;
