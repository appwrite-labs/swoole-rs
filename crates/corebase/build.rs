// SPDX-License-Identifier: Apache-2.0
//! With `upstream-symbols`, base.cc's printf family: C variadics, which stable
//! Rust can't define (src/format.c).
fn main() {
    #[cfg(feature = "upstream-symbols")]
    {
        println!("cargo::rerun-if-changed=src/format.c");
        cc::Build::new()
            .file("src/format.c")
            .compile("swoole_rs_corebase_format");
    }
}
