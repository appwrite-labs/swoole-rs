//! `swoole.so`: every module crate's exports, through `swoole-rs-ffi`, linked
//! with the native objects `build.rs` compiles. Zend loads it by `get_module()`.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
pub use swoole_rs_ffi::*;
// build.rs compiles native/tls against the headers of the OpenSSL this links.
#[cfg(feature = "php")]
use openssl_sys as _;

#[cfg(feature = "php")]
unsafe extern "C" {
    /// native/phphelpers/module.cc: the module entry, built once with C++'s
    /// host ops. Safe to call: it takes no arguments.
    safe fn swr_cpp_phpswoole_module() -> *mut std::ffi::c_void;
}

/// The `zend_module_entry` Zend looks up when it loads the extension. A C
/// definition would stay hidden: a cdylib exports only Rust's symbols.
#[cfg(feature = "php")]
#[unsafe(no_mangle)]
pub extern "C" fn get_module() -> *mut std::ffi::c_void {
    swr_cpp_phpswoole_module()
}
