fn main() {
    #[cfg(feature = "libcares")]
    sdk();
}
#[cfg(feature = "libcares")]
#[expect(clippy::expect_used, reason = "a missing system SDK is a build error")]
fn sdk() {
    let sdk = pkg_config::Config::new()
        .probe("libcares")
        .expect("system c-ares SDK");
    println!("cargo::rerun-if-changed=src/sdk.c");
    cc::Build::new()
        .file("src/sdk.c")
        .includes(sdk.include_paths)
        .compile("swoole_rs_cares_sdk");
}
