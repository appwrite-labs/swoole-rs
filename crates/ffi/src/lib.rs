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

/// Return the migrated kernels: every module, as the bits `MODULES` in `tools/build_hybrid.py` names.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_feature_bits() -> u64 {
    (1 << 28) - 1
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

pub use swoole_rs_memory::*;

pub use swoole_rs_http::*;

pub use swoole_rs_utilities::*;

pub use swoole_rs_locks::*;

#[cfg(feature = "php")]
pub use swoole_rs_phplocks::*;

pub use swoole_rs_wrappers::*;

#[cfg(feature = "php")]
pub use swoole_rs_phptimer::*;

#[cfg(feature = "php")]
pub use swoole_rs_phpswoole::*;

#[cfg(feature = "php")]
pub use swoole_rs_phpredis::*;

pub use swoole_rs_framing::{
    swr_v1_framing_default, swr_v1_framing_hton64, swr_v1_framing_mime_count,
    swr_v1_framing_mime_entry, swr_v1_framing_mime_fallback, swr_v1_framing_mime_suffix,
    swr_v1_framing_recv, swr_v1_framing_unpack, swr_v1_framing_width,
};

pub use swoole_rs_reactor::*;

pub use swoole_rs_corebase::*;

pub use swoole_rs_coroutine::*;

pub use swoole_rs_hooks::*;
pub use swoole_rs_phpruntime as phpruntime;

pub use swoole_rs_ossupport::*;

// Reactor's Socket patch calls the TLS reads and writes in crates/tls.
pub use swoole_rs_tls::*;

pub use swoole_rs_process::*;

#[cfg(feature = "php")]
pub use swoole_rs_phpprocess::*;

#[cfg(feature = "php")]
pub use swoole_rs_phpthread::*;

pub use swoole_rs_clients as clients;
#[cfg(feature = "php")]
pub use swoole_rs_phpclients::*;
// The clients patch calls the HTTP inflate stream in crates/inflate.
pub use swoole_rs_inflate as inflate;

// Keep local Rust type names scoped; C exports retain their no_mangle names.
pub use swoole_rs_httpbindings as httpbindings;

#[cfg(feature = "php")]
pub use swoole_rs_phphttp::*;

pub use swoole_rs_http2 as http2;
#[cfg(feature = "php")]
pub use swoole_rs_phphttp2::*;

pub use swoole_rs_ports as ports;

#[cfg(feature = "php")]
pub use swoole_rs_phpserver::*;

pub use swoole_rs_phphelpers as phphelpers;
pub use swoole_rs_phpstdext as phpstdext;

pub use swoole_rs_phpcollections as phpcollections;

pub use swoole_rs_phpevents as phpevents;

pub use swoole_rs_phpcoroutines as phpcoroutines;

#[cfg(feature = "pgsql")]
pub use swoole_rs_pgsql as pgsql;

#[cfg(feature = "odbc")]
pub use swoole_rs_odbc as odbc;

#[cfg(feature = "firebird")]
pub use swoole_rs_firebird as firebird;

#[cfg(feature = "curl")]
pub use swoole_rs_curl as curl;

#[cfg(feature = "ssh2")]
pub use swoole_rs_ssh2 as ssh2;
