//! PHP `Swoole\Atomic`, `Swoole\Atomic\Long`, `Swoole\Lock` and (ZTS)
//! `Swoole\Thread\Atomic`, `Swoole\Thread\Atomic\Long` and `Swoole\Thread\Lock`:
//! class registration, objects and every method, over atomic words and the
//! locks crate's locks, with no `unsafe`. Process-shared words and locks live
//! in Swoole's global memory pool. See docs/zend.md.
//!
//! Without the `php` feature the crate is empty.
#![cfg(feature = "php")]
#![forbid(unsafe_code)]

mod atomic;
mod coroutine;
mod lock;
mod thread;
mod word;

pub use atomic::*;
pub use coroutine::*;
pub use lock::*;
pub use thread::*;
