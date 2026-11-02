//! PHP coroutines (with the `php` feature, see docs/zend.md): upstream's
//! `PHPCoroutine` and `swoole_coroutine.cc`. Rust owns each coroutine's PHP
//! context and its VM swap (through the zend-sys shim, as the executor
//! globals' fields depend on the PHP version), activation, configuration, the
//! preemptive scheduler, `Swoole\Coroutine`'s methods, `go()`/`defer()`, the
//! exit and `@` handlers and the coroutine-aware autoloader.
//!
//! The coroutine core stays C++ and lends its operations as a table. The
//! coroutine's entry stays a C++ function that runs the user callable and its
//! defer tasks inside `zend_first_try`, so a bailout never crosses a Rust frame;
//! Rust frames that may switch coroutines own nothing that needs a drop.
#![cfg(feature = "php")]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

mod classes;
pub use classes::*;
mod context;
pub use context::*;
mod host;
mod methods;
pub use methods::*;
mod runtime;
pub use runtime::*;
