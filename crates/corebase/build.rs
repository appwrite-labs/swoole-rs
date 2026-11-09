// SPDX-License-Identifier: Apache-2.0
//! With `upstream-symbols`, base.cc's C variadics, which stable Rust can't
//! define (src/format.c): the printf family and the fatal-error hook.
fn main() {
    #[cfg(feature = "upstream-symbols")]
    {
        println!("cargo::rerun-if-changed=src/format.c");
        println!("cargo::rerun-if-changed=../../native/include/swoole_rs_corebase_state.h");
        cc::Build::new()
            .file("src/format.c")
            .include("../../native/include")
            .compile("swoole_rs_corebase_format");
    }
}
