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

/// The `zend_module_entry` Zend looks up when it loads the extension. A C
/// definition would stay hidden: a cdylib exports only Rust's symbols.
#[cfg(feature = "php")]
#[unsafe(no_mangle)]
pub extern "C" fn get_module() -> *mut std::ffi::c_void {
    phpswoole_module().cast()
}

/// The whole-thread escape-scope ABI (`native/corebase/swoole_rs_zend_guard.h`) for
/// native extensions loaded beside this one. zend-sys's C definitions stay hidden,
/// as a cdylib exports only Rust's symbols, so each export is a naked tail jump to
/// one: no Rust frame, so a native C++ exception unwinds through C alone (Rust
/// would abort it, built with `panic = "abort"`).
macro_rules! guard_exports {
    ($($export:ident => $definition:ident),* $(,)?) => {
        #[cfg(feature = "php")]
        unsafe extern "C" {
            $(fn $definition();)*
        }
        $(
            #[doc = concat!("`", stringify!($definition), "`, exported.")]
            #[cfg(feature = "php")]
            #[unsafe(naked)]
            #[unsafe(no_mangle)]
            pub extern "C" fn $export() {
                #[cfg(target_arch = "x86_64")]
                core::arch::naked_asm!("jmp {}@PLT", sym $definition);
                #[cfg(target_arch = "aarch64")]
                core::arch::naked_asm!("b {}", sym $definition);
            }
        )*
    };
}

guard_exports! {
    swr_v1_zend_try => swr_zend_try,
    swr_v1_zend_guard_target => swr_zend_guard_target,
    swr_v1_zend_guard_resume => swr_zend_guard_resume,
}
