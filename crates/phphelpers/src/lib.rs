//! Common Zend value ownership and operations, plus asynchronous and legacy adapter policy.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
#[cfg(feature = "php")]
mod array;
mod asynchronous;
#[cfg(feature = "php")]
pub mod callable;
#[cfg(feature = "php")]
mod common;
#[cfg(feature = "php")]
mod known_strings;
#[cfg(feature = "php")]
mod owned;
#[cfg(feature = "php")]
mod private;
#[cfg(feature = "php")]
mod utilities;

#[cfg(feature = "php")]
pub use private::{Create, function_call, named_call};
