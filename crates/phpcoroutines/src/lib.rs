//! PHP coroutines (with the `php` feature, see docs/zend.md): upstream's
//! `PHPCoroutine` and `swoole_coroutine.cc`. Rust owns each coroutine's PHP
//! context and its VM swap (through the zend-sys shim, as the executor
//! globals' fields depend on the PHP version), activation, configuration, the
//! preemptive scheduler, `Swoole\Coroutine`'s methods, `go()`/`defer()`, the
//! exit and `@` handlers and the coroutine-aware autoloader.
//!
//! Rust reads the coroutine core's state directly; `host.rs` names what it
//! asks of the rest of Swoole. The coroutine's entry runs the user callable and its defer
//! tasks inside the zend-sys shim's `zend_first_try`; Rust frames that may
//! switch coroutines or bail out own nothing that needs a drop.
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

/// Per-thread state, as Swoole's `SW_THREAD_LOCAL`: thread-local on a
/// thread-safe PHP, plain process state otherwise (one PHP thread, and no
/// TLS lookup on the coroutine switch path).
macro_rules! per_thread {
    ($($(#[$meta:meta])* static $name:ident: $ty:ty = $init:expr;)*) => {
        #[cfg(zts)]
        thread_local! { $($(#[$meta])* static $name: $ty = const { $init };)* }
        $(
            $(#[$meta])*
            #[cfg(not(zts))]
            static $name: crate::Process<$ty> = crate::Process($init);
        )*
    };
}

/// A process-wide value with `LocalKey::with`'s interface, for a
/// non-thread-safe PHP.
#[cfg(not(zts))]
pub(crate) struct Process<T>(pub(crate) T);

// SAFETY: a non-thread-safe PHP runs PHP on one thread; the interrupt thread
// touches none of this state.
#[cfg(not(zts))]
unsafe impl<T> Sync for Process<T> {}

#[cfg(not(zts))]
impl<T> Process<T> {
    #[inline]
    pub(crate) fn with<R>(&'static self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.0)
    }
}

pub mod channel;
mod classes;
pub use classes::*;
mod context;
mod host;
pub use host::{Coroutine, Waiter, counts, in_coroutine, sleep, suspend, suspend_for};
mod methods;
pub use methods::*;
mod runtime;
pub use runtime::*;
mod scheduler;
pub use scheduler::*;
mod system;
pub use system::*;
mod tracer;
pub use tracer::*;
