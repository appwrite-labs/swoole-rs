// SPDX-License-Identifier: Apache-2.0
//! `ossl300` when openssl-sys links OpenSSL 3, where `SSL_get_peer_certificate` is a
//! macro over `SSL_get1_peer_certificate`.
fn main() {
    println!("cargo:rustc-check-cfg=cfg(ossl300)");
    let version = std::env::var("DEP_OPENSSL_VERSION_NUMBER").unwrap_or_default();
    if u64::from_str_radix(&version, 16).is_ok_and(|version| version >= 0x3000_0000) {
        println!("cargo:rustc-cfg=ossl300");
    }
}
