//! PHP `Swoole\Coroutine\Channel` and `Swoole\Table` (with the `php` feature,
//! see docs/zend.md), with no `unsafe`.
#![cfg(feature = "php")]
#![forbid(unsafe_code)]

mod channel;
mod table;
pub use channel::*;
pub use table::*;
