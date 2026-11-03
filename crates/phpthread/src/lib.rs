//! PHP `Swoole\Thread`, `Swoole\Thread\Map`, `ArrayList`, `Queue` and `Barrier`
//! (ZTS builds): values copied out of one thread's request memory into tables
//! and queues every thread shares, and copied back into PHP values on read.
//! Upstream's `ArrayItem` and `ZendArray` live here; the Lock and Atomic
//! resources a value may hold are crates/phplocks', and C++ keeps the sockets,
//! reached through `Host`.
//! See docs/zend.md.
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

mod array;
mod barrier;
mod classes;
mod host;
mod item;
mod queue;
mod request;
mod table;
mod thread;

pub use array::*;
pub use barrier::*;
pub use classes::*;
pub use host::swr_v1_phpthread_minit;
pub use request::{
    swr_v1_phpthread_request, swr_v1_phpthread_rinit, swr_v1_phpthread_rshutdown,
    swr_v1_phpthread_stdio_close, swr_v1_phpthread_stdio_open,
};
pub use thread::*;
