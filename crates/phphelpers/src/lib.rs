//! Common Zend value ownership and operations, plus legacy adapter policy.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
#[cfg(feature = "php")]
pub mod callable;
#[cfg(feature = "php")]
mod common;
#[cfg(feature = "php")]
mod known_strings;
#[cfg(feature = "php")]
mod private;
#[cfg(feature = "php")]
mod utilities;

#[cfg(feature = "php")]
pub use common::{known_free, known_init, sw_zend_known_strings};
#[cfg(feature = "php")]
pub use private::{
    Create, array_property_of, function_call, named_call, swr_v1_phphelpers_cache_free,
    swr_v1_phphelpers_call, swr_v1_phphelpers_call_anyway, swr_v1_phphelpers_named_call,
};
