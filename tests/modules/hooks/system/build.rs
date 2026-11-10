fn main() {
    let dir = std::env::var("SWOOLE_LIB_DIR")
        .expect("SWOOLE_LIB_DIR: system.py's lib/, holding the libswoole.so it builds");
    println!("cargo:rustc-link-search=native={dir}");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    println!("cargo:rerun-if-env-changed=SWOOLE_LIB_DIR");
}
