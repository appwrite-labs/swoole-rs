// SPDX-License-Identifier: Apache-2.0
//! base.cc's printf family: C variadics, which stable
//! Rust can't define (src/format.c).
#[path = "../phpcoroutines/build.rs"]
mod php_config;

mod platform;

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    for probe in ["HAVE_MUTEX_TIMEDLOCK", "HAVE_RWLOCK_TIMEDRDLOCK"] {
        println!("cargo::rustc-check-cfg=cfg({probe})");
        if platform::defines(&os).any(|name| name == probe) {
            println!("cargo::rustc-cfg={probe}");
        }
    }
    php_config::main();
    println!("cargo:rustc-check-cfg=cfg(ossl300)");
    let version = std::env::var("DEP_OPENSSL_VERSION_NUMBER").unwrap_or_default();
    if u64::from_str_radix(&version, 16).is_ok_and(|version| version >= 0x3000_0000) {
        println!("cargo:rustc-cfg=ossl300");
    }
    println!("cargo::rerun-if-changed=src/format.c");
    println!("cargo::rerun-if-changed=src/fatal.c");
    println!("cargo::rerun-if-changed=../../native/include/swoole_rs_corebase_constants.h");
    cc::Build::new()
        .include("../../native/include")
        .file("src/format.c")
        .file("src/fatal.c")
        .compile("swoole_rs_corebase_format");
}
