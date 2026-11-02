//! Versioned C seam for the hybrid extension; base queries are allocation-free.
//!
//! These queries accept no pointers, invoke no callbacks, and cannot panic.
//! PHP/Zend values and all runtime ownership remain with upstream Swoole.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

/// Return the supported C ABI version.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_abi_version() -> u32 {
    1
}

/// Return enabled migrated kernels; graduated ones (base64, buffer, crc32, error, framing, http, protocols, websocket, timer) are always set.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_feature_bits() -> u64 {
    1 | (1 << 1)
        | (1 << 2)
        | (1 << 3)
        | (1 << 4)
        | (1 << 5)
        | (u64::from(cfg!(feature = "memory")) << 6)
        | (1 << 7)
        | (1 << 8)
        | (u64::from(cfg!(feature = "utilities")) << 9)
        | (u64::from(cfg!(feature = "locks")) << 10)
        | (u64::from(cfg!(feature = "wrappers")) << 11)
        | (u64::from(cfg!(feature = "reactor")) << 14)
        | (u64::from(cfg!(feature = "corebase")) << 13)
        | (1 << 12)
        | (u64::from(cfg!(feature = "coroutine")) << 15)
        | (u64::from(cfg!(feature = "hooks")) << 16)
        | (u64::from(cfg!(feature = "ossupport")) << 17)
        | (u64::from(cfg!(feature = "tls")) << 18)
        | (u64::from(cfg!(feature = "process")) << 19)
        | (u64::from(cfg!(feature = "clients")) << 20)
        | (u64::from(cfg!(feature = "httpbindings")) << 21)
        | (u64::from(cfg!(feature = "http2")) << 22)
        | (u64::from(cfg!(feature = "ports")) << 23)
        | (u64::from(cfg!(feature = "phphelpers")) << 24)
        | (u64::from(cfg!(feature = "phpcollections")) << 25)
        | (u64::from(cfg!(feature = "phpevents")) << 26)
        | (u64::from(cfg!(feature = "phpcoroutines")) << 27)
}

pub use swoole_rs_base64::{swr_v1_base64_decode, swr_v1_base64_encode};

pub use swoole_rs_crc32::swr_v1_crc32;

pub use swoole_rs_error::*;

pub use swoole_rs_websocket::*;

pub use swoole_rs_buffer::*;

pub use swoole_rs_buffer::string::*;

pub use swoole_rs_timer::*;

pub use swoole_rs_server::{swr_v1_server_run, *};

pub use swoole_rs_protocols::*;

#[cfg(feature = "memory")]
pub use swoole_rs_memory::*;

pub use swoole_rs_http::*;

#[cfg(feature = "utilities")]
pub use swoole_rs_utilities::*;

#[cfg(feature = "locks")]
pub use swoole_rs_locks::*;

#[cfg(feature = "locks")]
pub use swoole_rs_phplocks::*;

#[cfg(feature = "wrappers")]
pub use swoole_rs_wrappers::*;

#[cfg(feature = "php")]
pub use swoole_rs_phptimer::*;

#[cfg(feature = "php")]
pub use swoole_rs_phpswoole::*;

pub use swoole_rs_framing::{
    swr_v1_framing_default, swr_v1_framing_hton64, swr_v1_framing_mime_count,
    swr_v1_framing_mime_entry, swr_v1_framing_mime_fallback, swr_v1_framing_mime_suffix,
    swr_v1_framing_step, swr_v1_framing_unpack, swr_v1_framing_width,
};

#[cfg(feature = "reactor")]
pub use swoole_rs_reactor::*;

#[cfg(feature = "corebase")]
pub use swoole_rs_corebase::*;

#[cfg(feature = "coroutine")]
pub use swoole_rs_coroutine::*;

#[cfg(feature = "hooks")]
pub use swoole_rs_hooks::*;

#[cfg(feature = "ossupport")]
pub use swoole_rs_ossupport::*;

// Reactor's Socket patch calls the TLS reads and writes in crates/tls.
#[cfg(any(feature = "tls", feature = "reactor"))]
pub use swoole_rs_tls::*;

#[cfg(feature = "process")]
pub use swoole_rs_process::*;

#[cfg(feature = "process")]
pub use swoole_rs_phpprocess::*;

#[cfg(feature = "process")]
pub use swoole_rs_phpthread::*;

#[cfg(feature = "clients")]
pub use swoole_rs_clients as clients;
#[cfg(feature = "clients")]
pub use swoole_rs_phpclients::*;
// The clients patch calls the HTTP inflate stream in crates/inflate.
#[cfg(feature = "clients")]
pub use swoole_rs_inflate as inflate;

#[cfg(feature = "httpbindings")]
// Keep local Rust type names scoped; C exports retain their no_mangle names.
pub use swoole_rs_httpbindings as httpbindings;

#[cfg(feature = "httpbindings")]
pub use swoole_rs_phphttp::*;

#[cfg(feature = "http2")]
pub use swoole_rs_http2 as http2;
#[cfg(feature = "http2")]
pub use swoole_rs_phphttp2::*;

#[cfg(feature = "ports")]
pub use swoole_rs_ports as ports;

#[cfg(feature = "phphelpers")]
pub use swoole_rs_phphelpers as phphelpers;
#[cfg(feature = "phphelpers")]
pub use swoole_rs_phpstdext as phpstdext;

#[cfg(feature = "phpcollections")]
pub use swoole_rs_phpcollections as phpcollections;

#[cfg(feature = "phpevents")]
pub use swoole_rs_phpevents as phpevents;

#[cfg(feature = "phpcoroutines")]
pub use swoole_rs_phpcoroutines as phpcoroutines;
