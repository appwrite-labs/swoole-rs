// SPDX-License-Identifier: Apache-2.0
//! With `upstream-symbols`, base.cc's printf family: C variadics, which stable
//! Rust can't define (src/format.c).
#[path = "../phpcoroutines/build.rs"]
mod php_config;

fn main() {
    php_config::main();
    println!("cargo:rustc-check-cfg=cfg(ossl300)");
    let version = std::env::var("DEP_OPENSSL_VERSION_NUMBER").unwrap_or_default();
    if u64::from_str_radix(&version, 16).is_ok_and(|version| version >= 0x3000_0000) {
        println!("cargo:rustc-cfg=ossl300");
    }
    #[cfg(feature = "upstream-symbols")]
    {
        println!("cargo::rerun-if-changed=src/format.c");
        println!("cargo::rerun-if-changed=src/fatal.c");
        cc::Build::new()
            .file("src/format.c")
            .file("src/fatal.c")
            .compile("swoole_rs_corebase_format");
    }
}
