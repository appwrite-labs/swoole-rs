//! Bind the system liburing that pkg-config finds and compile the C shim over its
//! inline functions. The extension links liburing itself (`config.m4`); the
//! pkg-config probe links it for this workspace's own tests.

fn main() {
    #[cfg(feature = "liburing")]
    uring::bind();
}

#[cfg(feature = "liburing")]
#[expect(
    clippy::expect_used,
    reason = "a build script reports failure by panicking"
)]
mod uring {
    use std::{env, path::PathBuf};

    pub(super) fn bind() {
        for file in ["src/shim.h", "src/shim.c"] {
            println!("cargo::rerun-if-changed={file}");
        }
        let library = pkg_config::Config::new()
            .atleast_version("2.8")
            .probe("liburing")
            .expect("uring-sys: liburing >= 2.8 (pkg-config liburing)");
        let out = PathBuf::from(env::var("OUT_DIR").expect("uring-sys: OUT_DIR"));
        bindgen::Builder::default()
            .header("src/shim.h")
            .generate_comments(false)
            .derive_default(true)
            .clang_args(
                library
                    .include_paths
                    .iter()
                    .map(|dir| format!("-I{}", dir.display())),
            )
            .allowlist_function("(io_uring_|swr_uring_).*")
            .allowlist_type("(io_uring|__kernel_timespec).*")
            .allowlist_var("(IORING_|IOSQE_|IO_URING_VERSION_).*")
            .generate()
            .expect("uring-sys: bindgen")
            .write_to_file(out.join("bindings.rs"))
            .expect("uring-sys: write bindings");
        cc::Build::new()
            .file("src/shim.c")
            .includes(&library.include_paths)
            .warnings(true)
            .compile("swoole_rs_uring_shim");
    }
}
