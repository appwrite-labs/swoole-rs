//! Each case package's build script: links SWOOLE_LIB_DIR's libswoole.a, the C and C++
//! tests/modules/hooks/system.py builds (the pin's thirdparty, native/'s Zend-free sources and
//! the package's fixture), whole, so its references into the crates resolve from the rlibs.
//! Unset (`cargo clippy --workspace`, which links nothing), it links nothing.
fn main() {
    println!("cargo:rerun-if-env-changed=SWOOLE_LIB_DIR");
    let Ok(dir) = std::env::var("SWOOLE_LIB_DIR") else {
        return;
    };
    println!("cargo:rerun-if-changed={dir}/libswoole.a");
    println!("cargo:rustc-link-search=native={dir}");
    println!("cargo:rustc-link-lib=static:+whole-archive=swoole");
    println!("cargo:rustc-link-lib=stdc++");
}
