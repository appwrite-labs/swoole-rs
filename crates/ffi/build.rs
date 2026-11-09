// SPDX-License-Identifier: Apache-2.0
//! src/fatal.c: what `swoole_init` needs of C (a variadic and libc's stdout).
fn main() {
    println!("cargo::rerun-if-changed=src/fatal.c");
    cc::Build::new()
        .file("src/fatal.c")
        .compile("swoole_rs_ffi_fatal");
}
